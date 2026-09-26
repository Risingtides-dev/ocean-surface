//! Surface stays inside ocean-os's published session wire
//! (`docs/contracts/session-wire.json`, vendored at
//! `tests/fixtures/ocean-os-session-wire/`).
//!
//! A child module of `daemon` (declared there with `#[path]`) because the crate
//! is a binary and the request/response types are private to `daemon`. The
//! rules, from the contract README: a decoder knows every published variant it
//! claims to handle and relies on nothing unpublished; every encoder body is a
//! subset of the published request fields.
//!
//! Where Surface is ALREADY outside the contract, the gap is named in a
//! `KNOWN_*` list below rather than hidden: each entry is checked to still be
//! unpublished, so the day ocean-os publishes it the entry has to go, and
//! anything NOT on a list still fails.

use super::*;
use crate::wire_contract_support::*;
use serde_json::json;
use std::collections::BTreeSet;

const DAEMON_RS: &str = include_str!("daemon.rs");

fn production() -> Source<'static> {
    Source::production(DAEMON_RS)
}

/// A recorded gap stays a gap: every entry is still unpublished. Once ocean-os
/// publishes one, it stops being drift and must leave the list.
fn assert_still_unpublished(list: &str, entries: &[&str], published: &BTreeSet<String>) {
    for entry in entries {
        assert!(
            !published.contains(*entry),
            "`{entry}` in {list} is now published by ocean-os; drop it from the list"
        );
    }
}

fn with_known(published: &BTreeSet<String>, known: &[&str]) -> BTreeSet<String> {
    let mut all = published.clone();
    all.extend(known.iter().map(|k| k.to_string()));
    all
}

/// The query parameter names of a URL.
fn query_names(url: &str) -> BTreeSet<String> {
    url.split_once('?')
        .map(|(_, query)| {
            query
                .split('&')
                .filter_map(|pair| pair.split('=').next())
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn assert_calls(route: &str, literal: &str) {
    assert!(
        production().find_code(literal).is_some(),
        "daemon.rs no longer calls {route} through {literal}"
    );
}

// ---- /v1/agent/events --------------------------------------------------------

#[test]
fn session_wire_every_subscribed_agent_event_is_published() {
    // gloo's EventSource only delivers frames whose `event:` name was
    // subscribed, and the daemon names each frame by its type, so this list IS
    // the decoder's claim.
    assert_subset(
        "AGENT_EVENT_NAMES (the /v1/agent/events subscriptions)",
        AGENT_EVENT_NAMES.iter().copied(),
        &strings(&session_wire(), "/agent_event_types"),
    );
}

#[test]
fn session_wire_agent_events_decode_by_the_published_tag() {
    let contract = session_wire();
    let tag = string(&contract, "/agent_event_tag");
    for event_type in strings(&contract, "/agent_event_types") {
        let decoded = serde_json::from_value::<AgentEvent>(json!({ tag.as_str(): event_type }));
        if AGENT_EVENT_NAMES.contains(&event_type.as_str()) {
            // Handled: the tag must select a real variant. With no payload the
            // only acceptable failure is a missing field of THAT variant.
            match decoded {
                Ok(AgentEvent::Other) => panic!(
                    "subscribed agent event `{event_type}` decodes to Other: no variant \
                     answers to the published `{tag}` value"
                ),
                Ok(_) => {}
                Err(err) => assert!(
                    err.to_string().contains("missing field"),
                    "`{event_type}` failed for a reason other than its payload: {err}"
                ),
            }
        } else {
            // A published type Surface does not handle (model_rerouted,
            // provider_retrying, session_config_changed, slack_canvas) must
            // degrade to Other, never fail the frame.
            assert!(
                matches!(decoded, Ok(AgentEvent::Other)),
                "unhandled published event `{event_type}` must decode to Other"
            );
        }
    }
}

#[test]
fn session_wire_agent_events_route_and_query_are_published() {
    let contract = session_wire();
    assert_eq!(
        string(&contract, "/agent_events_route"),
        "GET /v1/agent/events"
    );
    assert_calls(
        "GET /v1/agent/events",
        "\"{}/v1/agent/events?session_id={}\"",
    );
    assert_subset(
        "the scoped agent stream query",
        query_names("/v1/agent/events?session_id={}")
            .iter()
            .map(String::as_str),
        &strings(&contract, "/agent_events_query_fields"),
    );
    // Recorded, not asserted: Surface does not subscribe to the published
    // `event: error` reset frame (agent_events_error), so a `live_lag` /
    // `anchor_unavailable` reset on a live connection is dropped by gloo at the
    // transport and the transcript is not resynced until the next reconnect.
}

// ---- POST /v1/agent/sessions --------------------------------------------------

/// Fields Surface sends that the daemon does not publish (and, having no
/// `deny_unknown_fields`, silently drops): `title` is the lazy-create title
/// hint from `session_title_hint`, which therefore never reaches the daemon.
const KNOWN_UNPUBLISHED_CREATE_REQUEST: &[&str] = &["title"];

/// Response keys Surface decodes that the daemon does not publish. All are
/// `Option`/defaulted and so inert today, but `dispatch_prompt` PREFERS
/// `workspace_root` over the published `cwd` and adopts `title` as the header
/// title, and both reads are dead against the current daemon.
const KNOWN_UNPUBLISHED_CREATE_RESPONSE: &[&str] = &["error", "ok", "title", "workspace_root"];

/// Surface client types the daemon's harness profile does not know. An
/// unlisted `client_type` is accepted and runs with the CLI profile, so the
/// Tauri desktop's sessions and turns get the CLI harness, not the Web one.
const KNOWN_UNPUBLISHED_CLIENT_TYPES: &[&str] = &["surface-tauri"];

#[test]
fn session_wire_session_create_request_is_inside_the_published_fields() {
    let contract = session_wire();
    assert_eq!(
        string(&contract, "/session_create_route"),
        "POST /v1/agent/sessions"
    );
    assert_calls("POST /v1/agent/sessions", "\"{}/v1/agent/sessions\"");
    let published = strings(&contract, "/session_create_request_fields");
    assert_still_unpublished(
        "KNOWN_UNPUBLISHED_CREATE_REQUEST",
        KNOWN_UNPUBLISHED_CREATE_REQUEST,
        &published,
    );
    let body = AgentSessionCreateRequest {
        title: Some("t"),
        workspace_root: "/w",
        project_id: Some("p"),
        client_type: Some("surface-web"),
    };
    let sent = keys(&serde_json::to_value(&body).unwrap());
    assert_subset(
        "AgentSessionCreateRequest",
        sent.iter().map(String::as_str),
        &with_known(&published, KNOWN_UNPUBLISHED_CREATE_REQUEST),
    );
    // The one REQUIRED daemon field.
    assert!(sent.contains("workspace_root"));
}

#[test]
fn session_wire_session_create_response_decodes_from_the_published_keys() {
    let contract = session_wire();
    let published = strings(&contract, "/session_create_response_keys");
    assert_still_unpublished(
        "KNOWN_UNPUBLISHED_CREATE_RESPONSE",
        KNOWN_UNPUBLISHED_CREATE_RESPONSE,
        &published,
    );
    assert_subset(
        "AgentSessionCreateResponse",
        field_names::<AgentSessionCreateResponse>()
            .iter()
            .map(String::as_str),
        &with_known(&published, KNOWN_UNPUBLISHED_CREATE_RESPONSE),
    );
    // A body of exactly the published keys is a success with a session id,
    // and the root falls back to the published `cwd`.
    let body = object_with(&published, |key| json!(format!("{key}-value")));
    let response: AgentSessionCreateResponse = serde_json::from_value(body).unwrap();
    assert!(
        response.ok,
        "a published create answer carries no `ok` and must read as success"
    );
    assert_eq!(response.session_id.as_deref(), Some("session_id-value"));
    assert_eq!(
        response.workspace_root.or(response.cwd).as_deref(),
        Some("cwd-value")
    );
}

#[test]
fn session_wire_surface_client_types_are_known_to_the_daemon() {
    let contract = session_wire();
    let known = strings(&contract, "/session_create_client_types/known");
    assert_eq!(
        at(&contract, "/session_create_client_types/open"),
        &json!(true),
        "client_type must stay an open set, or surface-tauri is refused outright"
    );
    assert_still_unpublished(
        "KNOWN_UNPUBLISHED_CLIENT_TYPES",
        KNOWN_UNPUBLISHED_CLIENT_TYPES,
        &known,
    );
    let sent = production().literals_after("fn surface_client_type()");
    assert!(!sent.is_empty());
    assert_subset(
        "surface_client_type()",
        sent.iter().map(String::as_str),
        &with_known(&known, KNOWN_UNPUBLISHED_CLIENT_TYPES),
    );
    // The per-turn voice routing tag.
    assert!(known.contains(VOICE_CLIENT_TYPE), "{VOICE_CLIENT_TYPE}");
}

// ---- POST /v1/agent/turns -----------------------------------------------------

/// Turn fields Surface can send that the daemon does not publish: `room_id` is
/// always `None` (and skipped), but `canvas` rides every turn whose canvas has
/// a placed component and is silently dropped by the daemon.
const KNOWN_UNPUBLISHED_TURN_REQUEST: &[&str] = &["canvas", "room_id"];

#[test]
fn session_wire_turn_request_is_inside_the_published_fields() {
    let contract = session_wire();
    assert_eq!(
        string(&contract, "/agent_turn/route"),
        "POST /v1/agent/turns"
    );
    assert_calls("POST /v1/agent/turns", "\"{}/v1/agent/turns\"");
    let published = strings(&contract, "/agent_turn/request_fields");
    assert_still_unpublished(
        "KNOWN_UNPUBLISHED_TURN_REQUEST",
        KNOWN_UNPUBLISHED_TURN_REQUEST,
        &published,
    );
    let images = vec![TurnImage {
        mime_type: "image/png".into(),
        data: "AA==".into(),
    }];
    let canvas = crate::canvas::CanvasContext {
        active_canvas_id: None,
        canvases: Vec::new(),
    };
    // Every field populated, so nothing hides behind skip_serializing_if.
    let body = AgentTurnRequest {
        prompt: "p",
        cwd: "/",
        session_id: Some("s"),
        project_id: Some("p"),
        client_type: Some("surface-web"),
        guidance: Some(vec!["g".into()]),
        room_id: Some("pm"),
        thinking_level: Some("high"),
        model_id: Some("m"),
        images: Some(images),
        decision_token: Some("t"),
        client_context: Some(ClientContext::default()),
        agent: Some("a"),
        canvas: Some(canvas),
    };
    let sent = keys(&serde_json::to_value(&body).unwrap());
    assert_subset(
        "AgentTurnRequest",
        sent.iter().map(String::as_str),
        &with_known(&published, KNOWN_UNPUBLISHED_TURN_REQUEST),
    );
}

#[test]
fn session_wire_turn_response_decodes_from_the_published_keys() {
    let contract = session_wire();
    let published = strings(&contract, "/agent_turn/response_fields");
    assert_subset(
        "AgentTurnResponse",
        field_names::<AgentTurnResponse>()
            .iter()
            .map(String::as_str),
        &published,
    );
    let body = object_with(&published, |key| match key {
        "ok" => json!(true),
        "status" => json!("accepted"),
        _ => json!(format!("{key}-value")),
    });
    // Only the fields Surface reads get string samples; the numeric ones are
    // not decoded, so any shape is fine for them.
    let response: AgentTurnResponse = serde_json::from_value(body).unwrap();
    assert!(response.ok);
    assert_eq!(response.turn_id, "turn_id-value");
    assert_eq!(response.session_id, "session_id-value");
}

// ---- GET /v1/agent/sessions ----------------------------------------------------

#[test]
fn session_wire_session_list_is_inside_the_published_shape() {
    let contract = session_wire();
    assert_eq!(
        string(&contract, "/session_list/route"),
        "GET /v1/agent/sessions"
    );
    assert_subset(
        "the session-list query",
        query_names(&sessions_page_url("http://d", Some("c")))
            .iter()
            .map(String::as_str),
        &strings(&contract, "/session_list/query_fields"),
    );
    assert_subset(
        "fetch_all_sessions' SessionsResponse",
        production()
            .struct_fields("struct SessionsResponse")
            .iter()
            .map(String::as_str),
        &strings(&contract, "/session_list/response_fields"),
    );
    let summary = strings(&contract, "/session_list/summary_fields");
    assert_subset(
        "SessionSummary",
        field_names::<SessionSummary>().iter().map(String::as_str),
        &summary,
    );
    // The only required ones are published, so a published row decodes.
    let row = object_with(&summary, |key| match key {
        "turn_count" => json!(3),
        "owning_project" => json!({ "id": "p", "name": "P" }),
        "active_state" => json!("running"),
        _ => json!(format!("{key}-value")),
    });
    let decoded: SessionSummary = serde_json::from_value(row).unwrap();
    assert_eq!(decoded.id, "id-value");
    assert_eq!(decoded.active_state, Some(SessionRunState::Running));
}

// ---- GET /v1/sessions/{id} (legacy detail) ------------------------------------

#[test]
fn session_wire_legacy_session_detail_envelope_is_published() {
    let contract = session_wire();
    let (method, path) = split_route(&string(&contract, "/legacy_session_detail/route"));
    assert_eq!(
        (method.as_str(), path_shape(&path).as_str()),
        ("GET", "/v1/sessions/{}")
    );
    assert_calls("GET /v1/sessions/{id}", "\"{}/v1/sessions/{}\"");
    // The inner `session` object (SessionDetail) is deliberately unpublished
    // (README "Not published, on purpose"), so only the envelope is pinned.
    assert_subset(
        "SessionDetailResponse",
        field_names::<SessionDetailResponse>()
            .iter()
            .map(String::as_str),
        &strings(&contract, "/legacy_session_detail/response_fields"),
    );
}

// ---- POST /v1/requests/{id}/cancel ---------------------------------------------

#[test]
fn session_wire_request_cancel_is_inside_the_published_shape() {
    let contract = session_wire();
    assert_eq!(
        string(&contract, "/request_cancel/route"),
        "POST /v1/requests/{id}/cancel"
    );
    assert_calls(
        "POST /v1/requests/{id}/cancel",
        "\"{}/v1/requests/{request_id}/cancel\"",
    );
    let published = strings(&contract, "/request_cancel/response_fields");
    assert_subset(
        "RequestCancelResponse",
        field_names::<RequestCancelResponse>()
            .iter()
            .map(String::as_str),
        &published,
    );
    let body = object_with(&published, |key| match key {
        "ok" => json!(true),
        _ => json!(format!("{key}-value")),
    });
    assert_eq!(request_cancel_result(true, &body.to_string()), Ok(()));
}

// ---- POST /v1/permissions/{id}/decision ----------------------------------------

#[test]
fn session_wire_permission_decision_body_is_inside_the_published_fields() {
    let contract = session_wire();
    assert_eq!(
        string(&contract, "/permission_decision/route"),
        "POST /v1/permissions/{id}/decision"
    );
    let anchor = "\"{}/v1/permissions/{permission_id}/decision\"";
    assert_calls("POST /v1/permissions/{id}/decision", anchor);
    assert_subset(
        "the decision POST body",
        production()
            .json_body_keys_after(anchor)
            .iter()
            .map(String::as_str),
        &strings(&contract, "/permission_decision/request_fields"),
    );
    let decisions = strings(&contract, "/permission_decision/decisions");
    let line = "let decision = if allow { \"allow\" } else { \"deny\" };";
    assert!(
        production().find_code(line).is_some(),
        "the decision values moved; re-point this check at `{line}`'s replacement"
    );
    assert_subset("the decision values", ["allow", "deny"], &decisions);
}

// ---- /v1/models and /v1/model -------------------------------------------------

#[test]
fn session_wire_models_and_model_set_are_inside_the_published_shape() {
    let contract = session_wire();
    assert_eq!(string(&contract, "/models/route"), "GET /v1/models");
    assert_eq!(string(&contract, "/model_set/route"), "POST /v1/model");
    assert_calls("GET /v1/models", "\"{}/v1/models\"");
    assert_subset(
        "fetch_models' ModelsResponse",
        production()
            .struct_fields("struct ModelsResponse")
            .iter()
            .map(String::as_str),
        &strings(&contract, "/models/response_fields"),
    );
    assert_subset(
        "fetch_models' Current",
        production()
            .struct_fields("struct Current")
            .iter()
            .map(String::as_str),
        &strings(&contract, "/models/current_fields"),
    );
    assert_subset(
        "ModelInfo",
        field_names::<ModelInfo>().iter().map(String::as_str),
        &strings(&contract, "/models/model_fields"),
    );
    assert_subset(
        "set_model's POST body",
        production()
            .json_body_keys_after("\"{}/v1/model\"")
            .iter()
            .map(String::as_str),
        &strings(&contract, "/model_set/request_fields"),
    );
}

// ---- GET /v1/events (legacy control stream) ------------------------------------

#[test]
fn session_wire_control_stream_permission_frames_are_published() {
    let contract = session_wire();
    assert_eq!(string(&contract, "/events/route"), "GET /v1/events");
    assert_calls("GET /v1/events", "\"{}/v1/events\"");
    let types = strings(&contract, "/events/event_types");
    let subscriptions = "for name in [\"permission_request\", \"permission_decision\"]";
    assert!(
        production().find_code(subscriptions).is_some(),
        "the control-stream subscriptions moved; re-point this check"
    );
    // Recorded reliance on something unpublished: gloo subscribes by SSE FRAME
    // name, and the legacy stream's frame names are NOT published (README "Not
    // published, on purpose" says no consumer reads them). They equal the
    // published `type` values today, which is all this can pin.
    assert_subset(
        "the /v1/events subscriptions",
        ["permission_request", "permission_decision"],
        &types,
    );
    assert_eq!(string(&contract, "/events/event_tag"), "type");
    // The envelope ids ControlEvent reads. Its `tool`/`reason`/`args` come from
    // the flattened OceanEvent payload, whose fields are unpublished.
    let envelope = strings(&contract, "/events/envelope_fields");
    assert_subset(
        "ControlEvent envelope ids",
        ["permission_id", "session_id", "request_id"],
        &envelope,
    );
    let frame = json!({
        "type": "permission_request",
        "permission_id": "perm", "session_id": "s", "request_id": "r",
        "tool": "bash", "reason": "why", "args": {},
    });
    assert!(matches!(
        serde_json::from_value::<ControlEvent>(frame),
        Ok(ControlEvent::PermissionRequest { .. })
    ));
    // Every other published type (a bare `{type}` carries no `tool`, which only
    // permission_request requires) decodes rather than failing the frame.
    for event_type in types.iter().filter(|t| *t != "permission_request") {
        assert!(
            serde_json::from_value::<ControlEvent>(json!({ "type": event_type })).is_ok(),
            "published control event `{event_type}` fails ControlEvent"
        );
    }
}

// ---- every daemon route daemon.rs calls ----------------------------------------

/// Daemon routes daemon.rs calls that no vendored contract publishes. Each is a
/// real reliance on an unpublished route; the list keeps it visible.
const KNOWN_UNPUBLISHED_ROUTES: &[&str] = &[
    "/v1/agent/history/search",
    "/v1/component/event",
    "/v1/fs/dirs",
    "/v1/fs/file",
    "/v1/permissions",
    "/v1/projects",
    "/v1/repo/github/{}/head-sha/{}/checks",
    "/v1/repo/github/{}/pulls",
    "/v1/repo/github/{}/pulls/{}",
    "/v1/repo/github/{}/pulls/{}/reviews",
    "/v1/requests",
];

#[test]
fn session_wire_every_daemon_route_daemon_rs_calls_is_published_or_recorded() {
    let published: BTreeSet<String> = published_routes()
        .into_iter()
        .map(|(_, shape)| shape)
        .collect();
    let known: BTreeSet<String> = KNOWN_UNPUBLISHED_ROUTES
        .iter()
        .map(|r| r.to_string())
        .collect();
    for entry in &known {
        assert!(
            !published.contains(entry),
            "`{entry}` is now published; drop it from KNOWN_UNPUBLISHED_ROUTES"
        );
    }
    let called: BTreeSet<String> = production()
        .v1_literals()
        .into_iter()
        .map(|(_, literal)| path_shape(&literal))
        .collect();
    assert!(
        called.contains("/v1/agent/turns"),
        "the scan found no calls"
    );
    let strays: Vec<&String> = called
        .iter()
        .filter(|shape| !published.contains(*shape) && !known.contains(*shape))
        .collect();
    assert!(
        strays.is_empty(),
        "daemon.rs calls {strays:?}, which no vendored contract publishes. Get it \
         published in ocean-os, or record it in KNOWN_UNPUBLISHED_ROUTES."
    );
    for entry in &known {
        assert!(
            called.contains(entry),
            "daemon.rs no longer calls `{entry}`; drop it from KNOWN_UNPUBLISHED_ROUTES"
        );
    }
}
