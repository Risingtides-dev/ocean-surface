//! The proxy relays daemon routes on the paths and verbs ocean-os publishes
//! (the session, voice and Observatory contracts vendored under
//! `crates/ocean-surface-ui/tests/fixtures/ocean-os-*-wire/`).
//!
//! Read off `build_app`, the REAL route table, and off the forwarding
//! handlers. The UI crate's `wire_contract_support.rs` is included by path so
//! both crates read the same vendored bytes through the same scanner.
//!
//! Only routes whose PATH some contract publishes are held here: every verb
//! the proxy registers on such a path must be published for it. A path no
//! vendored contract covers (rooms, projects, fs, repo, …) is not this test's
//! business — room-wire is #225's, and the rest are recorded as unpublished in
//! the UI's `KNOWN_UNPUBLISHED_ROUTES`.

#[path = "../../ocean-surface-ui/src/wire_contract_support.rs"]
mod support;

use std::collections::BTreeSet;
use support::*;

const MAIN_RS: &str = include_str!("main.rs");

/// Verbs the proxy registers on a published path that ocean-os does not
/// publish for it. `GET /v1/model` is a read the daemon serves but no contract
/// lists; `POST /v1/agents` is the agent builder's create (ocean-os
/// `feat/agent-crud`), published only as `GET`.
const KNOWN_UNPUBLISHED_VERBS: &[(&str, &str)] = &[("GET", "/v1/model"), ("POST", "/v1/agents")];

fn registered() -> Vec<(String, String)> {
    Source::production(MAIN_RS).axum_routes("fn build_app(")
}

#[test]
fn wire_contract_every_verb_on_a_published_path_is_published() {
    let published = published_routes();
    let published_paths: BTreeSet<&String> = published.iter().map(|(_, path)| path).collect();
    let known: BTreeSet<(String, String)> = KNOWN_UNPUBLISHED_VERBS
        .iter()
        .map(|(m, p)| (m.to_string(), path_shape(p)))
        .collect();
    for entry in &known {
        assert!(
            !published.contains(entry),
            "{entry:?} is now published; drop it from KNOWN_UNPUBLISHED_VERBS"
        );
    }
    let routes = registered();
    assert!(
        routes
            .iter()
            .any(|(m, p)| m == "POST" && p == "/v1/agent/turns"),
        "the route scan found nothing: {routes:?}"
    );
    let mut strays = Vec::new();
    for (method, path) in &routes {
        let shape = path_shape(path);
        if !published_paths.contains(&shape) {
            continue;
        }
        let route = (method.clone(), shape);
        if !published.contains(&route) && !known.contains(&route) {
            strays.push(format!("{method} {path}"));
        }
    }
    assert!(
        strays.is_empty(),
        "the proxy registers {strays:?}, verbs ocean-os does not publish on those paths"
    );
    for entry in &known {
        assert!(
            routes
                .iter()
                .any(|(m, p)| (m.clone(), path_shape(p)) == *entry),
            "the proxy no longer registers {entry:?}; drop it from KNOWN_UNPUBLISHED_VERBS"
        );
    }
}

#[test]
fn wire_contract_voice_and_observatory_relays_reach_published_routes() {
    let published = published_routes();
    let routes: BTreeSet<(String, String)> = registered().into_iter().collect();
    let has = |method: &str, path: &str| routes.contains(&(method.to_string(), path.to_string()));
    let is_published = |route: &str| {
        let (method, path) = split_route(route);
        published.contains(&(method, path_shape(&path)))
    };

    // Same-origin daemon relays the web/PWA surface depends on, registered on
    // the published path and verb.
    for route in [
        "POST /v1/voice/realtime/client-secret",
        "POST /v1/agent/sessions/{id}/messages",
        "GET /v1/observatory/snapshot",
        "GET /v1/observatory/events",
        "POST /v1/agent/turns",
        "GET /v1/agent/events",
        "POST /v1/agent/sessions",
        "GET /v1/agent/sessions",
        "GET /v1/events",
        "POST /v1/permissions/{id}/decision",
        "POST /v1/requests/{id}/cancel",
    ] {
        let (method, path) = split_route(route);
        assert!(has(&method, &path), "the proxy does not register {route}");
        assert!(is_published(route), "{route} is not published");
    }

    // The /api/{stt,tts} adapters forward to the published voice routes.
    let source = Source::production(MAIN_RS);
    for (adapter, upstream) in [
        ("/api/stt", "POST /v1/voice/stt"),
        ("/api/tts", "POST /v1/voice/tts"),
    ] {
        assert!(
            has("POST", adapter),
            "the proxy does not register POST {adapter}"
        );
        assert!(is_published(upstream), "{upstream}");
        let (_, path) = split_route(upstream);
        assert!(
            source.find_code(&format!("\"{{}}{path}\"")).is_some(),
            "{adapter} no longer forwards to {upstream}"
        );
    }

    // Observatory auth: the proxy presents the observer token with reqwest's
    // `bearer_auth`, i.e. the published `Bearer` scheme.
    assert_eq!(string(&observatory_wire(), "/auth_scheme"), "Bearer");
    assert!(source
        .body_after("async fn proxy_observatory(")
        .contains(".bearer_auth(token)"));
}
