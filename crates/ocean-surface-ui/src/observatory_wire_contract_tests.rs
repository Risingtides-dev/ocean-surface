//! Ocean Floor's Observatory client stays inside ocean-os's published
//! Observatory wire (`docs/contracts/observatory-wire.json`, vendored at
//! `tests/fixtures/ocean-os-observatory-wire/`), including the typed
//! `StreamGap` envelope ocean-os #523 put on the `message` frame.
//!
//! The domain types and reducer are public; the adapter's EventSource loop is
//! wasm-only, so its frame names, query fields and resync branch are read off
//! `observatory/adapter.rs` (non-test half only).

use crate::observatory::domain::{
    EventEnvelope, EventKind, EventPayload, IntegrityState, ObservatorySnapshot, ObservatoryState,
    TruthProvenance,
};
use crate::observatory::reducer::apply;
use crate::wire_contract_support::*;
use serde_json::{json, Value};

const ADAPTER_RS: &str = include_str!("observatory/adapter.rs");

#[test]
fn observatory_wire_event_kinds_equal_the_published_set() {
    // Neither enum has a catch-all, so an unknown kind fails the whole frame:
    // the sets must be EQUAL, not merely overlapping.
    assert_eq!(
        variant_names::<EventKind>(),
        strings(&observatory_wire(), "/events/event_kinds")
    );
}

#[test]
fn observatory_wire_payload_kinds_equal_the_published_set() {
    let contract = observatory_wire();
    let tag = string(&contract, "/events/payload_tag");
    assert_eq!(
        tagged_variant_names::<EventPayload>(|probe| json!({ tag.as_str(): probe, "data": {} })),
        strings(&contract, "/events/payload_kinds")
    );
}

#[test]
fn observatory_wire_envelope_and_snapshot_fields_are_published() {
    let contract = observatory_wire();
    assert_subset(
        "EventEnvelope",
        field_names::<EventEnvelope>().iter().map(String::as_str),
        &strings(&contract, "/events/envelope_fields"),
    );
    assert_subset(
        "ObservatorySnapshot",
        field_names::<ObservatorySnapshot>()
            .iter()
            .map(String::as_str),
        &strings(&contract, "/snapshot/response_fields"),
    );
    // Cursors are decimal STRINGS on the wire (ocean-observatory's Cursor
    // serializes with serialize_str).
    let snapshot = object_with(
        &strings(&contract, "/snapshot/response_fields"),
        |key| match key {
            "nodes" | "edges" | "attention" => json!([]),
            _ => json!("7"),
        },
    );
    let decoded: ObservatorySnapshot = serde_json::from_value(snapshot).unwrap();
    assert_eq!(decoded.watermark_cursor, "7");
}

/// The gap signal exactly as ocean-os's `gap_envelope` builds it: an ordinary
/// envelope on the `message` frame, `kind` = gap_kind, payload = gap_payload_kind
/// `{from_cursor, to_cursor, reason}`, `truth` = gap_truth, empty topology,
/// `cursor` = the first missing cursor, all cursors as strings.
fn gap_envelope(from: u64, to: u64) -> Value {
    let contract = observatory_wire();
    json!({
        "schema_version": 1,
        "cursor": (from + 1).to_string(),
        "event_id": "4f7c1f0e-gap",
        "observatory_id": "obs-1",
        "daemon_instance_id": "daemon-1",
        "occurred_at": "2026-09-26T10:00:00Z",
        "recorded_at": "2026-09-26T10:00:00Z",
        "kind": string(&contract, "/events/gap_kind"),
        "truth": string(&contract, "/events/gap_truth"),
        "producer": { "kind": "daemon", "id": "ocean-daemon" },
        "topology": {
            "execution_id": "", "root_execution_id": "",
            "parent_execution_id": null, "edge_id": null,
            "session_id": "", "turn_id": "", "request_id": ""
        },
        "correlation": { "tool_call_id": null, "permission_id": null },
        "visibility": "metadata",
        "payload": {
            string(&contract, "/events/payload_tag"): string(&contract, "/events/gap_payload_kind"),
            "data": {
                "from_cursor": from.to_string(),
                "to_cursor": to.to_string(),
                "reason": "durable log skipped"
            }
        }
    })
}

#[test]
fn observatory_wire_typed_stream_gap_decodes_and_forces_a_resync() {
    let frame = gap_envelope(41, 45);
    // The gap envelope carries only published envelope fields.
    assert_subset(
        "the gap envelope",
        keys(&frame).iter().map(String::as_str),
        &strings(&observatory_wire(), "/events/envelope_fields"),
    );
    // Before ocean-os #523 the tail sent an untyped `{"kind":"stream.gap"}`
    // object and this decode failed ("unparseable Observatory frame").
    let event: EventEnvelope = serde_json::from_value(frame)
        .unwrap_or_else(|err| panic!("the published gap envelope does not decode: {err}"));
    assert_eq!(event.kind, EventKind::StreamGap);
    assert_eq!(event.truth, TruthProvenance::Derived);
    assert_eq!(event.cursor, "42");
    match &event.payload {
        EventPayload::StreamGap {
            from_cursor,
            to_cursor,
            ..
        } => assert_eq!((from_cursor.as_str(), to_cursor.as_str()), ("41", "45")),
        other => panic!("gap payload decoded as {other:?}"),
    }

    let state = ObservatoryState {
        observatory_id: "obs-1".into(),
        daemon_instance_id: "daemon-1".into(),
        cursor: 41,
        ..ObservatoryState::default()
    };
    let after = apply(state, event);
    assert_eq!(
        after.integrity,
        IntegrityState::Gap,
        "a typed gap must mark the state incomplete"
    );
    // ...and the live tail treats Gap as "fetch a fresh snapshot".
    let adapter = Source::production(ADAPTER_RS);
    assert!(adapter
        .find_code("IntegrityState::Gap | IntegrityState::Stale")
        .is_some());
}

#[test]
fn observatory_wire_tail_frames_and_queries_are_published() {
    let contract = observatory_wire();
    let adapter = Source::production(ADAPTER_RS);
    let frames = strings(&contract, "/events/frames");
    for frame in ["message", "reset"] {
        assert!(
            adapter
                .find_code(&format!("source.subscribe(\"{frame}\")"))
                .is_some(),
            "the tail no longer subscribes to `{frame}`"
        );
        assert!(frames.contains(frame), "{frame}");
    }
    // Every Observatory route the adapter calls, and every query field it
    // sends, is published. Read generically so a rewrite of the snapshot
    // fetch (#226 drops `?at=`) keeps the check honest either way round.
    let routes = [
        ("/snapshot/route", "/snapshot/query_fields"),
        ("/events/route", "/events/query_fields"),
    ]
    .map(|(route, query)| {
        let (method, path) = split_route(&string(&contract, route));
        assert_eq!(method, "GET");
        (path_shape(&path), strings(&contract, query))
    });
    let calls = adapter.v1_literals();
    for (shape, _) in &routes {
        assert!(
            calls.iter().any(|(_, lit)| path_shape(lit) == *shape),
            "the adapter no longer calls {shape}"
        );
    }
    for (_, literal) in &calls {
        let shape = path_shape(literal);
        let (_, fields) = routes
            .iter()
            .find(|(published, _)| *published == shape)
            .unwrap_or_else(|| panic!("the adapter calls unpublished `{literal}`"));
        let sent: Vec<&str> = literal
            .split_once('?')
            .map(|(_, query)| {
                query
                    .split('&')
                    .filter_map(|kv| kv.split('=').next())
                    .collect()
            })
            .unwrap_or_default();
        assert_subset(&format!("the query of `{literal}`"), sent, fields);
    }
}
