//! Three proxy-boundary holes, pinned against the PRODUCTION router.
//!
//! H1 — path normalisation. Every guard used to reason about the client's
//! path split on `/`, then `reqwest` parsed `format!("{base}{path}")` with the
//! WHATWG rules, which read `\` as `/` and collapse dot segments. hyper
//! accepts a raw `\` in the request target, so
//! `DELETE .../agents/x\..\..\close` looked like one harmless agent id to the
//! allowlist and arrived upstream as `DELETE .../close` carrying the operator
//! key. Two independent halves now hold it: `has_dot_segment` refuses `\` and
//! `%5C`, and `upstream_url` refuses any URL whose parsed path differs from
//! the one the proxy decided to forward. Captured `{id}` segments are
//! re-encoded so a decoded `%2F` cannot become a second segment.
//!
//! M-A — actor binding. With a roster user signed in, every identity the
//! daemon's rooms routes read (`?actor_id=` and `?uploader_id=` on every
//! method; `author_id`, `invoked_by`, `requested_by`, `owner_member_id`,
//! `owner_id`, a non-agent join's `id` in bodies) must be that user, or the
//! proxy answers `403 actor_mismatch` before the daemon sees it. A body that
//! repeats one of those keys (or `id`/`kind`) is `400 duplicate_identity_field`.
//!
//! M-B — auth-off CSRF. Every non-GET/HEAD request under `/v1/` and `/api/`
//! now meets the Origin/Referer policy the six authority routes had alone,
//! and every auth-off request must name a loopback Host (DNS rebinding).
//!
//! The upstream here is a recorder that answers 200 to ANY request and keeps
//! what it received, so "refused" is proven by an empty log rather than by
//! the absence of a route that would have 404'd anyway.

use super::*;
use crate::upstream_url;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const OPERATOR_KEY: &str = "proxy-owned-authority";

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    /// Path and query exactly as the upstream's HTTP server received them.
    target: String,
    operator: Option<String>,
}

type Log = Arc<Mutex<Vec<Seen>>>;

async fn spawn_recorder() -> (String, Log, tokio::task::JoinHandle<()>) {
    async fn record(
        axum::extract::State(log): axum::extract::State<Log>,
        req: Request<Body>,
    ) -> Json<Value> {
        log.lock().unwrap().push(Seen {
            method: req.method().to_string(),
            target: req
                .uri()
                .path_and_query()
                .map(|pq| pq.as_str().to_string())
                .unwrap_or_default(),
            operator: req
                .headers()
                .get("x-ocean-operator")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
        });
        Json(json!({ "ok": true }))
    }
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new().fallback(record).with_state(log.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind recorder");
    let addr = listener.local_addr().expect("recorder addr");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), log, handle)
}

fn seen(log: &Log) -> Vec<Seen> {
    log.lock().unwrap().clone()
}

fn write_operator_key(dir: &std::path::Path) -> PathBuf {
    let key_path = dir.join("operator.key");
    std::fs::write(&key_path, format!("{OPERATOR_KEY}\n")).expect("write operator key");
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
        .expect("chmod operator key");
    key_path
}

/// Auth-off (loopback development) state pointed at `daemon_url`, holding a
/// readable operator key so a slip would visibly carry it upstream.
fn auth_off_state(daemon_url: &str, key_path: PathBuf) -> Arc<AppState> {
    let mut state = auth_test_state();
    let inner = Arc::get_mut(&mut state).expect("sole state owner");
    inner.daemon_url = daemon_url.to_string();
    inner.basic_auth = None;
    inner.operator_key_path = key_path;
    state
}

/// Multi-user, auth-on: `alice` (tok-alice) and `bob` (tok-bob) both on the
/// recorder, so a request's only distinguishing mark is whose cookie it has.
fn roster_state(daemon_url: &str) -> Arc<AppState> {
    let mut state = auth_test_state();
    let inner = Arc::get_mut(&mut state).expect("sole state owner");
    inner.daemon_url = daemon_url.to_string();
    inner.users = vec![
        user("alice", "pw-a", daemon_url, "tok-alice"),
        user("bob", "pw-b", daemon_url, "tok-bob"),
    ];
    state
}

fn app_for(state: Arc<AppState>) -> (Router, tempfile::TempDir) {
    let dist = tempfile::tempdir().expect("dist tempdir");
    (build_app(state, dist.path()), dist)
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, String) {
    let response = app.clone().oneshot(req).await.expect("response");
    let status = response.status();
    (status, body_text(response).await)
}

fn bare(method: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .expect("request")
}

fn error_code(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value["error"].as_str().map(str::to_owned))
        .unwrap_or_else(|| body.to_string())
}

// ── H1: upstream_url, the parse-and-compare half ─────────────────────────

fn daemon_at(url: &str) -> ResolvedDaemon {
    ResolvedDaemon {
        device: "test".to_string(),
        url: url.to_string(),
        observer_token_path: None,
        operator_key_path: None,
        selection_key: None,
    }
}

#[test]
fn upstream_url_refuses_every_path_the_parser_would_rewrite() {
    let daemon = daemon_at("http://127.0.0.1:4780");
    for path in [
        "/v1/rooms/persistent/team/agents/x\\..\\..\\close",
        "/v1/rooms/persistent/team/agents/x\\..\\..\\participants\\bob",
        "/v1/rooms/persistent/team\\close",
        "/v1/rooms/persistent/../../v1/agent/turns",
        "/v1/rooms/persistent/%2e%2e/v1/agent/turns",
        "/v1/rooms/persistent/./events",
        "/v1/projects/a\tb",
        "/v1/projects/a\nb",
    ] {
        assert!(
            upstream_url(&daemon, path, None).is_none(),
            "{path:?} must be refused: the parser would send a different path"
        );
    }
}

#[test]
fn upstream_url_keeps_a_legitimate_path_and_query_byte_for_byte() {
    let daemon = daemon_at("http://127.0.0.1:4780/");
    let url = upstream_url(
        &daemon,
        "/v1/rooms/persistent/room.v2/transcript",
        Some("after_seq=4&actor_id=ali%20ce"),
    )
    .expect("legitimate path");
    assert_eq!(url.path(), "/v1/rooms/persistent/room.v2/transcript");
    assert_eq!(url.query(), Some("after_seq=4&actor_id=ali%20ce"));
    // `%5C` is not rewritten by the parser, so THIS half lets it through —
    // `has_dot_segment` is the half that refuses it (tested below).
    assert!(upstream_url(&daemon, "/v1/rooms/persistent/a%5Cb", None).is_some());

    // A daemon mounted under a path prefix keeps that prefix in the check.
    let prefixed = daemon_at("http://127.0.0.1:4780/ocean");
    let url = upstream_url(&prefixed, "/v1/models", None).expect("prefixed");
    assert_eq!(url.path(), "/ocean/v1/models");
    assert!(upstream_url(&prefixed, "/v1/../../models", None).is_none());
}

// ── H1: has_dot_segment, the string half ─────────────────────────────────

#[test]
fn the_dot_guard_refuses_backslashes_raw_and_encoded_in_either_case() {
    for path in [
        "/v1/rooms/persistent/team/agents/x\\..\\..\\close",
        "/v1/rooms/persistent/team/agents/x%5C..%5C..%5Cclose",
        "/v1/rooms/persistent/team/agents/x%5c..%5c..%5cclose",
        "/v1/rooms/persistent/team/agents/x\\..%5C..%5cclose",
        "/v1/rooms/persistent/a%5Cb",
        "\\",
    ] {
        assert!(has_dot_segment(path), "{path:?} must be refused");
    }
    // Double-encoded is one decode away from `%5C` text, which nothing
    // downstream decodes again — same single-pass rule as `%252e`.
    assert!(!has_dot_segment("/v1/rooms/persistent/a%255Cb"));
    assert!(!has_dot_segment("/v1/rooms/persistent/room.v2/events"));
}

// ── H1: through the production router ────────────────────────────────────

/// The probe shapes the review confirmed, and their encoded/mixed siblings,
/// for methods that WOULD get the operator key (keyed) and methods that would
/// not (unkeyed). Every one must die at the proxy with the dot guard's answer.
const BACKSLASH_PROBES: &[(&str, &str)] = &[
    // keyed: the exact authority shapes the allowlist approves by segment
    (
        "DELETE",
        "/v1/rooms/persistent/team/agents/x\\..\\..\\close",
    ),
    (
        "DELETE",
        "/v1/rooms/persistent/team/agents/x%5C..%5C..%5Cclose",
    ),
    (
        "DELETE",
        "/v1/rooms/persistent/team/agents/x%5c..\\..%5Cclose",
    ),
    (
        "DELETE",
        "/v1/rooms/persistent/team/agents/x\\..\\..\\participants\\bob",
    ),
    (
        "DELETE",
        "/v1/rooms/persistent/team/agents/x%5C..%5C..%5Cparticipants%5Cbob",
    ),
    (
        "POST",
        "/v1/rooms/persistent/team/agents/x\\..\\..\\close/reauthorize",
    ),
    (
        "POST",
        "/v1/rooms/persistent/team/agents/x%5c..%5c..%5cclose/reauthorize",
    ),
    // unkeyed: member-lane writes and reads
    ("POST", "/v1/rooms/persistent/team/messages\\..\\close"),
    ("POST", "/v1/rooms/persistent/team/messages%5C..%5Cclose"),
    (
        "GET",
        "/v1/rooms/persistent/team\\..\\..\\..\\agent\\sessions",
    ),
    (
        "GET",
        "/v1/rooms/persistent/team%5C..%5C..%5C..%5Cagent%5Csessions",
    ),
    ("GET", "/v1/rooms/persistent/team\\events"),
    // other families forwarding a client path
    ("POST", "/v1/longhouse/demo\\..\\..\\agent\\turns"),
    ("POST", "/v1/longhouse/demo%5C..%5C..%5Cagent%5Cturns"),
    ("GET", "/v1/agents/x%5C..%5C..%5Cmodels"),
    ("PUT", "/v1/agents/x\\..\\..\\models"),
    ("GET", "/v1/projects/x%5C..%5Cmodels"),
    ("PATCH", "/v1/projects/x\\..\\..\\model"),
    ("DELETE", "/v1/projects/x%5c..%5c..%5cagents%5cx"),
    ("GET", "/v1/sessions/x%5C..%5C..%5Cmodels"),
    ("GET", "/v1/agent/sessions/x\\..\\..\\..\\models"),
    ("POST", "/v1/agent/sessions/x%5C..%5C..%5Cturns/messages"),
    ("POST", "/v1/permissions/x\\..\\..\\model/decision"),
    ("POST", "/v1/requests/x%5C..%5C..%5Cmodel/cancel"),
    ("POST", "/v1/rooms/x%5C..%5C..%5Cmodel/livekit-token"),
];

#[tokio::test]
async fn backslash_probes_die_at_the_proxy_over_oneshot() {
    let (daemon_url, log, upstream) = spawn_recorder().await;
    let credential_dir = tempfile::tempdir().expect("credential tempdir");
    let key_path = write_operator_key(credential_dir.path());
    let (app, _dist) = app_for(auth_off_state(&daemon_url, key_path));

    for (method, uri) in BACKSLASH_PROBES {
        let (status, body) = send(&app, bare(method, uri)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri}");
        assert_eq!(
            body, "invalid path",
            "{method} {uri} — the dot guard's answer"
        );
    }
    assert!(
        seen(&log).is_empty(),
        "nothing may reach the daemon: {:?}",
        seen(&log)
    );
    upstream.abort();
}

/// Write one raw HTTP/1.1 request and return (status, body). Raw because the
/// point is a request target no client library would send: hyper on the
/// proxy's side accepts a bare `\`, which is how the probe got in.
async fn raw_http(addr: std::net::SocketAddr, method: &str, target: &str) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connect proxy");
    let request = format!(
        "{method} {target} HTTP/1.1\r\nHost: {addr}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read response");
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    (status, body)
}

async fn serve(app: Router) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind proxy");
    let addr = listener.local_addr().expect("proxy addr");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (addr, handle)
}

#[tokio::test]
async fn backslash_probes_die_at_the_proxy_over_real_tcp() {
    let (daemon_url, log, upstream) = spawn_recorder().await;
    let credential_dir = tempfile::tempdir().expect("credential tempdir");
    let key_path = write_operator_key(credential_dir.path());
    let (app, _dist) = app_for(auth_off_state(&daemon_url, key_path));
    let (addr, proxy) = serve(app).await;

    // Control first: the raw transport really does forward, and the path
    // arrives unchanged — so the refusals below are the guard, not a broken
    // harness.
    let (status, _) = raw_http(addr, "DELETE", "/v1/rooms/persistent/team/agents/member-1").await;
    assert_eq!(status, 200);
    let control = seen(&log);
    assert_eq!(control.len(), 1);
    assert_eq!(
        control[0].target,
        "/v1/rooms/persistent/team/agents/member-1"
    );
    assert_eq!(control[0].operator.as_deref(), Some(OPERATOR_KEY));

    for (method, target) in BACKSLASH_PROBES {
        let (status, body) = raw_http(addr, method, target).await;
        // hyper routes a raw `\` to the handler rather than rejecting it —
        // which is the whole hazard — so the refusal must be the proxy's own.
        assert_eq!(status, 400, "{method} {target}");
        assert_eq!(body, "invalid path", "{method} {target}");
    }
    let after = seen(&log);
    assert_eq!(
        after.len(),
        1,
        "only the control may reach the daemon: {after:?}"
    );
    proxy.abort();
    upstream.abort();
}

#[tokio::test]
async fn the_upstream_only_ever_receives_the_path_the_proxy_approved() {
    let (daemon_url, log, upstream) = spawn_recorder().await;
    let credential_dir = tempfile::tempdir().expect("credential tempdir");
    let key_path = write_operator_key(credential_dir.path());
    let (app, _dist) = app_for(auth_off_state(&daemon_url, key_path));
    let (addr, proxy) = serve(app.clone()).await;

    // (method, what the client sends, what the daemon must receive, keyed?)
    let cases: &[(&str, &str, &str, bool)] = &[
        (
            "POST",
            "/v1/rooms/persistent/team/agents/bootstrap",
            "/v1/rooms/persistent/team/agents/bootstrap",
            true,
        ),
        (
            "POST",
            "/v1/rooms/persistent/team/close?actor_id=surface-operator",
            "/v1/rooms/persistent/team/close?actor_id=surface-operator",
            false,
        ),
        (
            "GET",
            "/v1/rooms/persistent/room.v2/transcript?after_seq=3",
            "/v1/rooms/persistent/room.v2/transcript?after_seq=3",
            false,
        ),
        // A captured `{id}` is re-encoded into ONE segment: axum decoded the
        // `%2F`, and formatting it raw used to send `/v1/projects/a/b`.
        ("PATCH", "/v1/projects/a%2Fb", "/v1/projects/a%2Fb", false),
        ("GET", "/v1/sessions/a%2Fb", "/v1/sessions/a%2Fb", false),
        (
            "POST",
            "/v1/permissions/p%3Fx%3D1/decision",
            "/v1/permissions/p%3Fx%3D1/decision",
            false,
        ),
        // Longhouse forwards its raw path: `%3F` stays in the path instead of
        // becoming the start of a query.
        (
            "POST",
            "/v1/longhouse/demo%3Fforged=1",
            "/v1/longhouse/demo%3Fforged=1",
            false,
        ),
    ];
    for transport in ["oneshot", "tcp"] {
        for (method, sent, received, keyed) in cases {
            let before = seen(&log).len();
            let status = if transport == "tcp" {
                raw_http(addr, method, sent).await.0
            } else {
                send(&app, bare(method, sent)).await.0.as_u16()
            };
            assert_eq!(status, 200, "{transport} {method} {sent}");
            let log_now = seen(&log);
            assert_eq!(log_now.len(), before + 1, "{transport} {method} {sent}");
            let got = &log_now[before];
            assert_eq!(&got.method, method, "{transport} {sent}");
            assert_eq!(&got.target, received, "{transport} {method} {sent}");
            assert_eq!(
                got.operator.is_some(),
                *keyed,
                "{transport} {method} {sent}: operator key only on authority shapes"
            );
        }
    }
    proxy.abort();
    upstream.abort();
}

#[tokio::test]
async fn a_captured_dot_segment_is_refused_on_every_id_family() {
    let (daemon_url, log, upstream) = spawn_recorder().await;
    let (app, _dist) = app_for(auth_off_state(&daemon_url, PathBuf::from("/not-used")));
    for (method, uri) in [
        ("GET", "/v1/sessions/%2e%2e"),
        // decoded by axum to `a/../models`: the guard sees the `..` inside
        ("GET", "/v1/sessions/a%2F..%2Fmodels"),
        ("GET", "/v1/agent/sessions/.."),
        ("GET", "/v1/projects/%2E%2E"),
        ("PATCH", "/v1/projects/.."),
        ("DELETE", "/v1/projects/%2e%2e"),
        ("POST", "/v1/agent/sessions/%2e%2e/messages"),
        ("POST", "/v1/permissions/../decision"),
        ("POST", "/v1/requests/%2e%2e/cancel"),
        ("POST", "/v1/rooms/%2e%2e/livekit-token"),
    ] {
        let (status, body) = send(&app, bare(method, uri)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri}");
        assert_eq!(body, "invalid path", "{method} {uri}");
    }
    assert!(seen(&log).is_empty(), "{:?}", seen(&log));
    upstream.abort();
}

// ── M-A: the member lane's identity is the session's user ────────────────

fn as_alice(method: &str, uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, format!("{SESSION_COOKIE}=tok-alice"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request")
}

/// Every mutating member-lane route, once naming bob (refused) and once
/// naming alice (forwarded) — the pair proves the refusal is the identity and
/// not the route.
const MEMBER_LANE: &[(&str, &str, &str, &str, &str)] = &[
    // (method, uri as bob, body as bob, uri as alice, body as alice)
    (
        "POST",
        "/v1/rooms/persistent/team/close?actor_id=bob",
        "",
        "/v1/rooms/persistent/team/close?actor_id=alice",
        "",
    ),
    (
        "POST",
        "/v1/rooms/persistent/team/messages",
        r#"{"author_id":"bob","body":"hi"}"#,
        "/v1/rooms/persistent/team/messages",
        r#"{"author_id":"alice","body":"hi"}"#,
    ),
    (
        "POST",
        "/v1/rooms/persistent/team/participants",
        r#"{"id":"bob","display_name":"Bob","kind":"human"}"#,
        "/v1/rooms/persistent/team/participants",
        r#"{"id":"alice","display_name":"Alice","kind":"human"}"#,
    ),
    (
        "POST",
        "/v1/rooms/persistent/team/participants",
        r#"{"id":"bob","display_name":"Bob"}"#,
        "/v1/rooms/persistent/team/participants",
        r#"{"id":"alice","display_name":"Alice"}"#,
    ),
    (
        "POST",
        "/v1/rooms/persistent/team/participants",
        r#"{"id":"bob","display_name":"Bob","kind":"bot"}"#,
        "/v1/rooms/persistent/team/participants",
        r#"{"id":"alice","display_name":"Alice","kind":"bot"}"#,
    ),
    (
        "POST",
        "/v1/rooms/persistent/team/artifacts",
        r#"{"id":"plan","kind":"task","title":"t","body":"b","author_id":"bob"}"#,
        "/v1/rooms/persistent/team/artifacts",
        r#"{"id":"plan","kind":"task","title":"t","body":"b","author_id":"alice"}"#,
    ),
    (
        "POST",
        "/v1/rooms/persistent/team/artifacts/plan/amend",
        r#"{"expected_version":1,"author_id":"bob"}"#,
        "/v1/rooms/persistent/team/artifacts/plan/amend",
        r#"{"expected_version":1,"author_id":"alice"}"#,
    ),
    (
        "POST",
        "/v1/rooms/persistent/team/agents/helper/invoke",
        r#"{"invoked_by":"bob","message_seq":3}"#,
        "/v1/rooms/persistent/team/agents/helper/invoke",
        r#"{"invoked_by":"alice","message_seq":3}"#,
    ),
    (
        "POST",
        "/v1/rooms/persistent/team/summarize",
        r#"{"requested_by":"bob"}"#,
        "/v1/rooms/persistent/team/summarize",
        r#"{"requested_by":"alice"}"#,
    ),
    (
        "POST",
        "/v1/rooms/persistent/team/attachments?uploader_id=bob&filename=a.txt",
        "raw bytes, not json",
        "/v1/rooms/persistent/team/attachments?uploader_id=alice&filename=a.txt",
        "raw bytes, not json",
    ),
    (
        "DELETE",
        "/v1/rooms/persistent/team/attachments/att-1?actor_id=bob",
        "",
        "/v1/rooms/persistent/team/attachments/att-1?actor_id=alice",
        "",
    ),
    (
        "POST",
        "/v1/rooms/persistent/team/workspace/exec?actor_id=bob",
        r#"{"command":"ls"}"#,
        "/v1/rooms/persistent/team/workspace/exec?actor_id=alice",
        r#"{"command":"ls"}"#,
    ),
];

#[tokio::test]
async fn a_signed_in_user_cannot_act_as_another_roster_member() {
    let (daemon_url, log, upstream) = spawn_recorder().await;
    let (app, _dist) = app_for(roster_state(&daemon_url));

    for (method, bob_uri, bob_body, _, _) in MEMBER_LANE {
        let (status, body) = send(&app, as_alice(method, bob_uri, bob_body)).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {bob_uri} {bob_body}"
        );
        assert_eq!(error_code(&body), "actor_mismatch", "{method} {bob_uri}");
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap()["code"],
            "actor_mismatch"
        );
        assert!(
            seen(&log).is_empty(),
            "{method} {bob_uri} reached the daemon"
        );
    }
    for (method, _, _, alice_uri, alice_body) in MEMBER_LANE {
        let before = seen(&log).len();
        let (status, _) = send(&app, as_alice(method, alice_uri, alice_body)).await;
        assert_eq!(status, StatusCode::OK, "{method} {alice_uri} {alice_body}");
        assert_eq!(seen(&log).len(), before + 1, "{method} {alice_uri}");
        assert_eq!(&seen(&log)[before].target, alice_uri);
    }
    upstream.abort();
}

#[tokio::test]
async fn the_actor_check_cannot_be_smuggled_past() {
    let (daemon_url, log, upstream) = spawn_recorder().await;
    let (app, _dist) = app_for(roster_state(&daemon_url));

    for (uri, body) in [
        // a duplicate key: one match does not excuse the other
        (
            "/v1/rooms/persistent/team/close?actor_id=alice&actor_id=bob",
            "",
        ),
        (
            "/v1/rooms/persistent/team/close?actor_id=bob&actor_id=alice",
            "",
        ),
        // an encoded key decodes to the same name the daemon reads
        ("/v1/rooms/persistent/team/close?actor%5Fid=bob", ""),
        // an encoded value, and `+` as the daemon decodes it
        ("/v1/rooms/persistent/team/close?actor_id=%62ob", ""),
        ("/v1/rooms/persistent/team/close?actor_id=alice+", ""),
        // a JSON escape decodes to the same string serde gives the daemon
        (
            "/v1/rooms/persistent/team/messages",
            r#"{"author_id":"\u0062ob","body":"hi"}"#,
        ),
        // ...in the KEY too: `author\u005fid` is `author_id` to serde
        (
            "/v1/rooms/persistent/team/messages",
            r#"{"author\u005fid":"bob","body":"hi"}"#,
        ),
        // a non-string identity is not this user
        ("/v1/rooms/persistent/team/messages", r#"{"author_id":7}"#),
        (
            "/v1/rooms/persistent/team/messages",
            r#"{"author_id":null}"#,
        ),
        // a body this parser cannot read is a body it cannot vouch for
        ("/v1/rooms/persistent/team/messages", r#"author_id=bob"#),
        ("/v1/rooms/persistent/team/messages", r#"["bob"]"#),
    ] {
        let (status, text) = send(&app, as_alice("POST", uri, body)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri} {body}");
        assert_eq!(error_code(&text), "actor_mismatch", "{uri} {body}");
    }
    assert!(seen(&log).is_empty(), "{:?}", seen(&log));

    // An agent join's `id` is a folder, not a person — but its `owner_id` IS a
    // person, and naming someone else writes them into `room_agent_owners`.
    let (status, text) = send(
        &app,
        as_alice(
            "POST",
            "/v1/rooms/persistent/team/participants",
            r#"{"id":"helper","display_name":"Helper","kind":"agent","owner_id":"bob"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&text), "actor_mismatch");
    assert!(seen(&log).is_empty(), "{:?}", seen(&log));

    // Not identities: an artifact's own `id`, an agent join's `id` (owned by
    // the caller, or unowned), and an authorize body's `agent_member_id`.
    for (uri, body) in [
        (
            "/v1/rooms/persistent/team/artifacts",
            r#"{"id":"bob","kind":"task","title":"t","body":"b","author_id":"alice"}"#,
        ),
        (
            "/v1/rooms/persistent/team/participants",
            r#"{"id":"helper","display_name":"Helper","kind":"agent","owner_id":"alice"}"#,
        ),
        (
            "/v1/rooms/persistent/team/participants",
            r#"{"id":"helper","display_name":"Helper","kind":"agent"}"#,
        ),
    ] {
        let (status, _) = send(&app, as_alice("POST", uri, body)).await;
        assert_eq!(status, StatusCode::OK, "{uri} {body}");
    }

    upstream.abort();
}

#[tokio::test]
async fn reads_that_gate_on_actor_id_are_bound_too() {
    let (daemon_url, log, upstream) = spawn_recorder().await;
    let (app, _dist) = app_for(roster_state(&daemon_url));

    // The daemon's workspace lane uses `?actor_id=` as its membership gate
    // for reads as well as commands, so a read as bob is bob's read.
    for uri in [
        "/v1/rooms/persistent/team/workspace?actor_id=bob",
        "/v1/rooms/persistent/team/workspace/files/read?actor_id=bob&path=a.txt",
        "/v1/rooms/persistent/team/workspace/files/list?path=.&actor_id=bob",
        "/v1/rooms/persistent/team/workspace/files/read?actor_id=alice&actor_id=bob",
    ] {
        let (status, text) = send(&app, as_alice("GET", uri, "")).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
        assert_eq!(error_code(&text), "actor_mismatch", "{uri}");
    }
    assert!(seen(&log).is_empty(), "{:?}", seen(&log));

    // Alice's own reads, and reads that carry no identity at all, still read.
    for uri in [
        "/v1/rooms/persistent/team/workspace?actor_id=alice",
        "/v1/rooms/persistent/team/workspace/files/read?actor_id=alice&path=a.txt",
        "/v1/rooms/persistent/team/transcript?after_seq=3",
    ] {
        let (status, _) = send(&app, as_alice("GET", uri, "")).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
    }
    assert_eq!(seen(&log).len(), 3);
    upstream.abort();
}

#[tokio::test]
async fn the_authority_routes_owner_is_the_session_user() {
    let (daemon_url, log, upstream) = spawn_recorder().await;
    let credential_dir = tempfile::tempdir().expect("credential tempdir");
    let key_path = write_operator_key(credential_dir.path());
    let mut state = roster_state(&daemon_url);
    Arc::get_mut(&mut state).unwrap().operator_key_path = key_path;
    let (app, _dist) = app_for(state);

    // Probe-confirmed: bootstrap naming bob was forwarded WITH the key, and
    // the first bootstrap writes whoever it names as the room's local owner.
    for (uri, body) in [
        (
            "/v1/rooms/persistent/team/agents/bootstrap",
            r#"{"owner_member_id":"bob","agent_package_id":"researcher"}"#,
        ),
        (
            "/v1/rooms/persistent/team/agents",
            r#"{"agent_member_id":"researcher","agent_package_id":"researcher","owner_member_id":"bob","decision_id":"d1"}"#,
        ),
    ] {
        let (status, text) = send(&app, as_alice("POST", uri, body)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
        assert_eq!(error_code(&text), "actor_mismatch", "{uri}");
    }
    assert!(seen(&log).is_empty(), "{:?}", seen(&log));

    for (uri, body) in [
        (
            "/v1/rooms/persistent/team/agents/bootstrap",
            r#"{"owner_member_id":"alice","agent_package_id":"researcher"}"#,
        ),
        (
            "/v1/rooms/persistent/team/agents",
            r#"{"agent_member_id":"researcher","agent_package_id":"researcher","owner_member_id":"alice","decision_id":"d1"}"#,
        ),
        // The status-change bodies carry a decision id and no identity.
        (
            "/v1/rooms/persistent/team/agents/researcher/suspend",
            r#"{"decision_id":"d2"}"#,
        ),
    ] {
        let before = seen(&log).len();
        let (status, _) = send(&app, as_alice("POST", uri, body)).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        let got = &seen(&log)[before];
        assert_eq!(got.operator.as_deref(), Some(OPERATOR_KEY), "{uri}");
    }
    upstream.abort();
}

#[tokio::test]
async fn a_bound_key_sent_twice_is_refused_whichever_copy_matches() {
    let (daemon_url, log, upstream) = spawn_recorder().await;
    let (app, _dist) = app_for(roster_state(&daemon_url));

    for (uri, body) in [
        (
            "/v1/rooms/persistent/team/messages",
            r#"{"author_id":"alice","author_id":"bob","body":"hi"}"#,
        ),
        (
            "/v1/rooms/persistent/team/messages",
            r#"{"author_id":"bob","author_id":"alice","body":"hi"}"#,
        ),
        // the same key, spelled once plainly and once escaped
        (
            "/v1/rooms/persistent/team/messages",
            r#"{"author_id":"alice","author\u005fid":"alice","body":"hi"}"#,
        ),
        (
            "/v1/rooms/persistent/team/agents/bootstrap",
            r#"{"owner_member_id":"alice","owner_member_id":"alice","agent_package_id":"r"}"#,
        ),
        (
            "/v1/rooms/persistent/team/participants",
            r#"{"id":"alice","id":"bob","display_name":"A"}"#,
        ),
        // `kind` decides whether a join's `id` is bound
        (
            "/v1/rooms/persistent/team/participants",
            r#"{"id":"bob","kind":"agent","kind":"human","display_name":"B"}"#,
        ),
    ] {
        let (status, text) = send(&app, as_alice("POST", uri, body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(error_code(&text), "duplicate_identity_field", "{body}");
    }
    assert!(seen(&log).is_empty(), "{:?}", seen(&log));

    // A repeated key that is not an identity is the daemon's to judge.
    let (status, _) = send(
        &app,
        as_alice(
            "POST",
            "/v1/rooms/persistent/team/messages",
            r#"{"author_id":"alice","body":"a","body":"b"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    upstream.abort();
}

#[tokio::test]
async fn auth_off_refuses_a_rebound_host_on_every_request() {
    let (daemon_url, log, upstream) = spawn_recorder().await;
    let (app, _dist) = app_for(auth_off_state(&daemon_url, PathBuf::from("/not-used")));

    // A page that rebound its own name to 127.0.0.1 is same-origin with
    // itself, so no Origin check sees it; its Host header gives it away.
    for (method, uri) in [
        ("GET", "/v1/rooms/persistent"),
        ("GET", "/v1/rooms/persistent/team/transcript"),
        ("GET", "/v1/agent/sessions"),
        ("GET", "/api/config"),
        ("GET", "/"),
        ("POST", "/v1/agent/turns"),
    ] {
        for host in ["attacker.example", "attacker.example:8790", "10.0.0.5:8790"] {
            let (status, text) = send(
                &app,
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(header::HOST, host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri} {host}");
            assert_eq!(error_code(&text), "non_loopback_host_refused");
        }
    }
    assert!(seen(&log).is_empty(), "{:?}", seen(&log));

    // Every loopback spelling still works, and so does a request with no Host.
    for host in [
        Some("127.0.0.1:8790"),
        Some("localhost:8790"),
        Some("[::1]:8790"),
        None,
    ] {
        let mut builder = Request::builder().method("GET").uri("/v1/rooms/persistent");
        if let Some(host) = host {
            builder = builder.header(header::HOST, host);
        }
        let (status, _) = send(&app, builder.body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::OK, "{host:?}");
    }

    // Auth-on is not host-gated: a tunnel or tailnet name is how it is reached.
    let mut on = roster_state(&daemon_url);
    Arc::get_mut(&mut on).unwrap().users.clear();
    let (on_app, _d) = app_for(on);
    let (status, _) = send(
        &on_app,
        Request::builder()
            .method("GET")
            .uri("/v1/rooms/persistent")
            .header(header::HOST, "surface.example")
            .header(header::COOKIE, format!("{SESSION_COOKIE}=test-session"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    upstream.abort();
}

#[tokio::test]
async fn single_operator_and_auth_off_keep_their_constant_identity() {
    let (daemon_url, log, upstream) = spawn_recorder().await;

    // Auth on, no roster: /api/config publishes an empty user_id and the UI
    // uses `surface-operator`; nothing here is bound to a person.
    let mut single = auth_test_state();
    Arc::get_mut(&mut single).unwrap().daemon_url = daemon_url.clone();
    let (single_app, _d1) = app_for(single);
    // Auth off, loopback: same.
    let (off_app, _d2) = app_for(auth_off_state(&daemon_url, PathBuf::from("/not-used")));

    for (app, cookie) in [(&single_app, Some("test-session")), (&off_app, None)] {
        for (method, uri, body) in [
            (
                "POST",
                "/v1/rooms/persistent/team/close?actor_id=surface-operator",
                "",
            ),
            (
                "POST",
                "/v1/rooms/persistent/team/messages",
                r#"{"author_id":"surface-operator","body":"hi"}"#,
            ),
            (
                "POST",
                "/v1/rooms/persistent/team/messages",
                r#"{"author_id":"anyone","body":"hi"}"#,
            ),
        ] {
            let mut builder = Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json");
            if let Some(token) = cookie {
                builder = builder.header(header::COOKIE, format!("{SESSION_COOKIE}={token}"));
            }
            let (status, _) = send(app, builder.body(Body::from(body)).unwrap()).await;
            assert_eq!(status, StatusCode::OK, "{cookie:?} {method} {uri} {body}");
        }
    }
    assert_eq!(seen(&log).len(), 6);
    upstream.abort();
}

// ── M-B: auth-off CSRF across every mutating family ──────────────────────

fn from_source(method: &str, uri: &str, source: Option<(&str, &str)>) -> Request<Body> {
    from_source_typed(method, uri, source, "text/plain")
}

fn from_source_typed(
    method: &str,
    uri: &str,
    source: Option<(&str, &str)>,
    content_type: &str,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8790")
        // `text/plain` is what a cross-site form can send: no preflight.
        .header(header::CONTENT_TYPE, content_type);
    if let Some((name, value)) = source {
        builder = builder.header(name, value);
    }
    builder.body(Body::from("{}")).unwrap()
}

/// No-preflight mutations a hostile page can fire at a loopback proxy.
const CROSS_SITE_TARGETS: &[(&str, &str)] = &[
    (
        "POST",
        "/v1/rooms/persistent/team/close?actor_id=surface-operator",
    ),
    (
        "POST",
        "/v1/rooms/persistent/team/attachments?uploader_id=surface-operator&filename=x",
    ),
    ("POST", "/v1/rooms/persistent/team/messages"),
    ("POST", "/v1/rooms/persistent"),
    (
        "DELETE",
        "/v1/rooms/persistent/team/participants/surface-operator",
    ),
    ("PATCH", "/v1/rooms/persistent/team"),
    ("POST", "/v1/agent/turns"),
    ("POST", "/v1/agent/sessions"),
    ("POST", "/v1/permissions/p1/decision"),
    ("POST", "/v1/model"),
    ("POST", "/v1/agents"),
    ("PUT", "/v1/agents/helper"),
    ("POST", "/v1/projects"),
    ("PATCH", "/v1/projects/p1"),
    ("POST", "/v1/requests/r1/cancel"),
    ("POST", "/v1/component/event"),
    ("POST", "/v1/calls/place"),
    ("POST", "/v1/voice/realtime/client-secret"),
    ("POST", "/v1/agent/sessions/s1/messages"),
    ("POST", "/v1/rooms/main/livekit-token"),
    ("POST", "/v1/longhouse/demo"),
    ("POST", "/api/stt"),
    ("POST", "/api/devices/select"),
];

#[tokio::test]
async fn auth_off_refuses_foreign_browser_sources_on_every_mutating_family() {
    let (daemon_url, log, upstream) = spawn_recorder().await;
    let (app, _dist) = app_for(auth_off_state(&daemon_url, PathBuf::from("/not-used")));

    for (method, uri) in CROSS_SITE_TARGETS {
        for source in [
            (header::ORIGIN.as_str(), "https://attacker.example"),
            (header::REFERER.as_str(), "https://attacker.example/form"),
            (header::ORIGIN.as_str(), "null"),
            // same host, other port: a different origin
            (header::ORIGIN.as_str(), "http://127.0.0.1:9999"),
        ] {
            let (status, body) = send(&app, from_source(method, uri, Some(source))).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri} {source:?}");
            assert_eq!(
                error_code(&body),
                "cross_site_mutation_refused",
                "{method} {uri} {source:?}"
            );
        }
    }
    assert!(seen(&log).is_empty(), "{:?}", seen(&log));

    // The authority routes keep the code their clients already decode.
    let (status, body) = send(
        &app,
        from_source(
            "POST",
            "/v1/rooms/persistent/team/agents/bootstrap",
            Some((header::ORIGIN.as_str(), "https://attacker.example")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "cross_site_operator_mutation_refused");
    assert!(seen(&log).is_empty());
    upstream.abort();
}

#[tokio::test]
async fn auth_off_admits_same_origin_and_headerless_exactly_as_the_authority_policy_does() {
    let (daemon_url, log, upstream) = spawn_recorder().await;
    let (app, _dist) = app_for(auth_off_state(&daemon_url, PathBuf::from("/not-used")));

    // The rooms-persistent family is where this lands; the other families
    // answer with the recorder's 200 too, but `/api/*` handlers are not
    // pass-throughs, so only the forwarded ones are asserted on the log.
    let rooms: Vec<_> = CROSS_SITE_TARGETS
        .iter()
        .filter(|(_, uri)| uri.starts_with("/v1/"))
        .collect();
    for (method, uri) in &rooms {
        for source in [
            Some((header::ORIGIN.as_str(), "http://127.0.0.1:8790")),
            Some((header::REFERER.as_str(), "http://127.0.0.1:8790/rooms")),
            None,
        ] {
            let before = seen(&log).len();
            // The same-origin PWA sends JSON as JSON.
            let (status, body) = send(
                &app,
                from_source_typed(method, uri, source, "application/json"),
            )
            .await;
            assert_ne!(
                status,
                StatusCode::FORBIDDEN,
                "{method} {uri} {source:?} {body}"
            );
            assert_eq!(seen(&log).len(), before + 1, "{method} {uri} {source:?}");
        }
    }

    // Reads are never gated, whatever the source says.
    let (status, _) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri("/v1/rooms/persistent/team/transcript")
            .header(header::HOST, "127.0.0.1:8790")
            .header(header::ORIGIN, "https://attacker.example")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    upstream.abort();
}

#[tokio::test]
async fn auth_on_leaves_cross_site_defence_to_the_strict_session_cookie() {
    let (daemon_url, log, upstream) = spawn_recorder().await;
    let mut state = auth_test_state();
    Arc::get_mut(&mut state).unwrap().daemon_url = daemon_url;
    let (app, _dist) = app_for(state.clone());

    // The cookie a cross-site request would need is never sent cross-site.
    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("username=ocean&password=surface"))
                .unwrap(),
        )
        .await
        .unwrap();
    let cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(cookie.contains("SameSite=Strict"), "{cookie}");
    assert!(cookie.contains("HttpOnly"), "{cookie}");

    // Without it a cross-site POST is simply unauthenticated.
    let (status, _) = send(
        &app,
        from_source(
            "POST",
            "/v1/rooms/persistent/team/close?actor_id=surface-operator",
            Some((header::ORIGIN.as_str(), "https://attacker.example")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(seen(&log).is_empty());
    upstream.abort();
}

// ── JSON lanes do not upgrade a browser's no-preflight body into JSON ─────

/// Every JSON forward the proxy has, as the verb the route is registered for.
const JSON_LANES: &[(&str, &str)] = &[
    ("POST", "/v1/agent/turns"),
    ("POST", "/v1/agent/sessions"),
    ("POST", "/v1/agents"),
    ("PUT", "/v1/agents/helper"),
    ("POST", "/v1/model"),
    ("POST", "/v1/projects"),
    ("PATCH", "/v1/projects/p1"),
    ("POST", "/v1/component/event"),
    ("POST", "/v1/calls/place"),
    ("POST", "/v1/voice/realtime/client-secret"),
    ("POST", "/v1/agent/sessions/s1/messages"),
    ("POST", "/v1/rooms/main/livekit-token"),
    ("POST", "/v1/permissions/p1/decision"),
    ("POST", "/v1/longhouse/demo"),
    ("POST", "/v1/rooms/persistent"),
    ("POST", "/v1/rooms/persistent/team/messages"),
    ("PATCH", "/v1/rooms/persistent/team"),
    ("POST", "/v1/rooms/persistent/team/agents/bootstrap"),
    ("POST", "/v1/rooms/persistent/team/workspace/exec"),
];

fn typed(method: &str, uri: &str, content_type: Option<&str>, body: &str) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

#[tokio::test]
async fn a_no_preflight_body_is_never_forwarded_as_json() {
    let (daemon_url, log, upstream) = spawn_recorder().await;
    let credential_dir = tempfile::tempdir().expect("credential tempdir");
    let key_path = write_operator_key(credential_dir.path());
    // Auth-off, headerless: the CSRF gate admits these, so this rule alone
    // is what stands between them and the daemon.
    let (app, _dist) = app_for(auth_off_state(&daemon_url, key_path));

    for (method, uri) in JSON_LANES {
        for content_type in [
            Some("text/plain"),
            Some("text/plain;charset=UTF-8"),
            Some("application/x-www-form-urlencoded"),
            Some("multipart/form-data; boundary=x"),
            Some("application/jsonx"),
            Some("text/json"),
            // a typeless Blob is also sent without a preflight
            None,
        ] {
            let (status, text) =
                send(&app, typed(method, uri, content_type, r#"{"prompt":"x"}"#)).await;
            assert_eq!(
                status,
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "{method} {uri} {content_type:?}"
            );
            assert_eq!(error_code(&text), "json_content_type_required");
        }
    }
    assert!(seen(&log).is_empty(), "{:?}", seen(&log));

    // What the PWA sends still goes through: JSON as JSON (any case, with
    // parameters, or a +json type) and an empty body whatever it declares.
    for (method, uri) in JSON_LANES {
        for (content_type, body) in [
            (Some("application/json"), r#"{"prompt":"x"}"#),
            (Some("Application/JSON; charset=utf-8"), r#"{"prompt":"x"}"#),
            (Some("application/merge-patch+json"), r#"{"prompt":"x"}"#),
            (None, ""),
            (Some("text/plain"), ""),
        ] {
            let before = seen(&log).len();
            let (status, text) = send(&app, typed(method, uri, content_type, body)).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "{method} {uri} {content_type:?} {text}"
            );
            assert_eq!(
                seen(&log).len(),
                before + 1,
                "{method} {uri} {content_type:?}"
            );
        }
    }
    upstream.abort();
}

#[tokio::test]
async fn raw_byte_lanes_keep_their_own_type() {
    let (daemon_url, log, upstream) = spawn_recorder().await;
    let (app, _dist) = app_for(auth_off_state(&daemon_url, PathBuf::from("/not-used")));

    // The attachment upload is raw bytes: the PWA sends a typeless
    // ArrayBuffer, and any declared type is the file's own.
    for content_type in [None, Some("text/plain"), Some("image/png")] {
        let before = seen(&log).len();
        let (status, text) = send(
            &app,
            typed(
                "POST",
                "/v1/rooms/persistent/team/attachments?uploader_id=surface-operator&filename=a",
                content_type,
                "not json at all",
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{content_type:?} {text}");
        assert_eq!(seen(&log).len(), before + 1);
    }
    upstream.abort();
}
