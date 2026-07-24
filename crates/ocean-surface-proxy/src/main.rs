//! Ocean Surface — proxy.
//!
//! Two jobs:
//!
//! 1. Serve the compiled WASM bundle (Trunk's `dist/` directory) so a phone
//!    on the same network can load the app over HTTP without needing trunk
//!    serve running. Production deployment runs *only* this binary.
//!
//! 2. Forward STT + TTS requests to the daemon's voice endpoints so the
//!    browser never touches provider credentials. The daemon holds the
//!    xAI key; the proxy relays `/api/stt` and `/api/tts` to it.
//!
//! Run: `cargo run -p ocean-surface-proxy -- --dist ./dist --bind 0.0.0.0:8790`
//! Then point a browser at http://<host>:8790/.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use axum::{
    body::Bytes,
    extract::{Form, Path, Request, State},
    http::{header, HeaderMap, HeaderName, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tower_http::{services::ServeDir, trace::TraceLayer};
use tracing_subscriber::EnvFilter;

const DEFAULT_DAEMON_URL: &str = "http://127.0.0.1:4780";
const DEFAULT_LIVEKIT_ROOM_ID: &str = "project:surface-main";
const DEFAULT_VOICE_PROFILE: &str = "leo";

const SESSION_COOKIE: &str = "ocean_session";
const SESSION_MAX_AGE_SECONDS: u64 = 60 * 60 * 24 * 30;

const CALL_PLACE_DAEMON_PATH: &str = "/v1/calls/place";

/// Shared state.
struct AppState {
    /// General-purpose client used for the reverse-proxy routes — including the
    /// long-lived SSE streams (`/v1/agent/events`, `/v1/events`) that are piped
    /// through `bytes_stream()`. It deliberately has **no** request timeout: a
    /// `reqwest` `.timeout()` covers the whole request lifetime including reading
    /// the body, which would sever those open-ended event streams mid-session.
    http: reqwest::Client,
    /// TASK-73: buffered JSON forwards use a SEPARATE client that DOES carry a
    /// timeout. Before this, every forward shared the untimed SSE client, so a
    /// wedged daemon hung each JSON passthrough indefinitely — tasks and
    /// sockets accumulated on the proxy with no upper bound and the operator
    /// saw a spinner instead of an error. SSE keeps the untimed client above
    /// (a timeout there would sever live event streams mid-session), so the
    /// split is: streams untimed by necessity, request/response bounded.
    http_json: reqwest::Client,
    voice_profile: String,
    daemon_url: String,
    default_livekit_room_id: String,
    tldraw_sync_uri: Option<String>,
    /// Google Maps JS API key, handed to the client via /api/config so the map
    /// component can load the Maps script. Maps browser keys are referrer-
    /// restricted (not secret), so client-side exposure is the intended model.
    maps_key: Option<String>,
    /// Map ID for the map's visual style (DEMO_MAP_ID by default).
    maps_map_id: String,
    /// Optional operator login. `Some((user, pass))` enables the app-owned
    /// login form and session-cookie gate. `None` = open local development.
    /// The random session token is process-local, so a proxy restart safely
    /// expires every browser session without persisting bearer material.
    basic_auth: Option<(String, String)>,
    session_token: String,
    /// Mode-0600 boot-bound credential minted and rotated by ocean-daemon.
    /// Read immediately before each Observatory request; never sent to the browser.
    observer_token_path: PathBuf,
}

impl AppState {
    /// The proxy no longer holds provider credentials — STT/TTS routes forward
    /// to the daemon, which resolves the xAI key per-request. Report routes as
    /// available; per-request errors carry credential state from the daemon.
    fn has_auth(&self) -> bool {
        true
    }
}

fn ocean_config_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("OCEAN_CONFIG_DIR") {
        return PathBuf::from(path);
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("ocean-rs");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".config").join("ocean-rs");
    }
    PathBuf::from(".ocean-rs")
}

fn session_secret_path() -> PathBuf {
    if let Some(path) = std::env::var_os("OCEAN_SURFACE_SESSION_SECRET_FILE") {
        return PathBuf::from(path);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("ocean-surface")
            .join("session-secret");
    }
    PathBuf::from(".ocean-surface-session-secret")
}

fn read_mode_0600_secret(path: &FsPath, label: &str) -> anyhow::Result<String> {
    let link = std::fs::symlink_metadata(path)
        .with_context(|| format!("{label} unavailable at {}", path.display()))?;
    if link.file_type().is_symlink() || !link.is_file() {
        anyhow::bail!("{label} must be a regular file");
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("opening {label}"))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("reading {label} metadata"))?;
    if !metadata.is_file() || metadata.mode() & 0o777 != 0o600 {
        anyhow::bail!("{label} must be a mode-0600 regular file");
    }
    let mut value = String::new();
    file.read_to_string(&mut value)
        .with_context(|| format!("reading {label}"))?;
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("{label} is empty");
    }
    Ok(value.to_owned())
}

fn load_or_create_session_secret(path: &FsPath) -> anyhow::Result<String> {
    if path.exists() {
        return read_mode_0600_secret(path, "surface session secret");
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).context("OS randomness required for session secret")?;
    let secret = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(secret.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            Ok(secret)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            read_mode_0600_secret(path, "surface session secret")
        }
        Err(error) => Err(error).with_context(|| format!("creating {}", path.display())),
    }
}

fn derive_session_token(
    credentials: Option<&(String, String)>,
    secret_path: &FsPath,
) -> anyhow::Result<String> {
    let secret = load_or_create_session_secret(secret_path)?;
    let mut digest = Sha256::new();
    digest.update(secret.as_bytes());
    if let Some((user, pass)) = credentials {
        digest.update(b"\0user\0");
        digest.update(user.as_bytes());
        digest.update(b"\0pass\0");
        digest.update(pass.as_bytes());
    }
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest.finalize()))
}

/// Read the daemon-minted observer token without following symlinks. The
/// complete credential stays on the proxy side of the browser boundary.
fn read_observer_token(path: &FsPath) -> Result<String, String> {
    let link = std::fs::symlink_metadata(path)
        .map_err(|error| format!("observer credential unavailable: {error}"))?;
    if link.file_type().is_symlink() || !link.is_file() {
        return Err("observer credential must be a regular file".to_owned());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| format!("observer credential unavailable: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("observer credential unavailable: {error}"))?;
    if !metadata.is_file() || metadata.mode() & 0o777 != 0o600 {
        return Err("observer credential must be a mode-0600 regular file".to_owned());
    }
    let mut token = String::new();
    file.read_to_string(&mut token)
        .map_err(|error| format!("observer credential unavailable: {error}"))?;
    let token = token.trim();
    if token.is_empty() {
        return Err("observer credential is empty".to_owned());
    }
    Ok(token.to_owned())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "ocean_surface_proxy=info".into()),
        )
        .init();

    let bind: SocketAddr = std::env::var("OCEAN_SURFACE_BIND")
        .unwrap_or_else(|_| "0.0.0.0:8790".into())
        .parse()
        .context("OCEAN_SURFACE_BIND must be host:port")?;

    let dist = std::env::var("OCEAN_SURFACE_DIST")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("dist"));

    let voice_profile =
        std::env::var("OCEAN_VOICE_PROFILE").unwrap_or_else(|_| DEFAULT_VOICE_PROFILE.into());
    let daemon_url =
        std::env::var("OCEAN_DAEMON_URL").unwrap_or_else(|_| DEFAULT_DAEMON_URL.into());
    let default_livekit_room_id = std::env::var("OCEAN_LIVEKIT_ROOM_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_LIVEKIT_ROOM_ID.to_string());
    let tldraw_sync_uri = std::env::var("OCEAN_TLDRAW_SYNC_URI")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    // Google Maps JS API key for the map component. An explicit, non-empty
    // environment value enables maps; absence is safe and leaves the component
    // on its existing unavailable notice. Never ship an organization-owned
    // default key in the public source tree or release bundle.
    let maps_key = std::env::var("GOOGLE_MAPS_API_KEY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if maps_key.is_some() {
        tracing::info!("Google Maps key resolved; map component enabled");
    }
    // Map ID controls the map's visual style. Defaults to DEMO_MAP_ID (works
    // with advanced markers + Places UI Kit out of the box). Set
    // GOOGLE_MAPS_MAP_ID to your custom styled map id to skin it.
    let maps_map_id = std::env::var("GOOGLE_MAPS_MAP_ID")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "DEMO_MAP_ID".to_string());

    // Operator login. Credentials come from the environment — never hardcoded,
    // since this binds to 0.0.0.0 behind a public tunnel. The browser exchanges
    // them once for an HttpOnly session cookie; unlike an HTTP Basic challenge,
    // that session survives standalone iOS/Chrome PWA launches reliably.
    let basic_auth = if std::env::var("OCEAN_SURFACE_AUTH").as_deref() == Ok("off") {
        tracing::warn!("operator login DISABLED (OCEAN_SURFACE_AUTH=off)");
        None
    } else {
        let user = std::env::var("OCEAN_SURFACE_USER")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let pass = std::env::var("OCEAN_SURFACE_PASS")
            .ok()
            .filter(|s| !s.trim().is_empty());
        match (user, pass) {
            (Some(user), Some(pass)) => {
                tracing::info!("operator session login enabled");
                Some((user, pass))
            }
            _ => {
                panic!(
                    "operator login is on but OCEAN_SURFACE_USER / OCEAN_SURFACE_PASS \
                     are not set. Set both, or OCEAN_SURFACE_AUTH=off for trusted localhost."
                );
            }
        }
    };

    // Stable across deploys so an installed PWA remains signed in. The mode-0600
    // server secret never reaches the browser; rotating the configured username
    // or password changes the derived token and invalidates prior sessions.
    let session_token = derive_session_token(basic_auth.as_ref(), &session_secret_path())?;

    let observer_token_path = std::env::var_os("OCEAN_OBSERVER_TOKEN_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| ocean_config_dir().join("observatory-token"));

    let state = Arc::new(AppState {
        // TASK-71: never follow upstream redirects. A redirect-following
        // reverse proxy is an SSRF primitive waiting on a daemon-side 3xx —
        // the daemon returns none today, but this boundary should not depend
        // on that staying true.
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client with no-redirect policy should build"),
        http_json: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(JSON_FORWARD_TIMEOUT)
            .build()
            .expect("reqwest json client should build"),
        voice_profile,
        daemon_url,
        default_livekit_room_id,
        tldraw_sync_uri,
        basic_auth,
        session_token,
        maps_key,
        maps_map_id,
        observer_token_path,
    });

    let app = build_app(state, &dist);

    tracing::info!(?bind, dist = %dist.display(), "ocean-surface-proxy listening");
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// The full production router. Extracted from `main` so tests can exercise the
/// REAL route table (a synthetic router that registers routes inside the test
/// proves nothing — deleting a production route would leave it green).
fn build_app(state: Arc<AppState>, dist: &std::path::Path) -> Router {
    Router::new()
        .route("/health", get(health))
        // TASK-74: CSP violation sink. Without somewhere to report, the
        // report-only policy from TASK-72 is decorative — browsers log to
        // their own console and nobody ever sees it. This turns it into
        // operator-visible signal, which is the prerequisite for deciding
        // what a real enforced script-src can safely contain.
        .route("/csp-report", post(csp_report))
        .route("/login", get(login_page).post(login_submit))
        .route("/logout", post(logout))
        .route("/api/config", get(config))
        .route("/api/stt", post(stt))
        .route("/api/tts", post(tts))
        // Reverse-proxy the daemon's agent API so a remote browser (phone via
        // the tunnel) talks to the daemon through this same origin — no
        // hardcoded localhost, no mixed-content. The daemon stays bound to
        // 127.0.0.1 and is never exposed directly.
        .route("/v1/agent/turns", post(proxy_turns))
        .route("/v1/agent/events", get(proxy_events))
        // Read-only, metadata-safe Observatory contract. The proxy injects the
        // daemon's rotating local credential on the server-side hop; browsers
        // never receive the token or signing secret.
        .route("/v1/observatory/snapshot", get(proxy_observatory))
        .route("/v1/observatory/events", get(proxy_observatory))
        .route("/v1/observatory/replay", get(proxy_observatory))
        // Control stream + permission decision (OCEAN-135/136). The web UI opens
        // GET /v1/events (the CONTROL stream that carries permission_request
        // cards) and answers a prompt by POSTing
        // /v1/permissions/{id}/decision. Without these reverse-proxy routes both
        // fell through to ServeDir → 404, so on the phone/tunnel surface a
        // permission-gated tool call hung the turn forever (the card never
        // arrived) and Allow/Deny never reached the daemon. /v1/events streams
        // like /v1/agent/events; the decision route forwards body + the {id}.
        .route("/v1/events", get(proxy_control_events))
        .route("/v1/permissions", get(proxy_permissions_snapshot))
        .route(
            "/v1/permissions/{id}/decision",
            post(proxy_permission_decision),
        )
        .route(
            "/v1/agent/sessions",
            get(proxy_sessions).post(proxy_sessions_post),
        )
        .route("/v1/sessions/{id}", get(proxy_session_detail))
        .route("/v1/agent/sessions/{id}", get(proxy_agent_session_detail))
        // Model picker + halt button reach the daemon through this origin too.
        .route("/v1/models", get(proxy_models))
        .route("/v1/model", get(proxy_model_get).post(proxy_model_set))
        // Agent identity picker (TASK-9/TASK-11): surfaces call GET /v1/agents
        // same-origin; the proxy forwards to the daemon and returns the JSON list.
        .route("/v1/agents", get(proxy_agents))
        .route("/v1/fs/dirs", get(proxy_fs_dirs))
        .route(
            "/v1/projects",
            get(proxy_projects_list).post(proxy_projects_create),
        )
        .route(
            "/v1/projects/{id}",
            get(proxy_project_get)
                .patch(proxy_project_patch)
                .delete(proxy_project_delete),
        )
        // Persistent Rooms lifecycle (OCEAN-65/107/120). The Rooms UI talks to
        // these same-origin; without these reverse-proxy routes every rooms
        // request fell through to ServeDir → 404 (empty body) → the UI's
        // resp.json() choked with "EOF while parsing a value" and the whole
        // Rooms feature was dead on web. The `/persistent` literal route covers
        // list (GET) + create (POST); the wildcard catch-all covers every
        // sub-path (room get, participants join/leave, messages, transcript,
        // event stream) forwarding method + body + query. The handler internally
        // branches on the exact GET `{key}/events` shape to stream through
        // sse_stream_response rather than buffering (TASK-11 axum route conflict
        // fix: {key}/events and {*rest} cannot coexist at the same prefix).
        // Declared BEFORE the livekit-token route so the `persistent` segment is
        // matched as a literal, never swallowed by the `{room_id}` capture —
        // though the two are distinct subtrees either way.
        .route(
            "/v1/rooms/persistent",
            get(proxy_rooms_persistent).post(proxy_rooms_persistent),
        )
        .route(
            "/v1/rooms/persistent/{*rest}",
            get(proxy_rooms_persistent)
                .post(proxy_rooms_persistent)
                .delete(proxy_rooms_persistent),
        )
        .route(
            "/v1/rooms/{room_id}/livekit-token",
            post(proxy_livekit_token),
        )
        .route("/v1/requests/{id}/cancel", post(proxy_cancel))
        // Component interaction events (kanban click / form submit) flow from a
        // remote surface back to the daemon through this origin too, so a phone
        // via the tunnel can drive interactive components (OCEAN-62c).
        .route("/v1/component/event", post(proxy_component_event))
        // Outbound call placement (POST /v1/calls/place → daemon passthrough).
        .route("/v1/calls/place", post(proxy_call_place))
        // Realtime voice chat (voice phases 2/3): the browser asks the daemon
        // to mint an ephemeral OpenAI Realtime client secret same-origin, then
        // talks WebRTC directly to OpenAI. The voice agent's handoff notes are
        // appended into the chat session through this origin too.
        .route(
            "/v1/voice/realtime/client-secret",
            post(proxy_realtime_client_secret),
        )
        .route(
            "/v1/agent/sessions/{id}/messages",
            post(proxy_session_message_append),
        )
        // Longhouse council CONTROL endpoints (convene / demo) reach the daemon
        // through this origin, so the native in-app council surface can drive a
        // real council same-origin (e.g. POST /v1/longhouse/demo). The resulting
        // council events arrive back on /v1/agent/events as
        // extension=="longhouse" frames, which the surface now captures natively.
        .route(
            "/v1/longhouse/{*rest}",
            get(proxy_longhouse).post(proxy_longhouse),
        )
        // The Game Boy "longhouse" deck (/ui/council, /longhouse.html), its
        // `council_deck` handler, and the embedded `COUNCIL_DECK_HTML` const
        // were removed when the council modal went native — the iframe body was
        // swapped for the in-app Leptos council component fed by the daemon's
        // captured longhouse payloads.
        .fallback_service(ServeDir::new(dist).append_index_html_on_directories(true))
        // Set Cache-Control on static-file responses so a CDN / tunnel can never
        // wedge the app on a stale shell again (OCEAN — "blank pane / 11-minute
        // load"). The shell + sw.js + manifest are `no-cache, no-store,
        // must-revalidate` so a new deploy / new worker is always fetched fresh;
        // hashed, content-addressed assets are `immutable`. The middleware
        // leaves API / proxy / SSE routes (and `.wasm`, owned by `wasm_headers`
        // below) untouched. It sits OUTSIDE the auth gate so it only ever
        // decorates responses that were already allowed.
        .layer(middleware::from_fn(static_cache_headers))
        // Fix the headers on the compiled `.wasm` asset. ServeDir guesses
        // `application/wasm` itself, but the deployed page once broke because
        // (1) Chrome aborted the wasm preload while Trunk emitted an
        // `integrity` attribute on it (see Trunk.toml's `no_sri` note —
        // "integrity … ignored for preload destinations … credentials mode
        // does not match"), observed alongside tunnel `content-encoding:
        // zstd`, and (2) the immutable hashed asset wasn't marked cacheable.
        // SRI/preload integrity is now disabled at build time (`no_sri` in
        // Trunk.toml), so a CDN-applied `content-encoding` is safe — and
        // wanted: it turns the ~3.7 MB optimized wasm into a ~1 MB transfer.
        // This post-response layer forces `Content-Type: application/wasm`
        // and `Cache-Control: public, max-age=31536000, immutable` (no
        // `no-transform`, so Cloudflare MAY compress the body). Declared AFTER
        // `static_cache_headers` so it owns the final `.wasm` response
        // headers; it runs AFTER routing/ServeDir so it only touches the
        // actual file response; non-wasm paths pass through untouched.
        .layer(middleware::from_fn(wasm_headers))
        // Security headers decorate the login document and application shell.
        .layer(middleware::from_fn(security_headers))
        // App-owned session auth deliberately does not emit WWW-Authenticate:
        // browser-native Basic prompts loop in standalone iOS PWAs. Navigations
        // redirect to /login; API calls receive a plain 401.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            session_auth_gate,
        ))
        // TASK-73: no CORS layer in the deployed topology. The app is served
        // by this same proxy and `config_payload` deliberately hands the client
        // an empty daemon_url so it never goes cross-origin — permissive() was
        // dev convenience that also answered preflights BEFORE the auth gate
        // and stamped `*` onto 401s. Nothing legitimate needs it; a future
        // genuine cross-origin consumer should get a narrow allow-list here,
        // not a blanket permit.
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Resources that are safe before login. API namespaces are never public.
fn is_public_boot_asset(path: &str) -> bool {
    if path.starts_with("/v1/") || path.starts_with("/api/") {
        return false;
    }

    path == "/login"
        || path == "/health"
        || path == "/csp-report"
        || path == "/manifest.webmanifest"
        || path == "/sw.js"
        || path == "/favicon.ico"
        || path == "/apple-touch-icon.png"
        || path.starts_with("/icon-")
        || path.starts_with("/brand/")
        || path.starts_with("/fonts/")
        || is_hashed_asset(path)
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|part| {
            part.split_once('=')
                .filter(|(key, _)| *key == name)
                .map(|(_, value)| value)
        })
}

fn has_valid_session(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(provided) = cookie_value(headers, SESSION_COOKIE) else {
        return false;
    };
    constant_time_eq(provided.as_bytes(), state.session_token.as_bytes())
}

fn has_valid_basic_credentials(state: &AppState, headers: &HeaderMap) -> bool {
    let Some((want_user, want_pass)) = state.basic_auth.as_ref() else {
        return true;
    };
    let provided = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok());
    let Some((user, pass)) = provided.as_deref().and_then(|value| value.split_once(':')) else {
        return false;
    };
    let user_ok = constant_time_eq(user.as_bytes(), want_user.as_bytes());
    let pass_ok = constant_time_eq(pass.as_bytes(), want_pass.as_bytes());
    user_ok & pass_ok
}

async fn session_auth_gate(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    if state.basic_auth.is_none()
        || is_public_boot_asset(req.uri().path())
        || has_valid_session(&state, req.headers())
        // Keep scripted/smoke clients compatible during migration, but never
        // challenge a browser for Basic credentials.
        || has_valid_basic_credentials(&state, req.headers())
    {
        return next.run(req).await;
    }

    let is_navigation = req.method() == axum::http::Method::GET
        && req
            .headers()
            .get(header::ACCEPT)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("text/html"));
    if is_navigation {
        return Redirect::to("/login").into_response();
    }
    (StatusCode::UNAUTHORIZED, "authentication required").into_response()
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
}

fn request_is_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("https"))
}

fn same_origin_form(headers: &HeaderMap) -> bool {
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let scheme = if request_is_https(headers) {
        "https"
    } else {
        "http"
    };
    origin == format!("{scheme}://{host}")
}

fn login_html(error: bool) -> Html<String> {
    let error_message = if error {
        "<p class=\"error\" role=\"alert\">That username or password was not accepted.</p>"
    } else {
        ""
    };
    Html(format!(
        r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover"><meta name="theme-color" content="#060606"><title>Sign in · Ocean</title>
<style>
*{{box-sizing:border-box}}html,body{{min-height:100%;margin:0}}body{{display:grid;place-items:center;background:#060606;color:#fafcff;font:15px Poppins,system-ui,-apple-system,sans-serif;padding:calc(24px + env(safe-area-inset-top)) 24px calc(24px + env(safe-area-inset-bottom))}}main{{width:min(100%,380px);padding:32px;border:1px solid #272b31;border-radius:24px;background:#0d0f12;box-shadow:0 24px 80px #000}}img{{display:block;width:72px;height:72px;margin:0 auto 20px}}h1{{margin:0;text-align:center;font-size:28px}}.sub{{color:#aab2bd;text-align:center;margin:8px 0 28px}}label{{display:block;color:#d6dbe2;font-size:13px;margin:16px 0 7px}}input{{width:100%;border:1px solid #343a43;border-radius:12px;background:#08090b;color:#fafcff;padding:13px 14px;font:inherit;outline:none}}input:focus{{border-color:#00d7d7;box-shadow:0 0 0 3px #00d7d722}}button{{width:100%;border:0;border-radius:12px;margin-top:22px;padding:13px;background:#00d7d7;color:#03181a;font:600 15px inherit;cursor:pointer}}.error{{border:1px solid #673b3b;border-radius:10px;background:#251414;color:#ffb9b9;padding:10px 12px;font-size:13px}}.note{{color:#77818d;text-align:center;font-size:12px;margin:18px 0 0}}
</style></head><body><main><img src="/brand/master-1024.png" alt=""><h1>Ocean</h1><p class="sub">Sign in to your private surface.</p>{error_message}<form method="post" action="/login"><label for="username">Username</label><input id="username" name="username" autocomplete="username" autocapitalize="none" spellcheck="false" required maxlength="256"><label for="password">Password</label><input id="password" name="password" type="password" autocomplete="current-password" required maxlength="256"><button type="submit">Continue</button></form><p class="note">Credentials stay between this device and your Ocean proxy.</p></main></body></html>"##
    ))
}

async fn login_page(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if state.basic_auth.is_none() || has_valid_session(&state, &headers) {
        return Redirect::to("/").into_response();
    }
    login_html(false).into_response()
}

async fn login_submit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    let Some((want_user, want_pass)) = state.basic_auth.as_ref() else {
        return Redirect::to("/").into_response();
    };
    if !same_origin_form(&headers) || form.username.len() > 256 || form.password.len() > 256 {
        return (StatusCode::FORBIDDEN, "login request rejected").into_response();
    }
    let user_ok = constant_time_eq(form.username.as_bytes(), want_user.as_bytes());
    let pass_ok = constant_time_eq(form.password.as_bytes(), want_pass.as_bytes());
    if !(user_ok & pass_ok) {
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
        return (StatusCode::UNAUTHORIZED, login_html(true)).into_response();
    }

    let secure = if request_is_https(&headers) {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "{SESSION_COOKIE}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age={SESSION_MAX_AGE_SECONDS}{secure}",
        state.session_token
    );
    let mut response = Redirect::to("/").into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        cookie
            .parse()
            .expect("session cookie must be a valid header"),
    );
    response
}

async fn logout(headers: HeaderMap) -> Response {
    if !same_origin_form(&headers) {
        return (StatusCode::FORBIDDEN, "logout request rejected").into_response();
    }
    let secure = if request_is_https(&headers) {
        "; Secure"
    } else {
        ""
    };
    let mut response = Redirect::to("/login").into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure}")
            .parse()
            .expect("expired cookie must be a valid header"),
    );
    response
}

/// Header value: long-lived and immutable. Deliberately WITHOUT
/// `no-transform`: with SRI disabled at build time (`no_sri` in Trunk.toml)
/// a `content-encoding` applied by the tunnel can no longer abort the wasm
/// preload (there is no integrity attribute left to mismatch), and
/// compressing the module is a large win on slow links.
const WASM_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// Response post-processor: for any request whose path ends in `.wasm`, force
/// the correct MIME (`application/wasm`, required for `instantiateStreaming`)
/// and the immutable cache policy above.
/// Everything else passes through unchanged. This is the primary fix for the
/// blank deployed page — see the layer registration in `main` for the full
/// root-cause writeup.
/// Baseline security response headers (TASK-72).
///
/// The surface renders untrusted model output on the same origin that holds
/// the operator's basic-auth session — and that session is the only thing
/// between the internet and a daemon with no auth of its own. Before this,
/// the proxy set no security headers at all.
///
/// Split deliberately into ENFORCED and REPORT-ONLY:
///
/// Enforced here are the headers that cannot break a working app —
/// `nosniff` (ServeDir already sends correct types), `no-referrer` (nothing
/// depends on outbound Referer), `frame-ancestors 'none'` (the app is never
/// framed; this is the clickjacking guard for an authenticated session),
/// `base-uri 'self'` (a `<base>` injected into rendered output cannot
/// re-root relative URLs), and `object-src 'none'`.
///
/// CSP proper ships REPORT-ONLY, and the reason is worth stating plainly: the
/// Trunk-generated shell carries large INLINE `<script>` blocks (LiveKit
/// wiring, social embeds), so an enforcing policy would need
/// `script-src 'unsafe-inline'` — which defeats most of what CSP is for. The
/// honest fix is nonce- or hash-based script-src, which means rewriting the
/// shell at build time; that is follow-up work, not a header change. Until
/// then Report-Only tells us exactly what a real policy would break without
/// risking a blank app for the operator.
///
/// The allow-list reflects what the bundle actually loads today: jsdelivr
/// (livekit-client ESM), tiktok/instagram embed scripts, Google Maps JS, and
/// `wss:` for LiveKit rooms whose URL is server-supplied per token.
async fn security_headers(req: Request, next: Next) -> Response {
    // API/proxy/SSE responses are JSON or event streams — a document CSP is
    // meaningless there, and skipping them keeps this off the hot paths.
    let path = req.uri().path().to_string();
    let is_api = path.starts_with("/v1/") || path.starts_with("/api/");
    let mut resp = next.run(req).await;
    if is_api {
        return resp;
    }
    let headers = resp.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        header::HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        header::HeaderValue::from_static(CSP_ENFORCED),
    );
    headers.insert(
        header::HeaderName::from_static("content-security-policy-report-only"),
        header::HeaderValue::from_static(CSP_REPORT_ONLY),
    );
    resp
}

/// Enforced directives only — chosen because none of them can break a working
/// page. Notably absent: `script-src`/`default-src` (see `security_headers`).
const CSP_ENFORCED: &str = "frame-ancestors 'none'; base-uri 'self'; object-src 'none'";

/// The policy we WANT, shipped observe-only until inline scripts get nonces.
/// `'unsafe-inline'` appears here on purpose: it documents the current gap
/// rather than hiding it, so violation reports show what else would break.
const CSP_REPORT_ONLY: &str = "default-src 'self'; \
     script-src 'self' 'wasm-unsafe-eval' 'unsafe-inline' https://cdn.jsdelivr.net https://www.tiktok.com https://www.instagram.com https://maps.googleapis.com https://*.gstatic.com; \
     style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; \
     img-src 'self' data: blob: https:; \
     media-src 'self' data: blob:; \
     font-src 'self' data: https://fonts.gstatic.com; \
     connect-src 'self' wss: https://maps.googleapis.com https://*.gstatic.com; \
     worker-src 'self' blob:; \
     frame-src https://www.tiktok.com https://www.instagram.com; \
     frame-ancestors 'none'; base-uri 'self'; object-src 'none'; \
     report-uri /csp-report";

async fn wasm_headers(req: Request, next: Next) -> Response {
    let is_wasm = req.uri().path().ends_with(".wasm");
    let mut resp = next.run(req).await;
    if is_wasm && resp.status().is_success() {
        let headers = resp.headers_mut();
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/wasm"),
        );
        headers.insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static(WASM_CACHE_CONTROL),
        );
    }
    resp
}

/// Cache-Control gate for static files (the `ServeDir` fallback).
///
/// Root cause of the "blank pane / 11-minute load": a wedged service worker
/// pinned a stale shell. A long-lived HTTP cache on `sw.js` / the HTML shell is
/// exactly how an old worker gets stuck — so we pin the cache policy here at the
/// origin instead of trusting `ServeDir`'s (header-less) defaults or whatever a
/// tunnel/CDN decides on its own:
///
///   • HTML shell, `sw.js`, and `manifest.webmanifest` → `no-cache, no-store,
///     must-revalidate` (+ `Pragma: no-cache`). These have no content hash, so
///     they MUST be revalidated on every load; this guarantees a fresh deploy /
///     a fresh worker is always picked up and an old one can never pin itself.
///   • Hashed, content-addressed assets (`*-<hash>.{js,wasm,css}`) →
///     `public, max-age=31536000, immutable`. Their bytes never change for a
///     given URL (a new build emits a new filename), so caching them forever is
///     safe and fast.
///   • Everything else → a short `max-age=300` with revalidation.
async fn static_cache_headers(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();

    // Only decorate static-file responses. API, reverse-proxy, SSE, and the
    // embedded council/health routes set (or intentionally omit) their own
    // headers — leave them alone so we never clobber `Content-Type:
    // text/event-stream` caching or proxied JSON.
    // `.wasm` is owned by the `wasm_headers` layer (it forces the MIME type
    // `instantiateStreaming` requires). Skip it here so we never overwrite
    // that Cache-Control.
    let is_dynamic = path.starts_with("/v1/")
        || path.starts_with("/api/")
        || path == "/health"
        || path.ends_with(".wasm");
    if is_dynamic {
        return next.run(req).await;
    }

    let mut resp = next.run(req).await;

    let value = if path == "/sw.js"
        || path == "/login"
        || path == "/logout"
        || path == "/"
        || path.ends_with('/')
        || path.ends_with(".html")
        || path.ends_with("manifest.webmanifest")
    {
        // Shell + worker + manifest: always revalidate, never pin.
        resp.headers_mut().insert(
            header::PRAGMA,
            axum::http::HeaderValue::from_static("no-cache"),
        );
        "no-cache, no-store, must-revalidate"
    } else if is_hashed_asset(&path) {
        // Content-addressed build artifacts: cache forever.
        "public, max-age=31536000, immutable"
    } else {
        // Icons and other un-hashed assets: brief cache, then revalidate.
        "public, max-age=300, must-revalidate"
    };

    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static(value),
    );
    resp
}

/// True for Trunk's content-hashed build artifacts. Trunk emits the hash as a
/// `-<hex>` suffix on the file stem, e.g.:
///   `index-1a2b3c4d5e.js`
///   `ocean-surface-ui-1a2b3c4d5e_bg.wasm`
///   `style-1a2b3c4d5e.css`
/// The hash is a hex run of 8+ chars at the end of the stem, so the same URL
/// always maps to the same bytes — safe to mark `immutable`. The HTML shell and
/// `sw.js` carry no hash and are handled by the no-store branch above.
fn is_hashed_asset(path: &str) -> bool {
    // Restrict to immutable asset extensions.
    let lower = path.to_ascii_lowercase();
    if !(lower.ends_with(".js") || lower.ends_with(".wasm") || lower.ends_with(".css")) {
        return false;
    }
    // Filename only, then strip the extension and the wasm-bindgen `_bg` infix
    // so the trailing `-<hash>` is exposed regardless of asset type.
    let file = path.rsplit('/').next().unwrap_or(path);
    let stem = match file.rsplit_once('.') {
        Some((stem, _ext)) => stem,
        None => return false,
    };
    let stem = stem.strip_suffix("_bg").unwrap_or(stem);
    // The stem must end in `-<8+ hex>`.
    match stem.rsplit_once('-') {
        Some((_, hash)) => hash.len() >= 8 && hash.chars().all(|c| c.is_ascii_hexdigit()),
        None => false,
    }
}

/// Health check. STT/TTS routes are always available — per-request errors
/// carry the daemon's credential state (the proxy no longer holds the xAI key).
async fn health() -> Json<Value> {
    Json(json!({
        "ok": true,
        "service": "ocean-surface-proxy",
        "stt": true,
        "tts": true,
    }))
}

/// Zero-config bootstrap the UI fetches on load. `daemon_url` is empty so the
/// client talks to the daemon through THIS origin (the /v1/agent/* reverse
/// proxy below) — works identically on localhost and through the tunnel, with
/// no mixed-content or hardcoded host.
async fn config(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(config_payload(&state))
}

fn config_payload(state: &AppState) -> Value {
    json!({
        "daemon_url": "",
        "has_auth": state.has_auth(),
        "voice_profile": state.voice_profile,
        "maps_key": state.maps_key.clone().unwrap_or_default(),
        "maps_map_id": state.maps_map_id.clone(),
        "livekit_room_id": state.default_livekit_room_id,
        "livekit_token_path": livekit_token_daemon_path(&state.default_livekit_room_id),
        "tldraw_sync_uri": state.tldraw_sync_uri.clone().unwrap_or_default(),
        "surface": {
            "livekit_room_id": state.default_livekit_room_id,
            "livekit_token_path": livekit_token_daemon_path(&state.default_livekit_room_id),
            "tldraw_sync_uri": state.tldraw_sync_uri.clone().unwrap_or_default(),
        }
    })
}

/// Reverse-proxy POST /v1/agent/turns to the local daemon.
async fn proxy_turns(State(state): State<Arc<AppState>>, body: Bytes) -> impl IntoResponse {
    let url = format!("{}/v1/agent/turns", state.daemon_url.trim_end_matches('/'));
    match state
        .http_json
        .post(&url)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.to_vec())
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            let bytes = resp.bytes().await.unwrap_or_default();
            (
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                [(header::CONTENT_TYPE, "application/json")],
                bytes,
            )
                .into_response()
        }
        Err(err) => (StatusCode::BAD_GATEWAY, daemon_unreachable_body(&err)).into_response(),
    }
}

/// Reverse-proxy POST /v1/agent/sessions to the local daemon.
async fn proxy_sessions_post(State(state): State<Arc<AppState>>, body: Bytes) -> impl IntoResponse {
    let url = format!(
        "{}/v1/agent/sessions",
        state.daemon_url.trim_end_matches('/')
    );
    match state
        .http_json
        .post(&url)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.to_vec())
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            let bytes = resp.bytes().await.unwrap_or_default();
            (
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                [(header::CONTENT_TYPE, "application/json")],
                bytes,
            )
                .into_response()
        }
        Err(err) => (StatusCode::BAD_GATEWAY, daemon_unreachable_body(&err)).into_response(),
    }
}

/// Reverse-proxy GET /v1/agent/sessions to the local daemon.
async fn proxy_sessions(State(state): State<Arc<AppState>>, req: Request) -> impl IntoResponse {
    let q = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let url = format!(
        "{}/v1/agent/sessions{q}",
        state.daemon_url.trim_end_matches('/')
    );
    match state.http_json.get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            let bytes = resp.bytes().await.unwrap_or_default();
            (
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                [(header::CONTENT_TYPE, "application/json")],
                bytes,
            )
                .into_response()
        }
        Err(err) => (StatusCode::BAD_GATEWAY, daemon_unreachable_body(&err)).into_response(),
    }
}

/// Single-session detail passthrough. The chat app loads a session's transcript
/// via GET /v1/sessions/{id} (and the /v1/agent/sessions/{id} variant). Without
/// these the proxy 404'd that path → the app parsed an empty body → "EOF while
/// parsing a value" → blank chat history on session switch.
async fn proxy_session_detail(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    proxy_get_json(&state, &format!("/v1/sessions/{id}")).await
}

async fn proxy_agent_session_detail(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    proxy_get_json(&state, &format!("/v1/agent/sessions/{id}")).await
}

/// JSON GET passthrough helper for small daemon endpoints.
async fn proxy_get_json(state: &AppState, path: &str) -> Response {
    let url = format!("{}{path}", state.daemon_url.trim_end_matches('/'));
    match state.http_json.get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            let bytes = resp.bytes().await.unwrap_or_default();
            (
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                [(header::CONTENT_TYPE, "application/json")],
                bytes,
            )
                .into_response()
        }
        Err(err) => (StatusCode::BAD_GATEWAY, daemon_unreachable_body(&err)).into_response(),
    }
}

/// JSON POST passthrough helper for small daemon endpoints.
async fn proxy_post_json(state: &AppState, path: &str, body: Bytes) -> Response {
    let url = format!("{}{path}", state.daemon_url.trim_end_matches('/'));
    match state
        .http_json
        .post(&url)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.to_vec())
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            let bytes = resp.bytes().await.unwrap_or_default();
            (
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                [(header::CONTENT_TYPE, "application/json")],
                bytes,
            )
                .into_response()
        }
        Err(err) => (StatusCode::BAD_GATEWAY, daemon_unreachable_body(&err)).into_response(),
    }
}

/// Reverse-proxy GET /v1/models (model picker catalogue).
async fn proxy_models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    proxy_get_json(&state, "/v1/models").await
}

/// Reverse-proxy GET /v1/agents (named agent identity picker, TASK-9/TASK-11).
async fn proxy_agents(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    proxy_get_json(&state, "/v1/agents").await
}

/// Reverse-proxy GET /v1/fs/dirs?path=<path> (filesystem directory listing).
/// Forwards the full query string so `?path=~/dev` reaches the daemon intact.
async fn proxy_fs_dirs(State(state): State<Arc<AppState>>, req: Request) -> impl IntoResponse {
    let mut url = format!("{}/v1/fs/dirs", state.daemon_url.trim_end_matches('/'));
    if let Some(qs) = req.uri().query() {
        url.push('?');
        url.push_str(qs);
    }
    match state.http_json.get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            let bytes = resp.bytes().await.unwrap_or_default();
            (
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                [(header::CONTENT_TYPE, "application/json")],
                bytes,
            )
                .into_response()
        }
        Err(err) => (StatusCode::BAD_GATEWAY, daemon_unreachable_body(&err)).into_response(),
    }
}

/// Reverse-proxy GET /v1/model (current selection).
async fn proxy_model_get(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    proxy_get_json(&state, "/v1/model").await
}

/// Reverse-proxy POST /v1/model (hot-swap the model).
async fn proxy_model_set(State(state): State<Arc<AppState>>, body: Bytes) -> impl IntoResponse {
    proxy_post_json(&state, "/v1/model", body).await
}

/// Reverse-proxy GET /v1/projects (project list for the picker).
async fn proxy_projects_list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    proxy_get_json(&state, "/v1/projects").await
}

/// Reverse-proxy POST /v1/projects (create a project).
async fn proxy_projects_create(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> impl IntoResponse {
    proxy_post_json(&state, "/v1/projects", body).await
}

/// Reverse-proxy GET /v1/projects/{id} (project + its sessions).
async fn proxy_project_get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    proxy_get_json(&state, &format!("/v1/projects/{id}")).await
}

/// Reverse-proxy PATCH /v1/projects/{id} (update name/config).
async fn proxy_project_patch(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    proxy_method_json(
        &state,
        reqwest::Method::PATCH,
        &format!("/v1/projects/{id}"),
        body,
    )
    .await
}

/// Reverse-proxy DELETE /v1/projects/{id}.
async fn proxy_project_delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    proxy_method_json(
        &state,
        reqwest::Method::DELETE,
        &format!("/v1/projects/{id}"),
        Bytes::new(),
    )
    .await
}

/// Reverse-proxy POST /v1/component/event (component interaction → daemon).
async fn proxy_component_event(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> impl IntoResponse {
    proxy_post_json(&state, "/v1/component/event", body).await
}

/// Reverse-proxy POST /v1/calls/place (outbound call → daemon).
async fn proxy_call_place(State(state): State<Arc<AppState>>, body: Bytes) -> impl IntoResponse {
    proxy_post_json(&state, CALL_PLACE_DAEMON_PATH, body).await
}
/// Reverse-proxy POST /v1/voice/realtime/client-secret (ephemeral OpenAI
/// Realtime token mint → daemon; the key never reaches the browser).
async fn proxy_realtime_client_secret(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> impl IntoResponse {
    proxy_post_json(&state, "/v1/voice/realtime/client-secret", body).await
}

/// Reverse-proxy POST /v1/agent/sessions/{id}/messages (voice-agent handoff
/// note appended to a chat session → daemon).
async fn proxy_session_message_append(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    proxy_post_json(&state, &format!("/v1/agent/sessions/{id}/messages"), body).await
}

/// Reverse-proxy POST /v1/rooms/{room_id}/livekit-token.
async fn proxy_livekit_token(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    proxy_post_json(&state, &livekit_token_daemon_path(&room_id), body).await
}

/// JSON passthrough for an arbitrary method (PATCH/DELETE), mirroring
/// proxy_post_json but with the verb supplied.
async fn proxy_method_json(
    state: &AppState,
    method: reqwest::Method,
    path: &str,
    body: Bytes,
) -> Response {
    let url = format!("{}{path}", state.daemon_url.trim_end_matches('/'));
    match state
        .http_json
        .request(method, &url)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.to_vec())
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            let bytes = resp.bytes().await.unwrap_or_default();
            (
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                [(header::CONTENT_TYPE, "application/json")],
                bytes,
            )
                .into_response()
        }
        Err(err) => (StatusCode::BAD_GATEWAY, daemon_unreachable_body(&err)).into_response(),
    }
}

/// Reverse-proxy POST /v1/requests/{id}/cancel (halt a running turn).
async fn proxy_cancel(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    proxy_post_json(&state, &format!("/v1/requests/{id}/cancel"), Bytes::new()).await
}

fn livekit_token_daemon_path(room_id: &str) -> String {
    format!(
        "/v1/rooms/{}/livekit-token",
        percent_encode_path_segment(room_id)
    )
}

/// CSP violation sink (TASK-74).
///
/// The report-only policy shipped in TASK-72 had nowhere to report, which made
/// it decorative: browsers logged violations to their own consoles and no
/// operator ever saw them. Enforcing `script-src` safely requires knowing what
/// a real policy would actually break — measure first, enforce second.
///
/// Deliberately minimal and defensive, because this endpoint is reachable by
/// anything that can reach the proxy:
/// - the body is capped and never parsed as trusted structure; we log the
///   fields we care about and drop the rest,
/// - it always answers 204 so a browser never retries or surfaces an error to
///   the operator,
/// - it logs at `info` (not `warn`) since violations are expected during the
///   measurement phase and should not read as incidents.
async fn csp_report(body: Bytes) -> impl IntoResponse {
    // A violation report is small; anything larger is not a browser.
    const MAX: usize = 16 * 1024;
    if body.len() > MAX {
        return StatusCode::NO_CONTENT;
    }
    match serde_json::from_slice::<Value>(&body) {
        Ok(report) => {
            // Both the legacy `csp-report` envelope and the newer flat shape.
            let r = report.get("csp-report").unwrap_or(&report);
            tracing::info!(
                directive = %r.get("effective-directive")
                    .or_else(|| r.get("violated-directive"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("?"),
                blocked = %r.get("blocked-uri").and_then(|v| v.as_str()).unwrap_or("?"),
                document = %r.get("document-uri").and_then(|v| v.as_str()).unwrap_or("?"),
                "csp violation (report-only)"
            );
        }
        Err(_) => {
            tracing::info!(bytes = body.len(), "csp violation report: unparseable body");
        }
    }
    StatusCode::NO_CONTENT
}

/// Opaque 502 body for an unreachable daemon (TASK-73).
///
/// A `reqwest::Error`'s `Display` includes the full upstream URL, so the old
/// `format!("daemon unreachable: {err}")` handed every caller the daemon's
/// bind address, port, and internal path shape. That is behind auth, but an
/// auth boundary should not narrate its own topology. The detail goes to the
/// log — where operators can actually use it — and the client gets a fixed
/// string.
fn daemon_unreachable_body(err: &reqwest::Error) -> &'static str {
    tracing::warn!(error = %err, "daemon unreachable");
    "daemon unreachable"
}

/// Timeout for buffered JSON forwards (TASK-73). Generous enough that a slow
/// but working daemon still answers — a cold model call can take a while —
/// while bounding the case the audit found: a WEDGED daemon previously hung
/// every JSON passthrough forever, because they all shared the untimed client
/// that SSE legitimately requires.
const JSON_FORWARD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Length-independent byte comparison (TASK-73).
///
/// Returns false for a length mismatch, but reads BOTH inputs fully before
/// answering so the work done is a function of the inputs' sizes rather than
/// of how many leading bytes happened to match. This is deliberately a small
/// local helper rather than a new dependency: the proxy has one credential
/// comparison, and adding a crypto crate to a boundary binary for six lines
/// is a worse trade than the six lines.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        // Still touch both, so a length probe is not obviously cheaper.
        let mut sink = 0u8;
        for byte in a.iter().chain(b.iter()) {
            sink |= *byte;
        }
        return std::hint::black_box(sink) == 0 && false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// True when a client-supplied path tail contains a dot segment (`.` or `..`).
///
/// The wildcard forwarders build their upstream URL by string-formatting a
/// client-controlled tail into it. `reqwest`'s `Url::parse` then applies RFC
/// 3986 dot-segment removal, so a tail carrying `..` COLLAPSES the traversal
/// and reaches a daemon path the proxy's route table never exposed (TASK-71,
/// probe-confirmed: `/v1/rooms/persistent/../../../v1/agent/turns` arrived at
/// the daemon as `/v1/agent/turns`). That matters more here than in a normal
/// reverse proxy: the daemon behind this boundary has NO auth of its own and
/// runs with tool execution ungated, so the proxy's route table is the real
/// allow-list of internet-reachable surface. A traversal makes it advisory.
///
/// TASK-82: this guard DECODES each segment itself rather than trusting the
/// caller to hand it a decoded value.
///
/// The original version documented "call this on the DECODED tail" and one of
/// its two call sites then passed the raw request path. `%2e%2e` is not
/// literally `..`, so it sailed through the guard — and the `url` crate
/// percent-decodes BEFORE applying RFC 3986 dot-segment removal, so the
/// traversal collapsed upstream anyway. Confirmed live: raw `..` returned 400
/// while `%2e%2e` reached the daemon with 200.
///
/// A rule that depends on every caller passing the right form is a rule that
/// eventually meets a caller who doesn't. Decoding here makes the guard
/// correct on BOTH raw and encoded input, so neither call site can hold it
/// wrong.
///
/// Percent-decoding is done per segment and is deliberately tolerant of
/// malformed escapes (a stray `%` is kept literally) — this is a security
/// check, not a parser, and anything it cannot decode it must not silently
/// treat as safe. Double-encoded input (`%252e`) decodes to the literal text
/// `%2e`, which is not a dot segment and is also not what the upstream `url`
/// crate will collapse (it decodes once), so single-pass decoding matches
/// upstream behavior exactly.
///
/// Segments are split on `/` so a legitimate id containing dots (`room.v2`)
/// is unaffected; only a segment that IS `.` or `..` after decoding is refused.
fn has_dot_segment(path: &str) -> bool {
    path.split('/')
        .any(|seg| matches!(decode_segment(seg).as_str(), "." | ".."))
}

/// Single-pass percent-decode of one path segment, for [`has_dot_segment`].
/// Invalid escapes are preserved literally rather than dropped, so a
/// malformed input can never decode into something shorter that looks safe.
fn decode_segment(seg: &str) -> String {
    let bytes = seg.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn percent_encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                use std::fmt::Write as _;
                write!(encoded, "%{byte:02X}").expect("writing to string should not fail");
            }
        }
    }
    encoded
}

/// Reverse-proxy the daemon's `/v1/longhouse/*` control endpoints (e.g.
/// `demo`, `convene`). Forwards method, path tail, query, and body so the
/// deck can drive a council through this same origin. The resulting council
/// events arrive on the existing `/v1/agent/events` SSE stream.
async fn proxy_longhouse(
    State(state): State<Arc<AppState>>,
    Path(rest): Path<String>,
    req: Request,
) -> impl IntoResponse {
    // TASK-71: `rest` is the DECODED wildcard capture, so `%2e%2e` is already
    // `..` by the time we see it. Refuse dot segments before they can collapse
    // into a daemon path this route was never meant to reach.
    if has_dot_segment(&rest) {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    }
    let method = req.method().clone();
    let q = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let url = format!(
        "{}/v1/longhouse/{rest}{q}",
        state.daemon_url.trim_end_matches('/')
    );
    // buffer the (small) body so we can forward it on POST
    // TASK-73: a body over the cap previously became an EMPTY forwarded
    // request via unwrap_or_default() — a truncation that presents upstream as
    // a legitimate call. Refuse it instead.
    let body = match axum::body::to_bytes(req.into_body(), 1 << 20).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return (StatusCode::PAYLOAD_TOO_LARGE, "request body too large").into_response();
        }
    };
    let builder = if method == axum::http::Method::POST {
        state
            .http_json
            .post(&url)
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.to_vec())
    } else {
        state.http_json.get(&url)
    };
    match builder.send().await {
        Ok(resp) => {
            let status = resp.status();
            let bytes = resp.bytes().await.unwrap_or_default();
            (
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                [(header::CONTENT_TYPE, "application/json")],
                bytes,
            )
                .into_response()
        }
        Err(err) => (StatusCode::BAD_GATEWAY, daemon_unreachable_body(&err)).into_response(),
    }
}

/// Reverse-proxy the daemon's persistent-rooms API (`/v1/rooms/persistent`
/// and everything under it). Mirrors `proxy_longhouse` — forwards the method,
/// full path, query string, and body — but also handles DELETE (leave a room:
/// `DELETE /v1/rooms/persistent/{key}/participants/{id}`). One handler serves
/// the whole subtree: list (GET) + create (POST) on the bare path, plus room
/// get (GET), join (POST), leave (DELETE), post-message (POST), transcript
/// (GET, `?after_seq=`), and the live event tail (GET, exact `{key}/events`
/// shape — streamed via sse_stream_response with Last-Event-ID resume).
async fn proxy_rooms_persistent(
    State(state): State<Arc<AppState>>,
    req: Request,
) -> impl IntoResponse {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    // TASK-71: this handler forwards the RAW request path verbatim, so a `..`
    // segment would collapse upstream into an unproxied daemon route. Refuse
    // before the SSE branch below, so both the streaming and buffered paths
    // are covered by one check.
    if has_dot_segment(&path) {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    }
    let q = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();

    // TASK-11: GET paths that match the exact shape
    // `/v1/rooms/persistent/{key}/events` (exactly one key segment before
    // `/events`) must stream through sse_stream_response rather than buffering
    // (axum rejects a separate {key}/events route alongside {*rest}).
    // We reconstruct this from path segments to avoid a loose ends_with.
    let is_events_tail = method == axum::http::Method::GET && {
        let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        segments.len() == 5
            && segments[0] == "v1"
            && segments[1] == "rooms"
            && segments[2] == "persistent"
            && segments[4] == "events"
            && !segments[3].is_empty()
    };
    if is_events_tail {
        let url = format!("{}{path}{q}", state.daemon_url.trim_end_matches('/'));
        let mut upstream = state.http.get(&url);
        if let Some(last_id) = req.headers().get("last-event-id") {
            if let Ok(val) = last_id.to_str() {
                upstream = upstream.header("Last-Event-ID", val);
            }
        }
        return match upstream.send().await {
            Ok(resp) => sse_stream_response(resp),
            Err(err) => (StatusCode::BAD_GATEWAY, daemon_unreachable_body(&err)).into_response(),
        };
    }

    // The path is always under /v1/rooms/persistent (the only routes wired to
    // this handler); forward it unchanged, with the query string preserved so
    // the transcript tail's ?after_seq= reaches the daemon.
    let url = format!("{}{path}{q}", state.daemon_url.trim_end_matches('/'));
    // buffer the (small) body so we can forward it on POST/DELETE
    // TASK-73: a body over the cap previously became an EMPTY forwarded
    // request via unwrap_or_default() — a truncation that presents upstream as
    // a legitimate call. Refuse it instead.
    let body = match axum::body::to_bytes(req.into_body(), 1 << 20).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return (StatusCode::PAYLOAD_TOO_LARGE, "request body too large").into_response();
        }
    };
    let builder = if method == axum::http::Method::GET {
        state.http_json.get(&url)
    } else {
        state
            .http_json
            .request(method, &url)
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.to_vec())
    };
    match builder.send().await {
        Ok(resp) => {
            let status = resp.status();
            let bytes = resp.bytes().await.unwrap_or_default();
            (
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                [(header::CONTENT_TYPE, "application/json")],
                bytes,
            )
                .into_response()
        }
        Err(err) => (StatusCode::BAD_GATEWAY, daemon_unreachable_body(&err)).into_response(),
    }
}

/// Build a streaming SSE response from an upstream reqwest `Response`, forwarding
/// the status + the upstream `text/event-stream` body **immediately** (header
/// flush happens as soon as this response is returned — before any body byte) and
/// stamping the explicit no-buffer hints so the proxy AND Cloudflare flush the
/// stream at t=0 instead of buffering it.
///
/// The buffer point that broke the deployed app was NOT the proxy (it already
/// streams via `bytes_stream()` → `Body::from_stream`, which forwards chunks as
/// they arrive). It was the CDN: Cloudflare buffered the `text/event-stream`
/// response ~15s before flushing headers, so the browser's `EventSource` sat in
/// CONNECTING and chat replies never arrived. The cure is the explicit signals
/// every nginx/CDN honors:
///   - `X-Accel-Buffering: no`  → "do not buffer this stream, flush now"
///   - `Cache-Control: no-cache, no-transform` → don't cache, don't buffer-to-
///     compress (no-transform stops Cloudflare from holding the stream to gzip it)
fn sse_stream_response(resp: reqwest::Response) -> Response {
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::OK);
    let upstream_headers = resp.headers().clone();
    let mut headers = sse_no_buffer_headers();
    for name in [
        header::PRAGMA,
        header::EXPIRES,
        HeaderName::from_static("x-observatory-cursor"),
        HeaderName::from_static("x-observatory-instance"),
    ] {
        if let Some(value) = upstream_headers.get(&name) {
            headers.insert(name, value.clone());
        }
    }
    // Pipe the upstream byte stream into the response body unchanged so deltas
    // arrive in real time.
    let stream = resp.bytes_stream();
    let body = axum::body::Body::from_stream(stream);
    (status, headers, body).into_response()
}

/// The header set that makes an SSE response flush immediately end-to-end:
///   - `Content-Type: text/event-stream` — the stream content type Cloudflare
///     auto-recognizes and (usually) won't buffer.
///   - `Cache-Control: no-cache, no-transform` — don't cache; `no-transform`
///     stops Cloudflare from holding the stream to gzip it.
///   - `X-Accel-Buffering: no` — the canonical "do not buffer this stream" hint
///     nginx and Cloudflare honor, which forces the headers to flush at t=0.
fn sse_no_buffer_headers() -> axum::http::HeaderMap {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache, no-transform"),
    );
    headers.insert(
        HeaderName::from_static("x-accel-buffering"),
        axum::http::HeaderValue::from_static("no"),
    );
    headers
}

/// Reverse-proxy the daemon's CONTROL stream `GET /v1/events` (OCEAN-135).
/// This is the SSE channel that carries `permission_request` cards. Mirrors
/// `proxy_events` exactly — streams the upstream body straight through and
/// forwards the full query string (e.g. ?session_id=) — but for the
/// control-plane path. Without it the web UI's control subscription 404'd and
/// permission cards never reached the phone, hanging every gated turn.
async fn proxy_control_events(
    State(state): State<Arc<AppState>>,
    req: Request,
) -> impl IntoResponse {
    let q = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let url = format!("{}/v1/events{q}", state.daemon_url.trim_end_matches('/'));
    match state.http.get(&url).send().await {
        Ok(resp) => sse_stream_response(resp),
        Err(err) => (StatusCode::BAD_GATEWAY, daemon_unreachable_body(&err)).into_response(),
    }
}

/// Reverse-proxy GET /v1/permissions (permission snapshot). The web UI polls
/// this to render pending-permission cards; without it the request fell through
/// to ServeDir → 404 (empty body) → "permission snapshot rejected".
async fn proxy_permissions_snapshot(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    proxy_get_json(&state, "/v1/permissions").await
}

/// Reverse-proxy POST /v1/permissions/{id}/decision (OCEAN-136). The web UI
/// answers a permission prompt here (Allow/Deny); the body matches the
/// daemon's decision payload. Forwards the {id} + body so the decision reaches
/// the daemon — without it Allow/Deny 404'd and the gated turn stayed stuck.
async fn proxy_permission_decision(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    proxy_post_json(&state, &format!("/v1/permissions/{id}/decision"), body).await
}

/// Reverse-proxy the daemon's SSE event stream. We stream the upstream body
/// straight through so deltas arrive in real time.
async fn proxy_events(State(state): State<Arc<AppState>>, req: Request) -> impl IntoResponse {
    // Preserve ?session_id= query string — scopes SSE to one session per
    // OCEAN_ECOSYSTEM_CONTRACT.md. Do not strip. The full upstream query is
    // forwarded verbatim so session_id (and any other params like ?all=1)
    // reaches the daemon and the stream stays scoped to the caller's session.
    let q = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let url = format!(
        "{}/v1/agent/events{q}",
        state.daemon_url.trim_end_matches('/')
    );
    match state.http.get(&url).send().await {
        Ok(resp) => sse_stream_response(resp),
        Err(err) => (StatusCode::BAD_GATEWAY, daemon_unreachable_body(&err)).into_response(),
    }
}

/// Reverse-proxy all read-only Observatory routes with the current daemon-minted
/// credential. Snapshot/replay are buffered JSON; events remains an unbuffered
/// SSE byte stream with Last-Event-ID resume preserved end to end.
async fn proxy_observatory(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let token = match read_observer_token(&state.observer_token_path) {
        Ok(token) => token,
        Err(error) => {
            tracing::warn!(%error, path = %state.observer_token_path.display(), "observatory credential unavailable");
            // TASK-73: the error stringifies io::Error, which carries the FULL
            // filesystem path of the credential file. Log it, never ship it.
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "observatory credential unavailable",
            )
                .into_response();
        }
    };
    let path = req.uri().path();
    let query = req
        .uri()
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let url = format!("{}{path}{query}", state.daemon_url.trim_end_matches('/'));
    // TASK-83: this one handler serves BOTH an SSE tail (`/events`) and
    // buffered routes (`/snapshot`, `/replay`), so the client must be chosen
    // by route shape rather than swapped wholesale. The streaming tail keeps
    // the untimed client — a timeout there would sever a live session — while
    // the buffered routes get the bounded one, which is what TASK-73 intended
    // and missed here because the choice happens before the branch below.
    let is_stream = path.ends_with("/events");
    let client = if is_stream {
        &state.http
    } else {
        &state.http_json
    };
    let mut upstream = client.get(&url).bearer_auth(token);
    if let Some(last_event_id) = req.headers().get("last-event-id") {
        if let Ok(last_event_id) = last_event_id.to_str() {
            upstream = upstream.header("Last-Event-ID", last_event_id);
        }
    }

    let response = match upstream.send().await {
        Ok(response) => response,
        Err(error) => {
            return (StatusCode::BAD_GATEWAY, daemon_unreachable_body(&error)).into_response();
        }
    };
    if is_stream {
        return sse_stream_response(response);
    }

    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let upstream_headers = response.headers().clone();
    let body = response.bytes().await.unwrap_or_default();
    let mut response = (status, body).into_response();
    for name in [
        header::CONTENT_TYPE,
        header::CACHE_CONTROL,
        header::PRAGMA,
        header::EXPIRES,
        HeaderName::from_static("x-observatory-cursor"),
        HeaderName::from_static("x-observatory-instance"),
    ] {
        if let Some(value) = upstream_headers.get(&name) {
            response.headers_mut().insert(name, value.clone());
        }
    }
    response
}

/// POST /api/stt — forward raw audio bytes to the daemon's voice STT endpoint.
/// The daemon holds the xAI key and handles multipart construction.
/// Returns `{ok, text}` on success, `{ok: false, error}` on failure.
async fn stt(State(state): State<Arc<AppState>>, body: Bytes) -> impl IntoResponse {
    let url = format!("{}/v1/voice/stt", state.daemon_url.trim_end_matches('/'));

    // TASK-83: buffered (the response is read to completion via `.json()`),
    // so it belongs on the timed client. It was left on the untimed SSE
    // client when TASK-73 split them, so a wedged daemon hung dictation
    // forever instead of failing.
    let resp = match state
        .http_json
        .post(&url)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(body.to_vec())
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(err) => {
            tracing::error!(error = %err, "stt daemon unreachable");
            return Json(json!({ "ok": false, "error": "stt daemon unreachable" })).into_response();
        }
    };

    let status = resp.status();

    // Tolerate non-JSON bodies from the daemon (a 502 gateway error etc).
    let payload: Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => {
            return Json(json!({ "ok": false, "error": "stt daemon returned invalid JSON" }))
                .into_response();
        }
    };

    translate_stt_daemon_response(status, &payload)
}

/// Translate a daemon STT response into the browser-facing `{ok, text}` or
/// `{ok: false, error}` JSON shape.
fn translate_stt_daemon_response(status: StatusCode, payload: &Value) -> Response {
    if status.is_success() {
        let text = payload
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        Json(json!({ "ok": true, "text": text })).into_response()
    } else {
        let error = payload
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("stt_failed");
        tracing::error!(%status, ?payload, "stt daemon error");
        Json(json!({ "ok": false, "error": error })).into_response()
    }
}

#[derive(Deserialize)]
struct TtsRequest {
    text: String,
}

/// POST /api/tts — forward `{text}` to the daemon's voice TTS endpoint.
/// The daemon holds the xAI key and handles upstream construction.
/// Forwards `voice` from the configured profile so the daemon applies it.
async fn tts(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TtsRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let text = req.text.trim().to_string();
    if text.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "text required".to_string()));
    }

    let url = format!("{}/v1/voice/tts", state.daemon_url.trim_end_matches('/'));

    // TASK-83: buffered (`.bytes()` below) — same miss as stt.
    let resp = state
        .http_json
        .post(&url)
        .header(header::CONTENT_TYPE, "application/json")
        .json(&json!({
            "text": text,
            "voice": state.voice_profile,
        }))
        .send()
        .await
        .map_err(|err| {
            tracing::error!(error = %err, "tts daemon unreachable");
            (
                StatusCode::BAD_GATEWAY,
                "tts daemon unreachable".to_string(),
            )
        })?;

    let status = resp.status();
    if !status.is_success() {
        // Try to extract a daemon error JSON; fall back to the body text.
        let body = resp.text().await.unwrap_or_default();
        let err_msg = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v.get("error").and_then(|v| v.as_str()).map(str::to_string))
            .unwrap_or(body);
        tracing::error!(%status, %err_msg, "tts daemon error");
        let code = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        return Err((code, format!("tts_failed: {err_msg}")));
    }

    // Forward audio bytes with whatever content-type the daemon returned
    // (audio/mpeg for xAI). If no content-type, default to audio/mpeg.
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("audio/mpeg")
        .to_string();

    let audio = resp
        .bytes()
        .await
        .map_err(|err| (StatusCode::BAD_GATEWAY, format!("tts read failed: {err}")))?;

    Ok(([(header::CONTENT_TYPE, content_type)], audio))
}

#[cfg(test)]
mod tests {
    use super::{
        build_app, config_payload, constant_time_eq, decode_segment, has_dot_segment,
        is_hashed_asset, livekit_token_daemon_path, percent_encode_path_segment,
        read_observer_token, session_auth_gate, sse_no_buffer_headers, wasm_headers, AppState,
        CALL_PLACE_DAEMON_PATH, WASM_CACHE_CONTROL,
    };
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
        middleware,
        routing::{get, post},
        Router,
    };
    use base64::Engine;
    use tower::ServiceExt; // for `oneshot`

    /// Build a router that returns a tiny body for any path, wrapped in the
    /// `wasm_headers` layer, so we can assert the layer's header rewriting per
    /// request path without touching the real ServeDir.
    fn wasm_test_router() -> Router {
        Router::new()
            .fallback(get(|| async {
                // Simulate ServeDir's default: a 200 with some other content-type.
                (
                    [(header::CONTENT_TYPE, "application/octet-stream")],
                    Body::from(vec![0x00, 0x61, 0x73, 0x6d]),
                )
            }))
            .layer(middleware::from_fn(wasm_headers))
    }

    fn auth_test_state() -> Arc<AppState> {
        Arc::new(AppState {
            http: reqwest::Client::new(),
            http_json: reqwest::Client::new(),
            voice_profile: "leo".to_string(),
            daemon_url: "http://127.0.0.1:4780".to_string(),
            default_livekit_room_id: "project:surface-test".to_string(),
            tldraw_sync_uri: None,
            maps_key: None,
            maps_map_id: "DEMO_MAP_ID".to_string(),
            basic_auth: Some(("ocean".to_string(), "surface".to_string())),
            session_token: "test-session".to_string(),
            observer_token_path: PathBuf::from("/not-used-in-auth-tests"),
        })
    }

    #[test]
    fn observer_token_reader_requires_mode_0600_regular_file() {
        let directory = tempfile::tempdir().expect("tempdir");
        let token_path = directory.path().join("observatory-token");
        std::fs::write(&token_path, "signed-token\n").expect("write token");
        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600))
            .expect("chmod");
        assert_eq!(
            read_observer_token(&token_path).expect("read token"),
            "signed-token"
        );

        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o644))
            .expect("chmod unsafe");
        assert!(read_observer_token(&token_path).is_err());
    }

    fn auth_gate_test_router() -> Router {
        let state = auth_test_state();

        Router::new()
            .route("/", get(|| async { StatusCode::OK }))
            .route("/v1/agent/turns", post(|| async { StatusCode::OK }))
            .fallback(get(|| async { StatusCode::OK }))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                session_auth_gate,
            ))
            .with_state(state)
    }

    fn request(path: &str) -> Request<Body> {
        Request::builder()
            .uri(path)
            .body(Body::empty())
            .expect("test request must be valid")
    }

    fn valid_basic_auth_header() -> String {
        let encoded = base64::engine::general_purpose::STANDARD.encode("ocean:surface");
        format!("Basic {encoded}")
    }

    #[tokio::test]
    async fn unauthenticated_navigation_redirects_to_login_without_basic_challenge() {
        let resp = auth_gate_test_router()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::ACCEPT, "text/html")
                    .body(Body::empty())
                    .expect("test request must be valid"),
            )
            .await
            .expect("router should respond");

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/login");
        assert!(resp.headers().get(header::WWW_AUTHENTICATE).is_none());
    }

    #[tokio::test]
    async fn unauthenticated_api_gets_plain_401_without_basic_challenge() {
        let resp = auth_gate_test_router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/agent/turns")
                    .body(Body::empty())
                    .expect("test request must be valid"),
            )
            .await
            .expect("router should respond");

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(resp.headers().get(header::WWW_AUTHENTICATE).is_none());
    }

    #[tokio::test]
    async fn session_cookie_allows_authenticated_request() {
        let resp = auth_gate_test_router()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::COOKIE, "other=x; ocean_session=test-session")
                    .body(Body::empty())
                    .expect("test request must be valid"),
            )
            .await
            .expect("router should respond");

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn basic_auth_allows_valid_credentials_for_root() {
        let resp = auth_gate_test_router()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::AUTHORIZATION, valid_basic_auth_header())
                    .body(Body::empty())
                    .expect("test request must be valid"),
            )
            .await
            .expect("router should respond");

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn basic_auth_allows_static_pwa_assets_without_credentials() {
        let cases = [
            "/ocean-surface-ui-0123456789abcdef_bg.wasm",
            "/index-0123456789abcdef.js",
            "/tailwind-0123456789abcdef.css",
            "/manifest.webmanifest",
            "/sw.js",
            "/icon-192.png",
            "/brand/ocean-mark.svg",
            "/fonts/inter-var.woff2",
        ];
        let mut challenged = Vec::new();

        for path in cases {
            let resp = auth_gate_test_router()
                .oneshot(request(path))
                .await
                .expect("router should respond");

            if resp.status() != StatusCode::OK {
                challenged.push((path, resp.status()));
            }
        }

        assert!(
            challenged.is_empty(),
            "static PWA assets must bypass Basic auth; challenged responses: {challenged:?}"
        );
    }

    #[tokio::test]
    async fn wasm_response_gets_application_wasm_and_allows_compression() {
        let resp = wasm_test_router()
            .oneshot(
                Request::builder()
                    .uri("/ocean-surface-ui-abc123.wasm")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/wasm"
        );
        let cc = resp.headers().get(header::CACHE_CONTROL).unwrap();
        assert_eq!(cc, WASM_CACHE_CONTROL);
        // Compression must be ALLOWED: `no-transform` would forbid the CDN
        // from content-encoding the (large) module. SRI is disabled at build
        // time, so a transformed transfer can no longer abort the preload.
        assert!(!cc.to_str().unwrap().contains("no-transform"));
        assert!(cc.to_str().unwrap().contains("immutable"));
    }

    #[tokio::test]
    async fn non_wasm_response_is_left_untouched() {
        let resp = wasm_test_router()
            .oneshot(
                Request::builder()
                    .uri("/index.html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        // Untouched: the layer must not rewrite non-wasm content types or add
        // the immutable cache policy to e.g. index.html.
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/octet-stream"
        );
        assert!(resp.headers().get(header::CACHE_CONTROL).is_none());
    }

    #[test]
    fn sse_responses_carry_the_no_buffer_headers() {
        // These three headers are what kill the 15s Cloudflare buffering hang on
        // /v1/agent/events + /v1/events: stream content type, no-transform so the
        // CDN doesn't hold the stream to compress it, and the explicit
        // X-Accel-Buffering: no flush hint nginx/Cloudflare honor.
        let headers = sse_no_buffer_headers();
        assert_eq!(
            headers.get(axum::http::header::CONTENT_TYPE).unwrap(),
            "text/event-stream"
        );
        assert_eq!(
            headers.get(axum::http::header::CACHE_CONTROL).unwrap(),
            "no-cache, no-transform"
        );
        assert_eq!(
            headers.get("x-accel-buffering").unwrap(),
            "no",
            "X-Accel-Buffering: no must be present so Cloudflare flushes SSE headers at t=0"
        );
    }

    #[test]
    fn hashed_assets_are_immutable_but_shell_is_not() {
        // Content-hashed build artifacts → immutable.
        assert!(is_hashed_asset("/index-1a2b3c4d5e.js"));
        assert!(is_hashed_asset("/ocean-surface-ui-1a2b3c4d5e_bg.wasm"));
        assert!(is_hashed_asset("/style-deadbeef99.css"));
        assert!(is_hashed_asset("/assets/app-0123456789abcdef.js"));
        // Shell, worker, manifest, icons → NOT immutable (must revalidate).
        assert!(!is_hashed_asset("/sw.js"));
        assert!(!is_hashed_asset("/"));
        assert!(!is_hashed_asset("/index.html"));
        assert!(!is_hashed_asset("/manifest.webmanifest"));
        assert!(!is_hashed_asset("/icon-192.png"));
        // Too-short / non-hex tails are not treated as hashed.
        assert!(!is_hashed_asset("/vendor-lib.js"));
        assert!(!is_hashed_asset("/index-1a2b.js"));
    }

    #[test]
    fn call_place_proxy_path_matches_daemon_endpoint() {
        assert_eq!(CALL_PLACE_DAEMON_PATH, "/v1/calls/place");
    }

    #[test]
    fn livekit_token_proxy_path_preserves_room_id_as_single_segment() {
        assert_eq!(
            livekit_token_daemon_path("project:surface-demo"),
            "/v1/rooms/project%3Asurface-demo/livekit-token"
        );
        assert_eq!(
            livekit_token_daemon_path("project/surface demo"),
            "/v1/rooms/project%2Fsurface%20demo/livekit-token"
        );
    }

    #[test]
    fn path_segment_encoder_leaves_safe_url_bytes_unescaped() {
        assert_eq!(
            percent_encode_path_segment("abc-XYZ_123.~"),
            "abc-XYZ_123.~"
        );
    }

    #[test]
    fn config_payload_includes_surface_collaboration_defaults() {
        let state = AppState {
            http: reqwest::Client::new(),
            http_json: reqwest::Client::new(),
            voice_profile: "leo".to_string(),
            daemon_url: "http://127.0.0.1:4780".to_string(),
            default_livekit_room_id: "project/surface demo".to_string(),
            tldraw_sync_uri: Some("http://127.0.0.1:5858/connect".to_string()),
            maps_key: Some("maps".to_string()),
            maps_map_id: "DEMO_MAP_ID".to_string(),
            basic_auth: None,
            session_token: "test-session".to_string(),
            observer_token_path: PathBuf::from("/not-used-in-config-tests"),
        };

        let payload = config_payload(&state);

        assert_eq!(payload["daemon_url"], "");
        assert_eq!(payload["has_auth"], true);
        assert_eq!(payload["livekit_room_id"], "project/surface demo");
        assert_eq!(
            payload["livekit_token_path"],
            "/v1/rooms/project%2Fsurface%20demo/livekit-token"
        );
        assert_eq!(payload["tldraw_sync_uri"], "http://127.0.0.1:5858/connect");
        assert_eq!(
            payload["surface"]["livekit_token_path"],
            "/v1/rooms/project%2Fsurface%20demo/livekit-token"
        );
    }

    // ── translate_stt_daemon_response unit tests ──

    #[test]
    fn translate_stt_success_extracts_text() {
        use super::translate_stt_daemon_response;
        use axum::http::StatusCode;
        let payload = serde_json::json!({"text": "hello world"});
        let resp = translate_stt_daemon_response(StatusCode::OK, &payload);
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn translate_stt_success_handles_missing_text() {
        use super::translate_stt_daemon_response;
        use axum::http::StatusCode;
        let payload = serde_json::json!({});
        let resp = translate_stt_daemon_response(StatusCode::OK, &payload);
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn translate_stt_daemon_error_json() {
        use super::translate_stt_daemon_response;
        use axum::http::StatusCode;
        let payload = serde_json::json!({"error": "credential unavailable"});
        let resp = translate_stt_daemon_response(StatusCode::SERVICE_UNAVAILABLE, &payload);
        // The proxy always returns 200 for handled errors to keep the client contract simple.
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn translate_stt_daemon_bad_gateway() {
        use super::translate_stt_daemon_response;
        use axum::http::StatusCode;
        let payload = serde_json::json!({"error": "upstream returned 500"});
        let resp = translate_stt_daemon_response(StatusCode::BAD_GATEWAY, &payload);
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// TASK-74: the CSP report sink must exist in the PRODUCTION router, must
    /// be reachable WITHOUT credentials (a browser posting a violation report
    /// does not carry the operator's basic auth), and must always answer 204
    /// so a browser never retries or surfaces an error. Drives `build_app`.
    #[tokio::test]
    async fn csp_report_endpoint_accepts_and_swallows() {
        let dist = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(AppState {
            http: reqwest::Client::new(),
            http_json: reqwest::Client::new(),
            voice_profile: "leo".to_string(),
            daemon_url: "http://127.0.0.1:9".to_string(),
            default_livekit_room_id: "project:surface-test".to_string(),
            tldraw_sync_uri: None,
            maps_key: None,
            maps_map_id: "DEMO_MAP_ID".to_string(),
            // Auth ON: a violation report must still land, otherwise the
            // report-only policy silently collects nothing in production.
            basic_auth: Some(("u".to_string(), "p".to_string())),
            session_token: "test-session".to_string(),
            observer_token_path: PathBuf::from("/not-used"),
        });
        let app = build_app(state, dist.path());

        // Legacy envelope, no credentials.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/csp-report")
                    .header(header::CONTENT_TYPE, "application/csp-report")
                    .body(Body::from(
                        r#"{"csp-report":{"effective-directive":"script-src","blocked-uri":"inline","document-uri":"https://x/"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NO_CONTENT,
            "violation reports must be accepted without auth",
        );

        // Garbage body must not 500 — a browser must never see an error here.
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/csp-report")
                    .body(Body::from("not json at all"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    /// TASK-73: the credential comparison must not short-circuit. This proves
    /// the SEMANTICS (right answer for every shape, including length
    /// mismatches and empty inputs) rather than attempting to time it —
    /// a timing assertion in a unit test would be flaky theatre.
    #[test]
    fn constant_time_eq_matches_equality_semantics() {
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"hunter2", b"hunter2"));
        assert!(!constant_time_eq(b"hunter2", b"hunter3"));
        assert!(
            !constant_time_eq(b"hunter2", b"hunter2x"),
            "length mismatch"
        );
        assert!(!constant_time_eq(b"", b"x"));
        assert!(!constant_time_eq(b"x", b""));
        // Differing in the FIRST byte and in the LAST byte must both be false;
        // a short-circuiting implementation gets these right too, but a broken
        // masking implementation (e.g. `&` instead of `|=`) would not.
        assert!(!constant_time_eq(b"aaaa", b"baaa"));
        assert!(!constant_time_eq(b"aaaa", b"aaab"));
    }

    /// TASK-73: error bodies must not narrate internal topology. The audit
    /// found 502s echoing the daemon's bind address and a 503 echoing the
    /// FULL filesystem path of the observer-token file.
    #[tokio::test]
    async fn daemon_unreachable_body_is_opaque() {
        let dist = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(AppState {
            http: reqwest::Client::new(),
            http_json: reqwest::Client::new(),
            voice_profile: "leo".to_string(),
            // Closed port → the forward fails and we see the real error body.
            daemon_url: "http://127.0.0.1:9".to_string(),
            default_livekit_room_id: "project:surface-test".to_string(),
            tldraw_sync_uri: None,
            maps_key: None,
            maps_map_id: "DEMO_MAP_ID".to_string(),
            basic_auth: None,
            session_token: "test-session".to_string(),
            observer_token_path: PathBuf::from("/not-used"),
        });
        let app = build_app(state, dist.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/permissions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .expect("read body");
        let text = String::from_utf8_lossy(&body);
        assert_eq!(text, "daemon unreachable");
        assert!(
            !text.contains("127.0.0.1") && !text.contains(':') && !text.contains("http"),
            "error body must not disclose the upstream address: {text}",
        );
    }

    /// TASK-83: every use of the UNTIMED client must be a genuine SSE tail.
    ///
    /// TASK-73 split the clients so a wedged daemon could not hang buffered
    /// forwards forever — but three buffered handlers (stt, tts, observatory
    /// snapshot/replay) were left on the untimed client, so the bug it
    /// claimed to close stayed open for them. The inverse mistake is worse:
    /// moving an SSE tail onto the timed client would SEVER live event
    /// streams mid-session.
    ///
    /// This pins the classification itself. Each untimed use must sit in a
    /// handler that hands its response to `sse_stream_response`; a new
    /// buffered forward on `state.http` fails here rather than shipping.
    #[test]
    fn untimed_client_is_used_only_by_streaming_handlers() {
        let src = include_str!("main.rs");
        // Needle assembled at runtime so it cannot match this test's own text.
        let untimed = format!("state.{}{}", "http", ".");
        let streaming_handlers = [
            "async fn proxy_rooms_persistent",
            "async fn proxy_control_events",
            "async fn proxy_events",
            "async fn proxy_observatory",
        ];

        // Collect the handler each untimed use falls inside.
        let mut offenders = Vec::new();
        for (idx, _) in src.match_indices(untimed.as_str()) {
            let before = &src[..idx];
            let handler = streaming_handlers
                .iter()
                .filter_map(|h| before.rfind(h).map(|pos| (pos, *h)))
                .max_by_key(|(pos, _)| *pos);
            // Also find the nearest preceding fn of ANY kind, so a use inside
            // a non-streaming fn is not attributed to an earlier streaming one.
            let nearest_fn = before.rfind("\nasync fn ").max(before.rfind("\nfn "));
            match (handler, nearest_fn) {
                (Some((hpos, _)), Some(fpos)) if hpos >= fpos => {}
                _ => {
                    let line = before.matches('\n').count() + 1;
                    offenders.push(line);
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "untimed client used outside a streaming handler at line(s) {offenders:?} — \
             a buffered forward there hangs forever on a wedged daemon (TASK-83). \
             Use state.http_json, or add the handler above if it truly streams.",
        );

        // And the observatory handler must pick its client by route shape,
        // since it serves both an SSE tail and buffered routes.
        assert!(
            src.contains("let is_stream = path.ends_with(\"/events\");"),
            "observatory must branch on route shape before choosing a client",
        );
    }

    /// TASK-82: the guard must catch dot segments in BOTH raw and encoded
    /// form, because the upstream `url` crate decodes before collapsing. It
    /// must NOT over-reject legitimate ids that merely contain dots, and it
    /// must treat double-encoding the way upstream does (one decode pass), so
    /// `%252e` stays the literal text `%2e` and is not a dot segment.
    #[test]
    fn dot_segment_guard_sees_through_percent_encoding() {
        // Raw.
        assert!(has_dot_segment("/a/../b"));
        assert!(has_dot_segment("/a/./b"));
        // Encoded — the case that shipped a live bypass.
        assert!(has_dot_segment("/a/%2e%2e/b"));
        assert!(has_dot_segment("/a/%2E%2E/b"));
        assert!(has_dot_segment("/a/%2e/b"));
        // Mixed raw/encoded within one segment.
        assert!(has_dot_segment("/a/.%2e/b"));
        assert!(has_dot_segment("/a/%2e./b"));
        // Legitimate ids must survive — a guard that breaks the feature is
        // not a fix.
        assert!(!has_dot_segment("/v1/rooms/persistent/room.v2/events"));
        assert!(!has_dot_segment("/v1/rooms/persistent/..config/events"));
        assert!(!has_dot_segment("/v1/rooms/persistent/a.b.c"));
        assert!(!has_dot_segment("/v1/agent/sessions"));
        // Double-encoded decodes to the literal "%2e", not a dot segment —
        // matching what the upstream url crate does with one decode pass.
        assert!(!has_dot_segment("/a/%252e%252e/b"));
        // Malformed escapes are preserved literally, never silently dropped
        // into something that looks safe.
        assert_eq!(decode_segment("%zz"), "%zz");
        assert_eq!(decode_segment("%2"), "%2");
        assert_eq!(decode_segment("%2e"), ".");
    }

    /// TASK-72: security headers must reach real document responses through
    /// the PRODUCTION router, and must NOT be pasted onto proxied API/SSE
    /// responses. Drives `build_app` so a middleware that gets dropped from
    /// the layer stack fails this test.
    #[tokio::test]
    async fn security_headers_cover_documents_and_skip_api() {
        let dist = tempfile::tempdir().expect("tempdir");
        std::fs::write(dist.path().join("index.html"), "<!doctype html>ok").expect("write shell");
        let state = Arc::new(AppState {
            http: reqwest::Client::new(),
            http_json: reqwest::Client::new(),
            voice_profile: "leo".to_string(),
            daemon_url: "http://127.0.0.1:9".to_string(),
            default_livekit_room_id: "project:surface-test".to_string(),
            tldraw_sync_uri: None,
            maps_key: None,
            maps_map_id: "DEMO_MAP_ID".to_string(),
            basic_auth: None,
            session_token: "test-session".to_string(),
            observer_token_path: PathBuf::from("/not-used"),
        });
        let app = build_app(state, dist.path());

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/index.html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let h = resp.headers();
        assert_eq!(
            h.get(header::X_CONTENT_TYPE_OPTIONS)
                .map(|v| v.to_str().unwrap()),
            Some("nosniff"),
        );
        assert_eq!(
            h.get(header::REFERRER_POLICY).map(|v| v.to_str().unwrap()),
            Some("no-referrer"),
        );
        let csp = h
            .get(header::CONTENT_SECURITY_POLICY)
            .expect("enforced CSP present")
            .to_str()
            .unwrap();
        assert!(
            csp.contains("frame-ancestors 'none'") && csp.contains("object-src 'none'"),
            "enforced CSP must carry the break-proof directives: {csp}",
        );
        assert!(
            !csp.contains("script-src"),
            "script-src must NOT be enforced while the shell has inline scripts — \
             it belongs in report-only until nonces land: {csp}",
        );
        assert!(
            h.get("content-security-policy-report-only").is_some(),
            "the aspirational policy must ship report-only so violations surface",
        );

        // A proxied API response must be left alone (JSON/SSE carry their own
        // headers; a document CSP there is meaningless and risks clobbering).
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/permissions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        assert!(
            resp.headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .is_none(),
            "API responses must not be decorated with a document CSP",
        );
    }

    /// TASK-71: dot segments in a wildcard forwarder's tail must be refused
    /// BEFORE the upstream URL is built, because `reqwest`'s `Url::parse`
    /// collapses them and would reach a daemon route this proxy never exposed.
    ///
    /// Drives the PRODUCTION router (`build_app`), not `has_dot_segment` alone:
    /// a helper-only test would pass even if a forwarder forgot to call it.
    /// The discriminator is 400-vs-502 — a refused traversal returns 400
    /// WITHOUT contacting the daemon, while a legitimate path reaches the
    /// forwarder and fails at the closed port with 502. A 502 on a traversal
    /// would mean the guard was bypassed and the request went upstream.
    #[tokio::test]
    async fn dot_segments_are_refused_before_reaching_the_daemon() {
        let dist = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(AppState {
            http: reqwest::Client::new(),
            http_json: reqwest::Client::new(),
            voice_profile: "leo".to_string(),
            // Closed port: a request that gets past the guard fails fast as
            // 502, which is exactly how we detect a bypass.
            daemon_url: "http://127.0.0.1:9".to_string(),
            default_livekit_room_id: "project:surface-test".to_string(),
            tldraw_sync_uri: None,
            maps_key: None,
            maps_map_id: "DEMO_MAP_ID".to_string(),
            basic_auth: None,
            session_token: "test-session".to_string(),
            observer_token_path: PathBuf::from("/not-used"),
        });
        let app = build_app(state, dist.path());

        // Both probe-confirmed attack shapes, raw and percent-encoded. The
        // encoded form matters because axum's Path extractor decodes before
        // the handler sees it, so `%2e%2e` arrives as `..`.
        // TASK-82: the encoded forms are the ones that BYPASSED the first
        // version of this guard. The original probe matrix tested raw dots on
        // rooms and encoded dots on longhouse — never encoded-on-rooms — and
        // that exact gap shipped a live bypass (confirmed in production:
        // %2e%2e reached the daemon with 200 while `..` returned 400).
        // Every combination of {raw, encoded, mixed-case} x {both forwarders}
        // is enumerated here so the matrix cannot be partial again.
        for uri in [
            // rooms — raw
            "/v1/rooms/persistent/../../../v1/agent/turns",
            "/v1/rooms/persistent/./events",
            // rooms — encoded (the shipped bypass)
            "/v1/rooms/persistent/%2e%2e/%2e%2e/%2e%2e/v1/agent/turns",
            "/v1/rooms/persistent/%2E%2E/v1/agent/turns",
            "/v1/rooms/persistent/%2e/events",
            // longhouse — raw and encoded
            "/v1/longhouse/../../v1/agent/sessions",
            "/v1/longhouse/%2e%2e/%2e%2e/v1/agent/sessions",
            "/v1/longhouse/%2E%2E/v1/agent/sessions",
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::BAD_REQUEST,
                "traversal must be refused at the proxy, never forwarded: {uri}",
            );
        }

        // Control: a legitimate room path still reaches the forwarder, proving
        // the guard rejects dot segments specifically rather than blanket-
        // failing the route (which would pass the assertions above vacuously).
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/rooms/persistent/room.v2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::BAD_GATEWAY,
            "a legitimate path (incl. dots inside a segment) must still route",
        );
    }

    /// GET /v1/permissions must NOT fall through to ServeDir → 404 — proven
    /// against the PRODUCTION router (`build_app`), not a synthetic one:
    /// deleting the real route flips this test's 502 into the fallback 404.
    /// The mock daemon URL points at a closed port, so reaching the forward
    /// handler yields BAD_GATEWAY — distinct from both the fallback and a
    /// working daemon, which is exactly the routing proof.
    #[tokio::test]
    async fn permissions_snapshot_routes_through_production_router() {
        let dist = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(AppState {
            http: reqwest::Client::new(),
            http_json: reqwest::Client::new(),
            voice_profile: "leo".to_string(),
            // Closed port: instant connection refusal, never a real daemon.
            daemon_url: "http://127.0.0.1:9".to_string(),
            default_livekit_room_id: "project:surface-test".to_string(),
            tldraw_sync_uri: None,
            maps_key: None,
            maps_map_id: "DEMO_MAP_ID".to_string(),
            basic_auth: None,
            session_token: "test-session".to_string(),
            observer_token_path: PathBuf::from("/not-used"),
        });
        let app = build_app(state, dist.path());

        // The production route must catch the path: handler reached → 502
        // (daemon unreachable), NOT the ServeDir fallback's 404.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/permissions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);

        // An unregistered sibling path proves the fallback is still 404, so
        // the assertion above genuinely distinguishes routed from fallthrough.
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/permissions-nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
