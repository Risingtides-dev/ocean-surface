//! Rooms DoD 5.8, surface half: the Rooms decoders cover ocean-os's published
//! room wire contract.
//!
//! ocean-os publishes `docs/contracts/room-wire.json` and holds it equal to
//! its router (`room_wire_contract_matches_the_daemon`). This repo vendors that
//! file at `tests/fixtures/ocean-os-room-wire/` via
//! `scripts/vendor-ocean-os-room-wire.mjs`, and CI's `script guards` job fails
//! when the vendored copy falls behind ocean-os `main`. These tests are the
//! other half: after a re-vendor, a value the surface cannot decode or does not
//! handle turns red HERE, naming the value, rather than surfacing as a room
//! that silently drops a frame or refuses to open.
//!
//! A child module of `rooms` (declared there with `#[path]`) because the crate
//! is a binary: an integration test in `tests/` cannot import a single item,
//! and the envelopes and the tail-frame decoder are private to `rooms`.

use super::*;
use serde_json::{json, Value};

const CONTRACT: &str = include_str!("../tests/fixtures/ocean-os-room-wire/room-wire.json");

const REVENDOR: &str = "re-vendored with `node scripts/vendor-ocean-os-room-wire.mjs`";

fn contract() -> Value {
    serde_json::from_str(CONTRACT).expect("vendored room-wire.json is valid JSON")
}

/// One of the contract's string lists. Missing or empty is a failure: a list
/// the vendored file stopped carrying would otherwise make every loop below
/// pass vacuously.
fn list(key: &str) -> Vec<String> {
    let values: Vec<String> = contract()[key]
        .as_array()
        .unwrap_or_else(|| panic!("room-wire.json has no `{key}` array ({REVENDOR}?)"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("room-wire.json `{key}` holds a non-string: {value}"))
                .to_string()
        })
        .collect();
    assert!(!values.is_empty(), "room-wire.json `{key}` is empty");
    values
}

// The surface's own wire spelling of every variant, by EXHAUSTIVE match: a
// variant added to one of these enums does not compile until it is named here,
// and the `..._are_all_in_the_contract` tests then require the daemon to
// publish it. The deserialize direction is the other half.

fn access_state_wire(state: RoomAccessState) -> &'static str {
    match state {
        RoomAccessState::Local => "local",
        RoomAccessState::Connecting => "connecting",
        RoomAccessState::Live => "live",
        RoomAccessState::Recovering => "recovering",
        RoomAccessState::Revoked => "revoked",
    }
}

fn message_kind_wire(kind: RoomMessageKind) -> &'static str {
    match kind {
        RoomMessageKind::Message => "message",
        RoomMessageKind::ParticipantJoined => "participant_joined",
        RoomMessageKind::ParticipantLeft => "participant_left",
        RoomMessageKind::System => "system",
    }
}

fn participant_kind_wire(kind: RoomParticipantKind) -> &'static str {
    match kind {
        RoomParticipantKind::Human => "human",
        RoomParticipantKind::Agent => "agent",
        RoomParticipantKind::Bot => "bot",
        RoomParticipantKind::Tool => "tool",
        RoomParticipantKind::System => "system",
    }
}

const ACCESS_STATES: [RoomAccessState; 5] = [
    RoomAccessState::Local,
    RoomAccessState::Connecting,
    RoomAccessState::Live,
    RoomAccessState::Recovering,
    RoomAccessState::Revoked,
];

const MESSAGE_KINDS: [RoomMessageKind; 4] = [
    RoomMessageKind::Message,
    RoomMessageKind::ParticipantJoined,
    RoomMessageKind::ParticipantLeft,
    RoomMessageKind::System,
];

const PARTICIPANT_KINDS: [RoomParticipantKind; 5] = [
    RoomParticipantKind::Human,
    RoomParticipantKind::Agent,
    RoomParticipantKind::Bot,
    RoomParticipantKind::Tool,
    RoomParticipantKind::System,
];

fn decode<T: serde::de::DeserializeOwned>(what: &str, wire: &str) -> T {
    serde_json::from_value(Value::String(wire.to_string())).unwrap_or_else(|err| {
        panic!(
            "room-wire.json lists {what} `{wire}` but the surface cannot decode it ({err}). \
             Add the variant (and handle it everywhere the compiler then points) before \
             landing the {REVENDOR} copy."
        )
    })
}

#[test]
fn room_wire_every_access_state_decodes_and_round_trips() {
    for wire in list("access_states") {
        let state: RoomAccessState = decode("access_state", &wire);
        assert_eq!(access_state_wire(state), wire);
        // `RoomAccessProjection` is also serialized back to the daemon (outbox
        // retry echoes it), so the write direction must spell it the same way.
        assert_eq!(serde_json::to_value(state).unwrap(), Value::String(wire));
    }
}

#[test]
fn room_wire_every_message_kind_decodes() {
    for wire in list("message_kinds") {
        let kind: RoomMessageKind = decode("message_kind", &wire);
        assert_eq!(message_kind_wire(kind), wire);
    }
}

#[test]
fn room_wire_every_participant_kind_decodes_and_labels() {
    for wire in list("participant_kinds") {
        let kind: RoomParticipantKind = decode("participant_kind", &wire);
        assert_eq!(participant_kind_wire(kind), wire);
        assert_eq!(
            serde_json::to_value(kind).unwrap(),
            Value::String(wire.clone())
        );
        // The roster chip's label is the wire word; a kind the daemon adds must
        // not render under another kind's name.
        assert_eq!(
            kind.label(),
            wire,
            "roster label for participant kind `{wire}`"
        );
    }
}

#[test]
fn room_wire_surface_variants_are_all_in_the_contract() {
    let access = list("access_states");
    for state in ACCESS_STATES {
        let wire = access_state_wire(state);
        assert!(
            access.iter().any(|v| v == wire),
            "surface access state `{wire}` is not in room-wire.json access_states"
        );
    }
    let messages = list("message_kinds");
    for kind in MESSAGE_KINDS {
        let wire = message_kind_wire(kind);
        assert!(
            messages.iter().any(|v| v == wire),
            "surface message kind `{wire}` is not in room-wire.json message_kinds"
        );
    }
    let participants = list("participant_kinds");
    for kind in PARTICIPANT_KINDS {
        let wire = participant_kind_wire(kind);
        assert!(
            participants.iter().any(|v| v == wire),
            "surface participant kind `{wire}` is not in room-wire.json participant_kinds"
        );
    }
}

/// A frame the daemon could send under each SSE event name this surface
/// handles. A contract event with no entry here is an event the tail does not
/// handle, which is the failure `room_wire_the_tail_handles_every_sse_event`
/// exists to name.
fn sample_frame(event: &str) -> Option<String> {
    let frame = match event {
        "room_message" => json!({
            "seq": 7,
            "author_id": "ari",
            "author_kind": "human",
            "kind": "message",
            "body": "hello",
            "created_at": "2026-09-25T10:00:00Z",
        }),
        "room_access" => json!({ "state": "live" }),
        "room_read_cursor" => json!({ "room_id": "room-1", "read_seq": "7" }),
        _ => return None,
    };
    Some(frame.to_string())
}

#[test]
fn room_wire_the_tail_handles_every_sse_event() {
    // The subscriptions live inside the wasm-only tail loop, which a host test
    // cannot run, so read them off the source. Scoped to the non-test half so
    // a test quoting a name cannot satisfy it.
    let source = include_str!("rooms.rs");
    let production = source.split("\n#[cfg(test)]").next().unwrap_or(source);
    for event in list("sse_events") {
        let frame = sample_frame(&event).unwrap_or_else(|| {
            panic!(
                "room-wire.json lists SSE event `{event}` and the room tail does not handle it: \
                 subscribe to it in `Rooms`' tail loop, decode it in `decode_room_tail_frame`, \
                 and give it a sample frame here"
            )
        });
        assert!(
            decode_room_tail_frame(&event, &frame, "room-1").is_some(),
            "decode_room_tail_frame drops a well-formed `{event}` frame"
        );
        let subscribe = format!("es.subscribe(\"{event}\")");
        assert!(
            production.contains(&subscribe),
            "the room tail never subscribes to `{event}` ({subscribe} not found in rooms.rs)"
        );
    }
}

/// A plausible value for every top-level key `/snapshot` or `/transcript`
/// answers. A contract key with no entry here is one this surface has not
/// ruled on — decoded or deliberately ignored — and the test names it.
fn sample_value(key: &str) -> Option<Value> {
    let message = json!({
        "seq": 3, "author_id": "ari", "author_kind": "human",
        "kind": "message", "body": "hi", "created_at": "2026-09-25T10:00:00Z",
    });
    Some(match key {
        "ok" => json!(true),
        "room" => json!({ "id": "room-1", "name": "Room one" }),
        "transcript" => json!([message]),
        "access" => json!({ "state": "local" }),
        "agent_owners" => json!([{ "agent_id": "a", "owner_id": "ari", "owner_present": true }]),
        "closed" => json!(true),
        "has_more" => json!(true),
        "last_seq" => json!(3),
        "prev_seq" => json!(2),
        "next_seq" => json!(4),
        // Deliberately not decoded by either envelope: the roster rides on
        // `room.participants`, and aliases are not rendered. Listed so the
        // ruling is explicit and a NEW key still fails below.
        "participants" => json!([]),
        "aliases" => json!([]),
        _ => return None,
    })
}

fn body_with_exactly(keys_field: &str) -> Value {
    let mut body = serde_json::Map::new();
    for key in list(keys_field) {
        let value = sample_value(&key).unwrap_or_else(|| {
            panic!(
                "room-wire.json `{keys_field}` lists `{key}` and the surface has not ruled on it: \
                 decode it in the envelope (or rule it ignored) and give it a sample value here"
            )
        });
        body.insert(key, value);
    }
    Value::Object(body)
}

#[test]
fn room_wire_snapshot_envelope_accepts_exactly_the_contract_keys() {
    let body = body_with_exactly("snapshot_keys");
    let snapshot: RoomSnapshotResponse = serde_json::from_value(body)
        .unwrap_or_else(|err| panic!("RoomSnapshotResponse refuses a contract body: {err}"));
    // Each field the surface reads came from the key the contract names, not a
    // serde default.
    let keys = list("snapshot_keys");
    let has = |key: &str| keys.iter().any(|k| k == key);
    assert_eq!(snapshot.ok, has("ok"));
    assert_eq!(snapshot.room.is_some(), has("room"));
    assert_eq!(snapshot.transcript.len(), usize::from(has("transcript")));
    assert_eq!(snapshot.access.state, RoomAccessState::Local);
    assert_eq!(snapshot.last_seq.is_some(), has("last_seq"));
    assert_eq!(snapshot.prev_seq.is_some(), has("prev_seq"));
    assert_eq!(snapshot.has_more, has("has_more"));
    assert_eq!(snapshot.closed, has("closed"));
    assert_eq!(snapshot.agent_owners.is_some(), has("agent_owners"));
    // `access` is the one REQUIRED field: the contract must keep publishing it.
    assert!(
        has("access"),
        "snapshot_keys dropped `access`, which every open requires"
    );
}

#[test]
fn room_wire_transcript_envelope_accepts_exactly_the_contract_keys() {
    let body = body_with_exactly("transcript_keys");
    let page: TranscriptResponse = serde_json::from_value(body)
        .unwrap_or_else(|err| panic!("TranscriptResponse refuses a contract body: {err}"));
    let keys = list("transcript_keys");
    let has = |key: &str| keys.iter().any(|k| k == key);
    assert_eq!(page.ok, has("ok"));
    assert_eq!(page.transcript.len(), usize::from(has("transcript")));
    assert_eq!(page.next_seq.is_some(), has("next_seq"));
    assert_eq!(page.has_more, has("has_more"));
}

#[test]
fn room_wire_not_open_answer_is_the_one_the_error_mappers_key_on() {
    let not_open = &contract()["not_open"];
    assert_eq!(not_open["status"], json!(404), "not_open.status");
    // attachments.rs, room_artifacts.rs and room_summary.rs decode this marker
    // as `room_not_open: bool`; a renamed marker would silently stop matching.
    assert_eq!(
        not_open["marker"],
        json!("room_not_open"),
        "not_open.marker is what the Rooms error mappers decode; rename their field with it"
    );
    assert_eq!(not_open["code"], json!("room_not_found"), "not_open.code");
}
