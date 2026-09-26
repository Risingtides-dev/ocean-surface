//! Surface stays inside ocean-os's published voice wire
//! (`docs/contracts/voice-wire.json`, vendored at
//! `tests/fixtures/ocean-os-voice-wire/`).
//!
//! A child module of `daemon` (declared there with `#[path]`): the realtime
//! mint and handoff request types are private to `daemon`. The tool dispatch in
//! `voice/realtime.rs` runs inside wasm-only closures a host test cannot call,
//! so it is read off the source — scoped to the non-test half, so a test that
//! quotes a tool name cannot satisfy it.

use super::*;
use crate::voice::planner::VoicePlannerBrief;
use crate::voice::transport::{
    interpret_stt, voice_transport_url, SttBody, SttOutcome, VoiceRoute,
};
use crate::wire_contract_support::*;
use serde_json::json;
use std::collections::BTreeSet;

const DAEMON_RS: &str = include_str!("daemon.rs");
const REALTIME_RS: &str = include_str!("voice/realtime.rs");
const TTS_RS: &str = include_str!("tts.rs");

fn route_of(url: &str) -> String {
    path_shape(url)
}

fn published_voice_route(method_path: &str) -> bool {
    let contract = voice_wire();
    let mut routes = strings(&contract, "/routes");
    routes.insert(string(&contract, "/handoff_route"));
    routes.contains(method_path)
}

// ---- routes ------------------------------------------------------------------

#[test]
fn voice_wire_stt_and_tts_post_to_published_routes() {
    // Daemon-direct hosts (Tauri, extension) post straight to the daemon; the
    // web/PWA posts to the proxy's /api/{stt,tts}, whose forward the proxy's
    // own contract test pins.
    for (route, published) in [
        (VoiceRoute::Stt, "POST /v1/voice/stt"),
        (VoiceRoute::Tts, "POST /v1/voice/tts"),
    ] {
        let url = voice_transport_url("http://127.0.0.1:4780", route);
        assert_eq!(format!("POST {}", route_of(&url)), published);
        assert!(published_voice_route(published), "{published}");
    }
}

#[test]
fn voice_wire_mint_and_handoff_routes_are_published() {
    let daemon = Source::production(DAEMON_RS);
    assert!(published_voice_route(
        "POST /v1/voice/realtime/client-secret"
    ));
    assert!(daemon
        .find_code("\"{}/v1/voice/realtime/client-secret\"")
        .is_some());
    assert_eq!(
        string(&voice_wire(), "/handoff_route"),
        "POST /v1/agent/sessions/{id}/messages"
    );
    assert!(daemon
        .find_code("\"{}/v1/agent/sessions/{}/messages\"")
        .is_some());
}

// ---- realtime client-secret mint ----------------------------------------------

#[test]
fn voice_wire_conversation_mint_body_is_inside_the_published_fields() {
    let fields = strings(&voice_wire(), "/realtime/request_fields");
    let sent =
        Source::production(DAEMON_RS).json_body_keys_after("pub async fn realtime_client_secret(");
    assert_subset(
        "realtime_client_secret's body",
        sent.iter().map(String::as_str),
        &fields,
    );
    // No `purpose` rides it: the daemon's default must be the conversation.
    assert!(!sent.contains("purpose"));
    assert_eq!(
        string(&voice_wire(), "/realtime/default_purpose"),
        "conversation"
    );
}

#[test]
fn voice_wire_planner_mint_body_is_inside_the_published_fields() {
    let contract = voice_wire();
    let body = PlannerRealtimeSecretRequest {
        purpose: "planner",
        planner_context: PlannerRealtimeContextRequest {
            project_id: "p",
            workspace_root: "/w",
        },
    };
    let value = serde_json::to_value(&body).unwrap();
    assert_subset(
        "PlannerRealtimeSecretRequest",
        keys(&value).iter().map(String::as_str),
        &strings(&contract, "/realtime/request_fields"),
    );
    assert!(strings(&contract, "/realtime/purposes").contains(value["purpose"].as_str().unwrap()));
    // The daemon validates both context fields, so the set is exact.
    assert_eq!(
        keys(&value["planner_context"]),
        strings(&contract, "/realtime/planner_context_fields")
    );
    // And the call site sends the purpose this test serialized.
    assert!(Source::production(DAEMON_RS)
        .find_code("purpose: \"planner\"")
        .is_some());
}

#[test]
fn voice_wire_mint_response_decodes_from_the_published_keys() {
    let contract = voice_wire();
    let mut published = strings(&contract, "/realtime/response_keys");
    let workspace_key = string(&contract, "/realtime/conversation_workspace_key");
    published.insert(workspace_key.clone());
    assert_subset(
        "RealtimeSecret",
        field_names::<RealtimeSecret>().iter().map(String::as_str),
        &published,
    );
    let body = object_with(&published, |key| json!(format!("{key}-value")));
    let secret: RealtimeSecret = serde_json::from_value(body).unwrap();
    assert_eq!(secret.client_secret, "client_secret-value");
    assert_eq!(secret.model, "model-value");
    assert_eq!(
        secret.workspace_root.as_deref(),
        Some(format!("{workspace_key}-value").as_str()),
        "the conversation workspace root rides the published key"
    );
    // Planner and project-less answers omit it; that must still decode.
    let bare = object_with(&strings(&contract, "/realtime/response_keys"), |key| {
        json!(format!("{key}-value"))
    });
    assert!(serde_json::from_value::<RealtimeSecret>(bare)
        .unwrap()
        .workspace_root
        .is_none());
}

// ---- realtime tools -----------------------------------------------------------

#[test]
fn voice_wire_conversation_dispatch_names_exactly_the_published_tools() {
    let contract = voice_wire();
    let arms = Source::production(REALTIME_RS).match_arm_literals("fn dispatch_tool(");
    // A closed-set dispatch: equality with the richest conversation mint, and
    // the plain mint's tools are all among them.
    assert_eq!(
        arms,
        strings(&contract, "/realtime/tools/conversation_with_workspace"),
        "dispatch_tool's arms vs the published conversation tools"
    );
    let plain = strings(&contract, "/realtime/tools/conversation");
    assert!(plain.is_subset(&arms), "{plain:?} ⊄ {arms:?}");
    assert!(
        Source::production(REALTIME_RS)
            .body_after("fn dispatch_tool(")
            .contains("_ => send_tool_output"),
        "an unknown tool must still be answered"
    );
}

#[test]
fn voice_wire_planner_dispatch_names_exactly_the_published_tools() {
    let arms = Source::production(REALTIME_RS).match_arm_literals("fn dispatch_planner_tool(");
    assert_eq!(
        arms,
        strings(&voice_wire(), "/realtime/tools/planner"),
        "dispatch_planner_tool's arms vs the published planner tools"
    );
}

#[test]
fn voice_wire_tool_arguments_are_the_published_ones() {
    let contract = voice_wire();
    let realtime = Source::production(REALTIME_RS);
    // write_handoff reads `note`.
    assert_eq!(
        strings(&contract, "/realtime/tool_arguments/write_handoff"),
        BTreeSet::from(["note".to_string()])
    );
    assert!(realtime
        .body_after("fn dispatch_tool(")
        .contains("pointer(\"/note\")"));
    // Both workspace tools read `path`.
    for tool in ["list_workspace", "read_workspace_file"] {
        assert_eq!(
            strings(&contract, &format!("/realtime/tool_arguments/{tool}")),
            BTreeSet::from(["path".to_string()]),
            "{tool}"
        );
    }
    assert!(realtime
        .body_after("fn planner_relative_path_from_args(")
        .contains("pointer(\"/path\")"));
    // propose_handoff: the brief is deny_unknown_fields with every field
    // required, so its field set must EQUAL the published arguments.
    assert_eq!(
        field_names::<VoicePlannerBrief>(),
        strings(&contract, "/realtime/tool_arguments/propose_handoff")
    );
    let args = object_with(
        &strings(&contract, "/realtime/tool_arguments/propose_handoff"),
        |key| match key {
            "title" | "problem" => json!(format!("{key}-value")),
            _ => json!([]),
        },
    );
    assert!(VoicePlannerBrief::from_tool_arguments(&args.to_string()).is_ok());
}

// ---- handoff append -----------------------------------------------------------

#[test]
fn voice_wire_handoff_bodies_are_inside_the_published_fields() {
    let contract = voice_wire();
    let fields = strings(&contract, "/handoff/request_fields");
    let roles = strings(&contract, "/handoff/roles");
    let kinds = strings(&contract, "/handoff/kinds");

    // The planner's typed append.
    let planner = serde_json::to_value(PlannerHandoffRequest {
        role: "user",
        kind: "planner_handoff",
        content: "# PRD",
    })
    .unwrap();
    assert_subset(
        "PlannerHandoffRequest",
        keys(&planner).iter().map(String::as_str),
        &fields,
    );
    assert!(Source::production(DAEMON_RS)
        .find_code("kind: \"planner_handoff\"")
        .is_some());
    assert!(roles.contains(planner["role"].as_str().unwrap()));
    assert!(kinds.contains(planner["kind"].as_str().unwrap()));

    // The realtime agent's `write_handoff` append.
    let daemon = Source::production(DAEMON_RS);
    let append = "pub async fn append_session_message(";
    assert_subset(
        "append_session_message's body",
        daemon
            .json_body_keys_after(append)
            .iter()
            .map(String::as_str),
        &fields,
    );
    assert!(daemon.body_after(append).contains("\"role\": \"user\""));
    assert!(Source::production(REALTIME_RS)
        .body_after("fn dispatch_tool(")
        .contains("Some(\"handoff\")"));
    assert!(roles.contains("user"));
    assert!(kinds.contains("handoff"));
}

// ---- STT / TTS -----------------------------------------------------------------

#[test]
fn voice_wire_stt_answer_decodes_from_the_published_keys() {
    let contract = voice_wire();
    let mut published = strings(&contract, "/stt/response_keys");
    published.insert(string(&contract, "/error_key"));
    // `ok` is the PROXY's own `/api/stt` envelope ({ok, text} / {ok:false,
    // error}), not a daemon key; the proxy lives in this repo.
    let mut allowed = published.clone();
    allowed.insert("ok".into());
    assert_subset(
        "SttBody",
        field_names::<SttBody>().iter().map(String::as_str),
        &allowed,
    );
    // A daemon-direct success is exactly `{text}`.
    let ok: SttBody = serde_json::from_value(json!({ "text": " heard " })).unwrap();
    assert!(matches!(interpret_stt(true, &ok), SttOutcome::Transcript(t) if t == "heard"));
    // A daemon-direct failure is `{error}` on a non-2xx.
    let err: SttBody =
        serde_json::from_value(json!({ string(&contract, "/error_key"): "no key" })).unwrap();
    assert!(matches!(interpret_stt(false, &err), SttOutcome::Report(r) if r == "no key"));
}

#[test]
fn voice_wire_tts_body_is_inside_the_published_fields() {
    let sent = Source::production(TTS_RS).struct_fields("struct TtsRequest");
    assert!(sent.contains("text"));
    assert_subset(
        "TtsRequest",
        sent.iter().map(String::as_str),
        &strings(&voice_wire(), "/tts/request_fields"),
    );
}

#[test]
fn voice_wire_voice_client_type_is_the_published_one() {
    assert_eq!(
        VOICE_CLIENT_TYPE,
        string(&voice_wire(), "/agent_voice/client_type")
    );
}
