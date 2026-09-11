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
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use axum::{
    body::Bytes,
    extract::{Extension, Form, Path, Request, State},
    http::{header, HeaderMap, HeaderName, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tower_http::{services::ServeDir, trace::TraceLayer};
use tracing_subscriber::EnvFilter;

const DEFAULT_DAEMON_URL: &str = "http://127.0.0.1:4780";
const DEFAULT_LIVEKIT_ROOM_ID: &str = "project:surface-main";
const DEFAULT_VOICE_PROFILE: &str = "leo";

const SESSION_COOKIE: &str = "ocean_session";
/// Which BROWSER this is, so two browsers of one person can sit on two
/// different machines. Opaque, random, and authenticating nothing on its own —
/// the session cookie beside it says who you are; see [`selection_key`].
const BROWSER_COOKIE: &str = "ocean_device";
const SESSION_MAX_AGE_SECONDS: u64 = 60 * 60 * 24 * 30;

const CALL_PLACE_DAEMON_PATH: &str = "/v1/calls/place";

/// Body ceiling for a room-attachment upload forward.
///
/// Mirrors the daemon's `MAX_ATTACHMENT_BYTES` (8 MiB) + `BODY_LIMIT_SLACK`
/// (4096) exactly. The slack is not decoration: a body a little over the cap
/// must still REACH the daemon so it comes back as the typed
/// `attachment_too_large` JSON. Capping at the cap itself would turn every
/// oversize upload into our own untyped 413, which reads to the operator as a
/// proxy bug rather than the rule it actually is. The generic
/// [`ROOMS_JSON_BODY_LIMIT`] stays where it is; a room message has no business
/// being megabytes.
const ATTACHMENT_UPLOAD_BODY_LIMIT: usize = 8 * 1024 * 1024 + 4096;

/// Body ceiling for every other persistent-rooms forward (TASK-73).
const ROOMS_JSON_BODY_LIMIT: usize = 1 << 20;

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
    /// Device health probes get their own short-timeout client. They run on a
    /// person waiting to pick a machine, so a sleeping laptop must answer
    /// "unreachable" in seconds, not sit on the JSON lane's 120s budget.
    http_probe: reqwest::Client,
    /// Which device each signed-in session is attached to.
    device_selections: Arc<DeviceSelections>,
    /// Announces a selections row that just changed, so every SSE stream this
    /// proxy is holding open on the machine being left can end instead of
    /// outliving the switch. Carries the row key, never a device name: two
    /// people may both be on "studio" and only one of them switched.
    selection_changes: tokio::sync::broadcast::Sender<String>,
    voice_profile: String,
    /// Fallback upstream: used when auth is off, and as the default for a user
    /// entry that names no daemon of its own.
    daemon_url: String,
    /// Everyone who may sign in, each with their own upstream. Empty means
    /// single-user mode driven by `basic_auth` + `daemon_url` above.
    users: Vec<ProxyUser>,
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
    /// Force the session cookie's Secure attribute for public HTTPS deployments
    /// whose tunnel does not preserve a usable x-forwarded-proto header.
    secure_cookie: bool,
    /// Mode-0600 boot-bound credential minted and rotated by ocean-daemon.
    /// Read immediately before each Observatory request; never sent to the browser.
    observer_token_path: PathBuf,
    /// Mode-0600 room-authorization credential minted by ocean-daemon. The
    /// browser never receives it; exact Rooms authority mutations are the only
    /// forwards that attach it to the upstream request.
    operator_key_path: PathBuf,
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
    getrandom::fill(&mut bytes)
        .map_err(|e| anyhow::anyhow!("OS randomness required for session secret: {e}"))?;
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

/// One machine a person can attach to: an Ocean daemon, plus the credentials
/// that daemon minted.
///
/// A device is the unit a signed-in session is routed to. `daemon_url` is
/// deliberately never published to the browser — the surface knows a device by
/// NAME only, so a page that renders untrusted model output never learns the
/// shape of somebody's tailnet, and nobody has to type a URL to reach their
/// own machine.
#[derive(Clone, Debug)]
struct ProxyDevice {
    name: String,
    daemon_url: String,
    /// The observer token file minted by THIS device's daemon, when it has one.
    /// A token is minted by one daemon and means nothing to another, so there
    /// is no cross-device fallback; see [`credentials_for_device`].
    observer_token_path: Option<PathBuf>,
    /// The mode-0600 room-operator key belonging to THIS device's daemon.
    /// Possession is local execution authority, so the no-fallback rule here is
    /// absolute.
    operator_key_path: Option<PathBuf>,
    /// The device a fresh session lands on before anyone picks one.
    is_default: bool,
}

/// One person who may sign in, and the machines their sessions can drive.
///
/// Multi-user is the whole point: a proxy that holds one daemon url and one
/// credential can only ever show everyone the SAME Ocean. Each user carries
/// their own devices so a login decides *whose* sessions and instance you
/// see, while Rooms stay shared because they federate through Bedrock rather
/// than through this proxy.
#[derive(Clone)]
struct ProxyUser {
    username: String,
    password: String,
    /// Every machine this person may attach to, in roster order. NEVER empty:
    /// an entry carrying only the legacy single `daemon_url` (or nothing at
    /// all) is normalized on load into exactly one device named after its
    /// daemon's host, so an existing deployment keeps working byte-for-byte
    /// and the routing below has just one shape to reason about.
    devices: Vec<ProxyDevice>,
    /// Derived from the shared server secret plus THIS user's credentials, so
    /// one user's cookie can never authenticate as another and rotating one
    /// person's password invalidates only their sessions.
    session_token: String,
}

impl ProxyUser {
    /// Where this person lands with no selection recorded.
    fn default_device(&self) -> Option<&ProxyDevice> {
        self.devices
            .iter()
            .find(|device| device.is_default)
            .or_else(|| self.devices.first())
    }

    fn device(&self, name: &str) -> Option<&ProxyDevice> {
        self.devices.iter().find(|device| device.name == name)
    }
}

impl std::fmt::Debug for ProxyUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyUser")
            .field("username", &self.username)
            .field("password", &"[redacted]")
            .field("devices", &self.devices)
            .field("session_token", &"[redacted]")
            .finish()
    }
}

/// The upstream chosen for one request, injected by the auth gate and read by
/// every proxying handler. Making it a request extension rather than shared
/// state is what keeps two concurrent users from racing on one field — and it
/// is now the ONLY place a device's credentials are resolved, so a route
/// cannot accidentally reach a different machine than the one whose token it
/// carries.
#[derive(Clone, Debug)]
struct ResolvedDaemon {
    /// The device name this request is attached to. Logs and typed errors name
    /// it; the browser sees this string and never the URL.
    device: String,
    url: String,
    observer_token_path: Option<PathBuf>,
    operator_key_path: Option<PathBuf>,
    /// The selections row this request resolved through, when it resolved
    /// through one. A stream opened on this device ends when THIS row changes;
    /// see [`stream_ends_on_switch`].
    selection_key: Option<String>,
}

impl ResolvedDaemon {
    fn base(&self) -> &str {
        self.url.trim_end_matches('/')
    }
}

/// One device in a users-file entry.
#[derive(Deserialize)]
struct DeviceFileEntry {
    name: String,
    daemon_url: String,
    #[serde(default)]
    observer_token_path: Option<String>,
    #[serde(default)]
    operator_key_path: Option<String>,
    /// At most one device per person may set this; absent it, the first entry
    /// in the list is where a fresh session lands.
    #[serde(default, rename = "default")]
    is_default: Option<bool>,
}

/// One entry in the users file.
#[derive(Deserialize)]
struct UserFileEntry {
    username: String,
    password: String,
    /// Optional legacy single machine: falls back to OCEAN_DAEMON_URL, so a
    /// single-machine entry needs only a username and password. Normalized
    /// into a one-device roster on load; mutually exclusive with `devices`.
    #[serde(default)]
    daemon_url: Option<String>,
    /// Optional: the observer token file for THIS user's daemon. Only needed
    /// when `daemon_url` points somewhere other than the default — a token is
    /// minted by one daemon and means nothing to another, so there is no
    /// sensible fallback. See `credentials_for_device`.
    #[serde(default)]
    observer_token_path: Option<String>,
    /// Optional mode-0600 room-operator key for this exact daemon. Required
    /// for authorization mutations when `daemon_url` is not the default.
    #[serde(default)]
    operator_key_path: Option<String>,
    /// The machines this person can attach to. Absent (or empty) keeps the
    /// legacy single-daemon shape above.
    #[serde(default)]
    devices: Vec<DeviceFileEntry>,
}

/// Where multi-user config lives. Same rule as the single-user credentials:
/// a 0600 file, never the plist, because plists are world-readable.
fn users_file_path() -> PathBuf {
    std::env::var_os("OCEAN_SURFACE_USERS_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_default();
            PathBuf::from(home).join(".config/ocean-surface/users.json")
        })
}

/// Load the roster.
///
/// Falls back to the single `OCEAN_SURFACE_USER`/`OCEAN_SURFACE_PASS` pair
/// when no users file exists, so an existing single-operator deployment keeps
/// working byte-for-byte and this change is additive rather than a migration.
fn load_users(
    default_daemon_url: &str,
    secret_path: &FsPath,
    path: &FsPath,
) -> anyhow::Result<Vec<ProxyUser>> {
    let entries: Vec<UserFileEntry> = match std::fs::read_to_string(path) {
        Ok(raw) => {
            // Refuse a world-readable roster: it holds every teammate's password.
            if let Ok(meta) = std::fs::metadata(path) {
                let mode = meta.mode() & 0o777;
                if mode & 0o077 != 0 {
                    anyhow::bail!(
                        "{} is mode {:o}; it holds credentials and must be 0600",
                        path.display(),
                        mode
                    );
                }
            }
            serde_json::from_str(&raw)
                .map_err(|e| anyhow::anyhow!("{} is not valid JSON: {e}", path.display()))?
        }
        Err(_) => Vec::new(),
    };

    let mut users = Vec::new();
    for entry in entries {
        if entry.username.trim().is_empty() || entry.password.trim().is_empty() {
            anyhow::bail!(
                "{}: every user needs a username and password",
                path.display()
            );
        }
        let devices = devices_for_entry(path, &entry, default_daemon_url)?;
        let session_token =
            derive_user_session_token(&entry.username, &entry.password, secret_path)?;
        users.push(ProxyUser {
            username: entry.username,
            password: entry.password,
            devices,
            session_token,
        });
    }

    // Duplicate usernames would make login order-dependent and revocation
    // ambiguous, so they are a hard configuration error.
    let mut seen = std::collections::BTreeSet::new();
    for u in &users {
        if !seen.insert(u.username.clone()) {
            anyhow::bail!("{}: duplicate username '{}'", path.display(), u.username);
        }
    }
    Ok(users)
}

/// Normalize one roster entry into the device list the router actually uses.
///
/// Three shapes go in and one comes out:
///
/// - nothing → one device on `OCEAN_DAEMON_URL`, named after its host;
/// - the legacy single `daemon_url` (plus its optional credential paths) →
///   one device on that URL, named after its host;
/// - an explicit `devices` list → itself, validated.
///
/// Setting BOTH the legacy `daemon_url` and a `devices` list is refused rather
/// than merged: which one a session lands on would be a guess, and a guess
/// about which machine somebody's turns execute on is not a thing to ship.
fn devices_for_entry(
    path: &FsPath,
    entry: &UserFileEntry,
    default_daemon_url: &str,
) -> anyhow::Result<Vec<ProxyDevice>> {
    let username = entry.username.trim();
    if entry.devices.is_empty() {
        let daemon_url = entry
            .daemon_url
            .clone()
            .unwrap_or_else(|| default_daemon_url.to_string());
        validate_daemon_url(&daemon_url).map_err(|reason| {
            anyhow::anyhow!("{}: user '{username}' daemon_url {reason}", path.display())
        })?;
        return Ok(vec![ProxyDevice {
            name: device_name_from_url(&daemon_url),
            daemon_url,
            observer_token_path: entry.observer_token_path.clone().map(PathBuf::from),
            operator_key_path: entry.operator_key_path.clone().map(PathBuf::from),
            is_default: true,
        }]);
    }

    if entry.daemon_url.is_some() {
        anyhow::bail!(
            "{}: user '{username}' sets both daemon_url and devices; move the daemon_url \
             into the devices list",
            path.display()
        );
    }

    let mut devices = Vec::with_capacity(entry.devices.len());
    let mut seen = std::collections::BTreeSet::new();
    let mut defaults = 0_usize;
    for device in &entry.devices {
        let name = device.name.trim().to_string();
        if name.is_empty() {
            anyhow::bail!(
                "{}: user '{username}' has a device with no name",
                path.display()
            );
        }
        // A name is an identifier the browser posts back and the operator reads
        // in a log line; control characters in either place are a footgun.
        if name.chars().any(|c| c.is_control()) {
            anyhow::bail!(
                "{}: user '{username}' device '{name}' has control characters in its name",
                path.display()
            );
        }
        if !seen.insert(name.clone()) {
            anyhow::bail!(
                "{}: user '{username}' has two devices named '{name}'",
                path.display()
            );
        }
        let daemon_url = device.daemon_url.trim().to_string();
        validate_daemon_url(&daemon_url).map_err(|reason| {
            anyhow::anyhow!(
                "{}: user '{username}' device '{name}' daemon_url {reason}",
                path.display()
            )
        })?;
        let is_default = device.is_default.unwrap_or(false);
        if is_default {
            defaults += 1;
        }
        devices.push(ProxyDevice {
            name,
            daemon_url,
            observer_token_path: device.observer_token_path.clone().map(PathBuf::from),
            operator_key_path: device.operator_key_path.clone().map(PathBuf::from),
            is_default,
        });
    }
    if defaults > 1 {
        anyhow::bail!(
            "{}: user '{username}' marks {defaults} devices as default; mark at most one",
            path.display()
        );
    }
    if defaults == 0 {
        // Roster order decides, so the list is never ambiguous.
        devices[0].is_default = true;
    }
    Ok(devices)
}

/// A daemon URL must be an absolute http(s) URL with a host. This is a
/// configuration check, not a security boundary — but a typo'd upstream is
/// otherwise discovered as a mystery 503 at the far end of a login.
fn validate_daemon_url(url: &str) -> Result<(), String> {
    if url.trim() != url || url.is_empty() {
        return Err("must not be empty or padded with whitespace".to_owned());
    }
    if url.chars().any(char::is_whitespace) {
        return Err("must not contain whitespace".to_owned());
    }
    let rest = match url.split_once("://") {
        Some(("http", rest)) | Some(("https", rest)) => rest,
        _ => return Err("must start with http:// or https://".to_owned()),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() {
        return Err("names no host".to_owned());
    }
    if url_host(url).is_empty() {
        return Err("names no host".to_owned());
    }
    Ok(())
}

/// The host of a daemon URL, with userinfo and port removed and an IPv6
/// literal's brackets preserved (`[fd7a::1]:4780` → `[fd7a::1]`).
fn url_host(url: &str) -> String {
    let rest = match url.split_once("://") {
        Some((_, rest)) => rest,
        None => url,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = match authority.rsplit_once('@') {
        Some((_userinfo, host)) => host,
        None => authority,
    };
    if let Some(end) = authority.find(']') {
        return authority[..=end].to_string();
    }
    authority
        .split_once(':')
        .map(|(host, _port)| host)
        .unwrap_or(authority)
        .to_string()
}

/// The implicit name of a legacy single-daemon entry: the machine it points at.
fn device_name_from_url(url: &str) -> String {
    let host = url_host(url);
    if host.is_empty() {
        "default".to_owned()
    } else {
        host
    }
}

/// Per-user session token. Same construction as the single-user form, with the
/// username bound in, so tokens are not interchangeable between accounts.
fn derive_user_session_token(
    user: &str,
    pass: &str,
    secret_path: &FsPath,
) -> anyhow::Result<String> {
    derive_session_token(Some(&(user.to_string(), pass.to_string())), secret_path)
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

/// Where each signed-in session's device choice is remembered across restarts.
fn device_selections_path() -> PathBuf {
    std::env::var_os("OCEAN_SURFACE_DEVICE_SELECTIONS_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_default();
            PathBuf::from(home).join(".config/ocean-surface/device-selections.json")
        })
}

/// The device each signed-in session is attached to.
///
/// Server-side on purpose. The choice never rides in the cookie, so a browser
/// cannot re-point its own traffic at a machine by editing one, and the cookie
/// stays exactly as load-bearing as it was. The file is what makes the choice
/// survive a proxy restart — the difference between "pick up where you left
/// off" and "choose your device again after every deploy".
///
/// Keys are a DIGEST of the session token, never the token: this is the one
/// piece of device state written to disk and it must not become a place bearer
/// material accumulates. A token is derived from one person's username and
/// password, so the map holds at most one row per roster user and cannot grow
/// without bound.
struct DeviceSelections {
    path: PathBuf,
    /// One lock, held across the read-modify-write AND the file replacement.
    ///
    /// Snapshotting under the lock and then persisting outside it lets two
    /// concurrent selections serialize their memory writes and then race their
    /// file writes, so the older snapshot can land last and the file ends up
    /// disagreeing with memory until the next restart — at which point somebody
    /// silently gets a machine they did not pick. Selections happen when a
    /// person clicks; the write is a few hundred bytes; the contention is
    /// nothing and the ordering guarantee is the whole point.
    entries: std::sync::Mutex<std::collections::BTreeMap<String, Selection>>,
}

/// One browser's choice, with the timestamp that lets old rows be pruned.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct Selection {
    device: String,
    /// Unix seconds. Rows older than a session cookie's life cannot belong to
    /// a browser that is still signed in.
    #[serde(default)]
    updated: u64,
}

/// The stored shape, versioned by its one key so a later format can be told
/// apart from this one.
#[derive(Deserialize)]
struct DeviceSelectionsFile {
    #[serde(default)]
    selections: std::collections::BTreeMap<String, Selection>,
}

/// The most rows the file will ever hold, oldest evicted first.
///
/// One row per (person, browser) — a private window is a new browser, and a
/// person who opens enough of them would otherwise grow this file forever.
/// The cap is far above any real roster and the eviction is by age, so the
/// row a live browser is using is never the one dropped.
const MAX_DEVICE_SELECTIONS: usize = 1024;

impl DeviceSelections {
    /// Read the file if it is present, private, and parses. Anything else
    /// starts empty with a warning: losing a remembered choice costs one click,
    /// and refusing to boot over it would take the whole surface down.
    fn load(path: PathBuf) -> Self {
        let entries = match std::fs::read_to_string(&path) {
            Ok(raw) => {
                let private = std::fs::metadata(&path)
                    .map(|meta| meta.mode() & 0o077 == 0)
                    .unwrap_or(false);
                if !private {
                    tracing::warn!(
                        path = %path.display(),
                        "device selections file is group/world readable; ignoring it"
                    );
                    Default::default()
                } else {
                    match serde_json::from_str::<DeviceSelectionsFile>(&raw) {
                        Ok(file) => file.selections,
                        Err(error) => {
                            tracing::warn!(%error, path = %path.display(), "device selections file is not valid JSON; ignoring it");
                            Default::default()
                        }
                    }
                }
            }
            Err(_) => Default::default(),
        };
        Self {
            path,
            entries: std::sync::Mutex::new(entries),
        }
    }

    fn selected(&self, key: &str) -> Option<String> {
        self.entries
            .lock()
            .ok()
            .and_then(|entries| entries.get(key).map(|row| row.device.clone()))
    }

    /// Record a choice and write it through, both under one lock so the file
    /// can never disagree with memory about which choice came last.
    ///
    /// A failed write is logged, not returned: the in-memory choice is
    /// authoritative for this process either way, and a full disk should not
    /// stop somebody switching machines.
    fn record(&self, key: &str, device: &str) {
        let Ok(mut entries) = self.entries.lock() else {
            tracing::warn!("device selections lock poisoned; choice not recorded");
            return;
        };
        entries.insert(
            key.to_owned(),
            Selection {
                device: device.to_owned(),
                updated: unix_now(),
            },
        );
        prune_selections(&mut entries);
        if let Err(error) = self.persist(&entries) {
            tracing::warn!(%error, path = %self.path.display(), "device selection not persisted");
        }
    }

    /// Atomic 0600 write: a temp file in the same directory, then a rename, so
    /// a crash mid-write cannot leave a truncated roster of choices behind.
    ///
    /// Called only with `entries` locked, and the temp name carries a
    /// process-local counter as well as the pid: two writers sharing one name
    /// can truncate each other's half-written file and rename the wrong bytes
    /// into place.
    fn persist(
        &self,
        entries: &std::collections::BTreeMap<String, Selection>,
    ) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let body = serde_json::to_vec_pretty(&json!({ "selections": entries }))?;
        static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let ticket = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp = self
            .path
            .with_extension(format!("tmp{}-{ticket}", std::process::id()));
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&temp)
                .with_context(|| format!("creating {}", temp.display()))?;
            file.write_all(&body)?;
            file.sync_all()?;
        }
        // `create` does not re-apply the mode to a file that already existed.
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
        std::fs::rename(&temp, &self.path)
            .with_context(|| format!("replacing {}", self.path.display()))?;
        Ok(())
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Drop rows no live browser can still be using, then cap what is left.
///
/// A row older than the session cookie's own lifetime belongs to a browser
/// whose cookie has expired, so it can never be looked up again; beyond that
/// the newest [`MAX_DEVICE_SELECTIONS`] survive. Eviction is by age precisely
/// so an active browser's row is never the one dropped.
fn prune_selections(entries: &mut std::collections::BTreeMap<String, Selection>) {
    let now = unix_now();
    entries.retain(|_, row| now.saturating_sub(row.updated) <= SESSION_MAX_AGE_SECONDS);
    if entries.len() <= MAX_DEVICE_SELECTIONS {
        return;
    }
    let mut ages: Vec<(u64, String)> = entries
        .iter()
        .map(|(key, row)| (row.updated, key.clone()))
        .collect();
    ages.sort_unstable();
    let excess = entries.len() - MAX_DEVICE_SELECTIONS;
    for (_, key) in ages.into_iter().take(excess) {
        entries.remove(&key);
    }
}

/// The selections-file key for one browser of one person.
///
/// Both halves are load-bearing. The session token alone would key the row to
/// the PERSON: this proxy derives it from their username and password so an
/// installed PWA stays signed in across deploys, which means every browser
/// they own presents the same token — and picking a machine on the phone would
/// have re-pointed the desktop's next request too, which is not what "per
/// session" means to anyone holding both devices. The browser id alone would
/// be a bearer key to somebody else's routing: it lives in a cookie, and a
/// cookie is a thing a browser sends. Digesting the two together gives a row
/// that only that person, in that browser, can address — and, being a digest,
/// one whose presence in a file is never possession of a session.
fn selection_key(session_token: &str, browser_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"ocean-surface-device-selection\0");
    digest.update(session_token.as_bytes());
    digest.update(b"\0browser\0");
    digest.update(browser_id.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest.finalize())
}

/// A fresh opaque browser id. Random, meaningless, and authenticating nothing:
/// the session cookie beside it is what says who this is.
fn mint_browser_id() -> String {
    let mut bytes = [0_u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // Randomness is not optional for an identifier that partitions two
        // browsers; without it they must share a row rather than collide on a
        // predictable one.
        return String::new();
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
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

/// Read the daemon's room-operator credential without weakening its custody
/// contract. This is stricter than the Observatory reader because possession
/// of this key permits durable local execution-authority mutations: the file
/// must be owner-owned, single-linked, mode 0600, regular, and opened without
/// following symlinks. The value is returned only to the server-side forwarder.
fn read_room_operator_key(path: &FsPath) -> Result<String, String> {
    let link = std::fs::symlink_metadata(path)
        .map_err(|error| format!("room operator credential unavailable: {error}"))?;
    if link.file_type().is_symlink() || !link.is_file() {
        return Err("room operator credential must be a regular file".to_owned());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| format!("room operator credential unavailable: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("room operator credential unavailable: {error}"))?;
    // SAFETY: `geteuid` takes no arguments, has no preconditions, and only
    // reads the effective user id of this process.
    let owner = unsafe { libc::geteuid() };
    if !metadata.is_file()
        || metadata.uid() != owner
        || metadata.nlink() != 1
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(
            "room operator credential must be an owner-owned single-link mode-0600 regular file"
                .to_owned(),
        );
    }
    let mut key = String::new();
    file.read_to_string(&mut key)
        .map_err(|error| format!("room operator credential unavailable: {error}"))?;
    let key = key.trim();
    if key.is_empty() {
        return Err("room operator credential is empty".to_owned());
    }
    if axum::http::HeaderValue::try_from(key).is_err() {
        return Err("room operator credential is not a valid header value".to_owned());
    }
    Ok(key.to_owned())
}

fn validate_auth_bind(bind: SocketAddr, auth_disabled: bool) -> anyhow::Result<()> {
    if auth_disabled && !bind.ip().is_loopback() {
        anyhow::bail!("OCEAN_SURFACE_AUTH=off is allowed only on a loopback bind; got {bind}");
    }
    Ok(())
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
    let auth_disabled = std::env::var("OCEAN_SURFACE_AUTH").as_deref() == Ok("off");
    validate_auth_bind(bind, auth_disabled)?;
    let basic_auth = if auth_disabled {
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

    // Public tunnels may terminate HTTPS without preserving a usable
    // x-forwarded-proto header. This switch controls cookie transport hygiene
    // only; it is deliberately not an origin or device allowlist.
    let secure_cookie = match std::env::var("OCEAN_SURFACE_COOKIE_SECURE") {
        Ok(value) if value.eq_ignore_ascii_case("on") => true,
        Ok(value) if value.eq_ignore_ascii_case("off") => false,
        Ok(value) => {
            anyhow::bail!("OCEAN_SURFACE_COOKIE_SECURE must be 'on' or 'off', got {value:?}")
        }
        Err(std::env::VarError::NotPresent) => false,
        Err(std::env::VarError::NotUnicode(_)) => {
            anyhow::bail!("OCEAN_SURFACE_COOKIE_SECURE must be valid UTF-8")
        }
    };

    // Stable across deploys so an installed PWA remains signed in. The mode-0600
    // server secret never reaches the browser; rotating the configured username
    // or password changes the derived token and invalidates prior sessions.
    let session_token = derive_session_token(basic_auth.as_ref(), &session_secret_path())?;

    // Multi-user roster. Absent file -> empty -> single-user behaviour is
    // unchanged, which is what keeps this additive for existing deployments.
    let users = load_users(&daemon_url, &session_secret_path(), &users_file_path())?;
    if users.is_empty() {
        tracing::info!("single-operator mode (no users file)");
    } else {
        tracing::info!(
            count = users.len(),
            "multi-user mode: per-login daemon routing"
        );
        for u in &users {
            let devices = u
                .devices
                .iter()
                .map(|device| device.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            tracing::info!(user = %u.username, %devices, "surface user");
        }
    }

    let observer_token_path = std::env::var_os("OCEAN_OBSERVER_TOKEN_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| ocean_config_dir().join("observatory-token"));
    let operator_key_path = std::env::var_os("OCEAN_OPERATOR_KEY_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| ocean_config_dir().join("operator.key"));

    let state = Arc::new(AppState {
        users,
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
        http_probe: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(DEVICE_PROBE_TIMEOUT)
            .build()
            .expect("reqwest probe client should build"),
        device_selections: Arc::new(DeviceSelections::load(device_selections_path())),
        selection_changes: tokio::sync::broadcast::channel(SELECTION_CHANGE_BACKLOG).0,
        voice_profile,
        daemon_url,
        default_livekit_room_id,
        tldraw_sync_uri,
        basic_auth,
        session_token,
        secure_cookie,
        maps_key,
        maps_map_id,
        observer_token_path,
        operator_key_path,
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
        // Which machines the signed-in person can attach to, and which one
        // this session is on. Both are login-gated (`/api/` is never a public
        // boot asset) and both stay reachable when the selected device is
        // gone — they are how the surface recovers from that.
        .route("/api/devices", get(devices))
        .route("/api/devices/select", post(select_device))
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
        // POST is the agent builder (rooms members rail): folder-as-agent used
        // to be authorable only by hand on disk. This allowlist is not a
        // passthrough — with only `get(..)` registered, a POST to this same
        // path is answered 405 with an EMPTY body, so the surface's
        // `resp.json()` dies with "EOF while parsing a value" and the failure
        // reads as a decode bug rather than a missing route. Same dead-feature
        // shape the rooms routes below were written about.
        .route("/v1/agents", get(proxy_agents).post(proxy_agent_create))
        // GET is the agent builder's prefill (an edit must start from the
        // agent's real agent.toml, not from form defaults); PUT is the edit
        // itself. DELETE was deliberately withheld while no surface verb used
        // it; the members rail's arm-confirm delete control now issues it, so
        // the allowlist carries it — still exactly the verbs the surface
        // actually uses, no more.
        .route(
            "/v1/agents/{name}",
            get(proxy_agent_get)
                .put(proxy_agent_update)
                .delete(proxy_agent_delete),
        )
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
        // though the two are distinct subtrees either way. PATCH carries the
        // room-scoped replace-semantics writes (read cursor, trigger policy);
        // the wildcard was wired get/post/delete only, so those flips died at
        // the proxy as an empty-bodied 405 the browser could only report as a
        // decode error while the daemon route sat healthy and unreachable.
        .route(
            "/v1/rooms/persistent",
            get(proxy_rooms_persistent).post(proxy_rooms_persistent),
        )
        .route(
            "/v1/rooms/persistent/{*rest}",
            get(proxy_rooms_persistent)
                .post(proxy_rooms_persistent)
                .patch(proxy_rooms_persistent)
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
    if session_user(state, headers).is_some() {
        return true;
    }
    let Some(provided) = cookie_value(headers, SESSION_COOKIE) else {
        return false;
    };
    constant_time_eq(provided.as_bytes(), state.session_token.as_bytes())
}

/// The browser id this request carries, if it has been given one.
///
/// Absent is not an error: a browser that has never asked for its devices has
/// no selection either, and lands on the person's default machine.
fn browser_id(headers: &HeaderMap) -> Option<&str> {
    cookie_value(headers, BROWSER_COOKIE).filter(|value| !value.is_empty())
}

/// The Set-Cookie value that gives this browser its id. Same transport hygiene
/// as the session cookie: HttpOnly so page scripts cannot read it, SameSite
/// Strict, and Secure under HTTPS.
fn browser_cookie(state: &AppState, headers: &HeaderMap, id: &str) -> String {
    let secure = if request_is_https(headers, state.secure_cookie) {
        "; Secure"
    } else {
        ""
    };
    format!(
        "{BROWSER_COOKIE}={id}; Path=/; HttpOnly; SameSite=Strict; \
         Max-Age={SESSION_MAX_AGE_SECONDS}{secure}"
    )
}

/// Which roster user this request's session cookie belongs to.
///
/// Every candidate is compared in constant time and the loop does NOT exit
/// early on a match, so the work done does not vary with which user signed in.
fn session_user<'a>(state: &'a AppState, headers: &HeaderMap) -> Option<&'a ProxyUser> {
    let provided = cookie_value(headers, SESSION_COOKIE)?;
    let mut found: Option<&ProxyUser> = None;
    for user in &state.users {
        if constant_time_eq(provided.as_bytes(), user.session_token.as_bytes()) {
            found = Some(user);
        }
    }
    found
}

/// The upstream the auth gate resolved for this request. Falls back to the
/// configured default if the extension is somehow absent, which keeps a
/// misordered layer from producing a broken URL rather than a wrong one.
fn resolved_daemon(state: &AppState, req: &Request) -> ResolvedDaemon {
    req.extensions()
        .get::<ResolvedDaemon>()
        .cloned()
        .unwrap_or_else(|| fallback_daemon(state))
}

/// The process-wide default machine: single-operator mode's only device, and
/// the upstream for anything that reaches a proxying route without a session.
fn fallback_daemon(state: &AppState) -> ResolvedDaemon {
    ResolvedDaemon {
        device: device_name_from_url(&state.daemon_url),
        url: state.daemon_url.clone(),
        observer_token_path: Some(state.observer_token_path.clone()),
        operator_key_path: Some(state.operator_key_path.clone()),
        // Single-operator mode has one machine and nothing to switch to, so no
        // stream opened here is ever torn down by a selection.
        selection_key: None,
    }
}

/// Which credentials a device's requests may carry.
///
/// The rule this preserves is older than devices and is not negotiable: a
/// token minted by one daemon is meaningless to another, and the room-operator
/// key is local execution authority, so neither is ever sent anywhere but the
/// daemon that issued it. A device names its own credential files; the
/// PROCESS-WIDE paths apply only to the device that is in fact the process
/// default daemon. Anything else resolves to `None` and the route fails
/// closed — no observatory beats the wrong operator's observatory.
fn credentials_for_device(
    state: &AppState,
    device: &ProxyDevice,
) -> (Option<PathBuf>, Option<PathBuf>) {
    let is_process_default =
        device.daemon_url.trim_end_matches('/') == state.daemon_url.trim_end_matches('/');
    let observer = device
        .observer_token_path
        .clone()
        .or_else(|| is_process_default.then(|| state.observer_token_path.clone()));
    let operator = device
        .operator_key_path
        .clone()
        .or_else(|| is_process_default.then(|| state.operator_key_path.clone()));
    (observer, operator)
}

fn resolve_device(
    state: &AppState,
    device: &ProxyDevice,
    selection_key: Option<String>,
) -> ResolvedDaemon {
    let (observer_token_path, operator_key_path) = credentials_for_device(state, device);
    ResolvedDaemon {
        device: device.name.clone(),
        url: device.daemon_url.clone(),
        observer_token_path,
        operator_key_path,
        selection_key,
    }
}

/// What the gate could make of this request's device selection.
enum DeviceRouting {
    /// Attach the request to this machine.
    Attached(ResolvedDaemon),
    /// The session names a device this person no longer has — the roster was
    /// edited under a live session. Fail loudly rather than quietly landing
    /// somebody on a machine they did not choose.
    Unknown(String),
}

/// The machine this request is attached to.
///
/// A signed-in roster user lands on the device their session selected, or on
/// their default device when they have not chosen one. Everything else falls
/// back to the process default, which is what single-operator mode has always
/// used.
fn device_for(state: &AppState, headers: &HeaderMap) -> DeviceRouting {
    let Some(user) = session_user(state, headers) else {
        return DeviceRouting::Attached(fallback_daemon(state));
    };
    // A browser with no id yet has made no choice yet: it lands on this
    // person's default machine, and picking one is what gives it an id.
    let key = browser_id(headers).map(|id| selection_key(&user.session_token, id));
    let selected = key
        .as_deref()
        .and_then(|key| state.device_selections.selected(key));
    let device = match selected {
        Some(name) => match user.device(&name) {
            Some(device) => device,
            None => return DeviceRouting::Unknown(name),
        },
        None => match user.default_device() {
            Some(device) => device,
            // A roster entry always normalizes to at least one device, so this
            // is unreachable by configuration; falling back beats panicking.
            None => return DeviceRouting::Attached(fallback_daemon(state)),
        },
    };
    DeviceRouting::Attached(resolve_device(state, device, key))
}

/// Routes that cannot be served without an upstream machine. `/api/config` and
/// `/api/devices*` are deliberately excluded: they are how a surface whose
/// selection went stale learns what happened and picks again.
fn requires_device(path: &str) -> bool {
    path.starts_with("/v1/") || path == "/api/stt" || path == "/api/tts"
}

/// The one shape the surface has to understand when a machine cannot be
/// reached. Carries the device NAME and nothing else — never the URL, never
/// the transport error, both of which describe the operator's network to a
/// page that renders untrusted model output.
fn device_unavailable(device: &str, reason: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::CONTENT_TYPE, "application/json")],
        json!({
            "ok": false,
            "error": "device_unavailable",
            "reason": reason,
            "device": device,
        })
        .to_string(),
    )
        .into_response()
}

/// Which observer token file belongs to the machine this request resolved to.
///
/// Every other proxying handler forwards the browser's own session, so routing
/// it to the caller's daemon is the whole job. The observatory routes are the
/// exception: they mint no per-user auth, they present a daemon-issued
/// *observer token* read off local disk. Multi-user routing sent the request
/// to the right daemon and kept reading the process-wide path — so a signed-in
/// teammate's observatory request carried THIS machine's observer token to
/// THEIR daemon.
///
/// That is a credential disclosure, not a routing bug. A token is minted by
/// one daemon and is meaningless to any other, so a mismatch cannot be
/// papered over with a fallback: the only safe answers are the token that
/// belongs to that daemon, or none. The resolution now happens once, in
/// [`credentials_for_device`], so the token travels WITH the upstream it
/// belongs to and the two cannot drift apart.
///
/// `None` here means the caller's device has no configured credential, and the
/// route fails closed. No observatory beats the wrong operator's observatory.
fn observatory_token_path(daemon: &ResolvedDaemon) -> Option<PathBuf> {
    daemon.observer_token_path.clone()
}

/// Which room-operator key belongs to the machine this request resolved to.
///
/// The key is local execution authority, so the no-fallback rule is absolute:
/// a request routed to another machine receives only that device's explicitly
/// configured key, or no key at all. The process-wide key is used solely for
/// the process-wide default daemon.
fn room_operator_key_path(daemon: &ResolvedDaemon) -> Option<PathBuf> {
    daemon.operator_key_path.clone()
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
    mut req: Request,
    next: Next,
) -> Response {
    let authed = state.basic_auth.is_none()
        || is_public_boot_asset(req.uri().path())
        || has_valid_session(&state, req.headers())
        // Keep scripted/smoke clients compatible during migration, but never
        // challenge a browser for Basic credentials.
        || has_valid_basic_credentials(&state, req.headers());

    if authed {
        // Resolve the upstream ONCE, here, and carry it on the request. Every
        // proxying handler reads this rather than a shared field, so two people
        // using the site at the same moment cannot be routed into each other's
        // Ocean — and two tabs of one person's session cannot be routed onto
        // two different machines mid-turn.
        match device_for(&state, req.headers()) {
            DeviceRouting::Attached(daemon) => {
                req.extensions_mut().insert(daemon);
            }
            DeviceRouting::Unknown(device) => {
                if requires_device(req.uri().path()) {
                    tracing::warn!(%device, "session names a device that is no longer in the roster");
                    return device_unavailable(&device, "unknown_device");
                }
                // The shell, `/api/config` and `/api/devices` still load, which
                // is how the surface finds out and offers a machine to pick.
            }
        }
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

fn first_forwarded_value<'a>(headers: &'a HeaderMap, name: &'static str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn request_is_https(headers: &HeaderMap, secure_cookie: bool) -> bool {
    secure_cookie
        || first_forwarded_value(headers, "x-forwarded-proto")
            .is_some_and(|value| value.eq_ignore_ascii_case("https"))
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
    if state.basic_auth.is_none() && state.users.is_empty() {
        return Redirect::to("/").into_response();
    }
    // Username and password are the complete login gate. Do not reject valid
    // credentials based on Origin, Host, forwarding headers, device identity,
    // or tunnel topology; those transport details are not authentication.
    //
    // The roster is checked first and WITHOUT an early exit, so a wrong
    // username costs the same as a wrong password.
    let mut matched: Option<&ProxyUser> = None;
    for user in &state.users {
        let user_ok = constant_time_eq(form.username.as_bytes(), user.username.as_bytes());
        let pass_ok = constant_time_eq(form.password.as_bytes(), user.password.as_bytes());
        if user_ok & pass_ok {
            matched = Some(user);
        }
    }

    let issued_token = if let Some(user) = matched {
        user.session_token.clone()
    } else if let Some((want_user, want_pass)) = state.basic_auth.as_ref() {
        let user_ok = constant_time_eq(form.username.as_bytes(), want_user.as_bytes());
        let pass_ok = constant_time_eq(form.password.as_bytes(), want_pass.as_bytes());
        if !(user_ok & pass_ok) {
            tokio::time::sleep(std::time::Duration::from_millis(750)).await;
            return (StatusCode::UNAUTHORIZED, login_html(true)).into_response();
        }
        state.session_token.clone()
    } else {
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
        return (StatusCode::UNAUTHORIZED, login_html(true)).into_response();
    };

    let secure = if request_is_https(&headers, state.secure_cookie) {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "{SESSION_COOKIE}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age={SESSION_MAX_AGE_SECONDS}{secure}",
        issued_token
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

async fn logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let secure = if request_is_https(&headers, state.secure_cookie) {
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
async fn config(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Json<Value> {
    Json(config_payload(&state, session_user(&state, &headers)))
}

fn config_payload(state: &AppState, user: Option<&ProxyUser>) -> Value {
    json!({
        "daemon_url": "",
        // Who is signed in, so the client stops inventing a per-browser
        // identity. Empty in single-operator mode, which keeps the previous
        // behaviour for a deployment that has no roster.
        "user_id": user.map(|u| u.username.clone()).unwrap_or_default(),
        "user_display_name": user.map(|u| u.username.clone()).unwrap_or_default(),
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

/// How long a device gets to answer `/health` before the picker calls it
/// unreachable. Short on purpose: this runs while somebody waits to choose a
/// machine, and a laptop that is asleep answers by not answering.
const DEVICE_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// The devices this request's person may attach to, in roster order.
fn devices_for_request(state: &AppState, headers: &HeaderMap) -> Vec<ProxyDevice> {
    match session_user(state, headers) {
        Some(user) => user.devices.clone(),
        // Single-operator mode has exactly one machine: the process default.
        None => vec![ProxyDevice {
            name: device_name_from_url(&state.daemon_url),
            daemon_url: state.daemon_url.clone(),
            observer_token_path: Some(state.observer_token_path.clone()),
            operator_key_path: Some(state.operator_key_path.clone()),
            is_default: true,
        }],
    }
}

/// The device name a request is currently attached to, and whether that was an
/// explicit choice or just where this person lands by default.
fn current_selection(state: &AppState, headers: &HeaderMap) -> (String, bool) {
    let Some(user) = session_user(state, headers) else {
        return (device_name_from_url(&state.daemon_url), false);
    };
    let selected = browser_id(headers).and_then(|id| {
        state
            .device_selections
            .selected(&selection_key(&user.session_token, id))
    });
    match selected {
        Some(name) => (name, true),
        None => (
            user.default_device()
                .map(|device| device.name.clone())
                .unwrap_or_default(),
            false,
        ),
    }
}

/// Ask one daemon how it is. Metadata only — `ok`, and whatever version and
/// revision it volunteers — because this answer is rendered in the browser.
async fn probe_device(client: reqwest::Client, daemon_url: String) -> Value {
    let url = format!("{}/health", daemon_url.trim_end_matches('/'));
    match client.get(&url).send().await {
        Ok(response) if response.status().is_success() => {
            let body = response.json::<Value>().await.unwrap_or(Value::Null);
            let field = |key: &str| {
                body.get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            json!({ "state": "ok", "version": field("version"), "rev": field("rev") })
        }
        Ok(response) => json!({
            "state": "unhealthy",
            "status": response.status().as_u16(),
            "version": "",
            "rev": "",
        }),
        // The transport error names the upstream URL; log it, never ship it.
        Err(error) => {
            tracing::debug!(%error, "device health probe failed");
            json!({ "state": "unreachable", "version": "", "rev": "" })
        }
    }
}

/// `GET /api/devices` — the machines this person can attach to, which one they
/// are on, and whether each one is answering right now.
///
/// No `daemon_url` appears in this payload by design: the browser addresses a
/// machine by name and nothing else, so nobody types a URL and no page learns
/// one. `selection_explicit` is false until somebody actually picks, which is
/// what lets the surface offer the choice exactly once after a login instead
/// of nagging on every load.
async fn devices(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let devices = devices_for_request(&state, &headers);
    let (selected, selection_explicit) = current_selection(&state, &headers);
    // Give this browser an id here, before it can pick, so the choice it makes
    // next is recorded against THIS browser and not against every browser the
    // person has signed in from.
    let minted = browser_id(&headers).is_none().then(mint_browser_id);

    // Probe every machine at once — a roster of sleeping laptops must cost one
    // timeout, not one per device — and index the answers so the list the
    // person reads stays in roster order regardless of who replies first.
    let mut health = vec![Value::Null; devices.len()];
    let mut probes = tokio::task::JoinSet::new();
    for (index, device) in devices.iter().enumerate() {
        let client = state.http_probe.clone();
        let url = device.daemon_url.clone();
        probes.spawn(async move { (index, probe_device(client, url).await) });
    }
    while let Some(joined) = probes.join_next().await {
        if let Ok((index, value)) = joined {
            health[index] = value;
        }
    }

    let rows: Vec<Value> = devices
        .iter()
        .enumerate()
        .map(|(index, device)| {
            json!({
                "name": device.name,
                "default": device.is_default,
                "selected": device.name == selected,
                "health": health[index].clone(),
            })
        })
        .collect();

    let mut response = Json(json!({
        "ok": true,
        "devices": rows,
        "selected": selected,
        "selection_explicit": selection_explicit,
    }))
    .into_response();
    if let Some(id) = minted.filter(|id| !id.is_empty()) {
        if let Ok(cookie) = browser_cookie(&state, &headers, &id).parse() {
            response.headers_mut().insert(header::SET_COOKIE, cookie);
        }
    }
    response
}

#[derive(Deserialize)]
struct SelectDeviceBody {
    name: String,
}

/// `POST /api/devices/select` — attach this browser to one machine.
///
/// The choice is recorded server-side against a digest of the session token
/// and this browser's id, never in a cookie the browser could edit: a page
/// cannot re-point its own traffic, one person's phone cannot re-point their
/// desktop, and the selection survives a proxy restart so switching machines
/// does not mean signing in again.
///
/// Recording it also ENDS the streams that were open on the machine being
/// left. Without that, a tab whose SSE tail is already connected keeps
/// receiving the old machine's events while its turns and decisions go to the
/// new one — two machines blended into one transcript, which is the thing the
/// session contract exists to forbid.
async fn select_device(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<SelectDeviceBody>,
) -> Response {
    let name = body.name.trim().to_string();
    let Some(user) = session_user(&state, &headers) else {
        // Single-operator mode has one machine and no roster to select from.
        if name == device_name_from_url(&state.daemon_url) {
            return Json(json!({ "ok": true, "selected": name })).into_response();
        }
        return (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "application/json")],
            json!({ "ok": false, "error": "unknown_device", "device": name }).to_string(),
        )
            .into_response();
    };
    if user.device(&name).is_none() {
        return (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "application/json")],
            json!({ "ok": false, "error": "unknown_device", "device": name }).to_string(),
        )
            .into_response();
    }
    let (id, minted) = match browser_id(&headers) {
        Some(id) => (id.to_string(), false),
        None => (mint_browser_id(), true),
    };
    if id.is_empty() {
        // Only reachable if the OS refused randomness. Two browsers sharing a
        // predictable id would share a row, so refuse rather than guess.
        tracing::error!("no OS randomness for a browser id; selection refused");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, "application/json")],
            json!({ "ok": false, "error": "selection_unavailable" }).to_string(),
        )
            .into_response();
    }
    let key = selection_key(&user.session_token, &id);
    let username = user.username.clone();
    state.device_selections.record(&key, &name);
    // Every stream this browser has open on the old machine ends now. A send
    // with no receivers is an error only in the sense that nobody was
    // listening, which is the common case.
    let _ = state.selection_changes.send(key);
    tracing::info!(user = %username, device = %name, "browser attached to device");
    let mut response = Json(json!({ "ok": true, "selected": name })).into_response();
    if minted {
        if let Ok(cookie) = browser_cookie(&state, &headers, &id).parse() {
            response.headers_mut().insert(header::SET_COOKIE, cookie);
        }
    }
    response
}

/// Reverse-proxy POST /v1/agent/turns to the local daemon.
async fn proxy_turns(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    body: Bytes,
) -> impl IntoResponse {
    let url = format!("{}/v1/agent/turns", daemon.base());
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
        Err(err) => device_unreachable(&daemon, &err),
    }
}

/// Reverse-proxy POST /v1/agent/sessions to the local daemon.
async fn proxy_sessions_post(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    body: Bytes,
) -> impl IntoResponse {
    let url = format!("{}/v1/agent/sessions", daemon.base());
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
        Err(err) => device_unreachable(&daemon, &err),
    }
}

/// Reverse-proxy GET /v1/agent/sessions to the local daemon.
async fn proxy_sessions(State(state): State<Arc<AppState>>, req: Request) -> impl IntoResponse {
    let daemon = resolved_daemon(&state, &req);
    let q = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let url = format!("{}/v1/agent/sessions{q}", daemon.base());
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
        Err(err) => device_unreachable(&daemon, &err),
    }
}

/// Single-session detail passthrough. The chat app loads a session's transcript
/// via GET /v1/sessions/{id} (and the /v1/agent/sessions/{id} variant). Without
/// these the proxy 404'd that path → the app parsed an empty body → "EOF while
/// parsing a value" → blank chat history on session switch.
async fn proxy_session_detail(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    proxy_get_json(&state, &daemon, &format!("/v1/sessions/{id}")).await
}

async fn proxy_agent_session_detail(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    proxy_get_json(&state, &daemon, &format!("/v1/agent/sessions/{id}")).await
}

/// JSON GET passthrough helper for small daemon endpoints.
async fn proxy_get_json(state: &AppState, daemon: &ResolvedDaemon, path: &str) -> Response {
    let url = format!("{}{path}", daemon.base());
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
        Err(err) => device_unreachable(daemon, &err),
    }
}

/// JSON POST passthrough helper for small daemon endpoints.
async fn proxy_post_json(
    state: &AppState,
    daemon: &ResolvedDaemon,
    path: &str,
    body: Bytes,
) -> Response {
    let url = format!("{}{path}", daemon.base());
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
        Err(err) => device_unreachable(daemon, &err),
    }
}

/// Reverse-proxy GET /v1/models (model picker catalogue).
async fn proxy_models(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
) -> impl IntoResponse {
    proxy_get_json(&state, &daemon, "/v1/models").await
}

/// Reverse-proxy GET /v1/agents (named agent identity picker, TASK-9/TASK-11).
async fn proxy_agents(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
) -> impl IntoResponse {
    proxy_get_json(&state, &daemon, "/v1/agents").await
}

/// Reverse-proxy POST /v1/agents (agent builder → create an agent folder).
///
/// This puts a filesystem-write API on the web origin: the daemon writes under
/// `$OCEAN_AGENTS_DIR` and applies no caller auth of its own, so this proxy's
/// session gate is the only fence. Same posture as the already-proxied
/// `POST /v1/projects`, and it holds only while the daemon stays bound to
/// 127.0.0.1.
async fn proxy_agent_create(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    body: Bytes,
) -> impl IntoResponse {
    proxy_post_json(&state, &daemon, "/v1/agents", body).await
}

/// Build the daemon path addressing one agent, or `None` if the name would
/// reach a route this proxy never exposed.
///
/// The dot-segment guard is NOT redundant with the encoder: [`percent_encode_path_segment`]
/// treats `.` as unreserved (it is, per RFC 3986), so `..` survives encoding
/// unchanged and `reqwest`'s `Url::parse` would then collapse it — the exact
/// TASK-71/82 shape that once shipped a live bypass. axum's `Path` extractor
/// has already decoded once by the time we see `name`, so `%2e%2e` arrives as
/// `..`; [`has_dot_segment`] decodes once more, catching `%252e%252e` too.
fn agent_daemon_path(name: &str) -> Option<String> {
    if has_dot_segment(name) {
        return None;
    }
    Some(format!("/v1/agents/{}", percent_encode_path_segment(name)))
}

/// Reverse-proxy GET /v1/agents/{name} (one agent's definition, for prefill).
async fn proxy_agent_get(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    Path(name): Path<String>,
) -> Response {
    let Some(path) = agent_daemon_path(&name) else {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    };
    proxy_get_json(&state, &daemon, &path).await
}

/// Reverse-proxy PUT /v1/agents/{name} (agent builder → edit an agent).
async fn proxy_agent_update(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    Path(name): Path<String>,
    body: Bytes,
) -> Response {
    let Some(path) = agent_daemon_path(&name) else {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    };
    proxy_method_json(&state, &daemon, reqwest::Method::PUT, &path, body).await
}

/// Reverse-proxy DELETE /v1/agents/{name} (agent builder → remove an agent).
async fn proxy_agent_delete(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    Path(name): Path<String>,
) -> Response {
    let Some(path) = agent_daemon_path(&name) else {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    };
    proxy_method_json(
        &state,
        &daemon,
        reqwest::Method::DELETE,
        &path,
        Bytes::new(),
    )
    .await
}

/// Reverse-proxy GET /v1/fs/dirs?path=<path> (filesystem directory listing).
/// Forwards the full query string so `?path=~/dev` reaches the daemon intact.
async fn proxy_fs_dirs(State(state): State<Arc<AppState>>, req: Request) -> impl IntoResponse {
    let daemon = resolved_daemon(&state, &req);
    let mut url = format!("{}/v1/fs/dirs", daemon.base());
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
        Err(err) => device_unreachable(&daemon, &err),
    }
}

/// Reverse-proxy GET /v1/model (current selection).
async fn proxy_model_get(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
) -> impl IntoResponse {
    proxy_get_json(&state, &daemon, "/v1/model").await
}

/// Reverse-proxy POST /v1/model (hot-swap the model).
async fn proxy_model_set(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    body: Bytes,
) -> impl IntoResponse {
    proxy_post_json(&state, &daemon, "/v1/model", body).await
}

/// Reverse-proxy GET /v1/projects (project list for the picker).
async fn proxy_projects_list(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
) -> impl IntoResponse {
    proxy_get_json(&state, &daemon, "/v1/projects").await
}

/// Reverse-proxy POST /v1/projects (create a project).
async fn proxy_projects_create(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    body: Bytes,
) -> impl IntoResponse {
    proxy_post_json(&state, &daemon, "/v1/projects", body).await
}

/// Reverse-proxy GET /v1/projects/{id} (project + its sessions).
async fn proxy_project_get(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    proxy_get_json(&state, &daemon, &format!("/v1/projects/{id}")).await
}

/// Reverse-proxy PATCH /v1/projects/{id} (update name/config).
async fn proxy_project_patch(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    Path(id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    proxy_method_json(
        &state,
        &daemon,
        reqwest::Method::PATCH,
        &format!("/v1/projects/{id}"),
        body,
    )
    .await
}

/// Reverse-proxy DELETE /v1/projects/{id}.
async fn proxy_project_delete(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    proxy_method_json(
        &state,
        &daemon,
        reqwest::Method::DELETE,
        &format!("/v1/projects/{id}"),
        Bytes::new(),
    )
    .await
}

/// Reverse-proxy POST /v1/component/event (component interaction → daemon).
async fn proxy_component_event(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    body: Bytes,
) -> impl IntoResponse {
    proxy_post_json(&state, &daemon, "/v1/component/event", body).await
}

/// Reverse-proxy POST /v1/calls/place (outbound call → daemon).
async fn proxy_call_place(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    body: Bytes,
) -> impl IntoResponse {
    proxy_post_json(&state, &daemon, CALL_PLACE_DAEMON_PATH, body).await
}
/// Reverse-proxy POST /v1/voice/realtime/client-secret (ephemeral OpenAI
/// Realtime token mint → daemon; the key never reaches the browser).
async fn proxy_realtime_client_secret(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    body: Bytes,
) -> impl IntoResponse {
    proxy_post_json(&state, &daemon, "/v1/voice/realtime/client-secret", body).await
}

/// Reverse-proxy POST /v1/agent/sessions/{id}/messages (voice-agent handoff
/// note appended to a chat session → daemon).
async fn proxy_session_message_append(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    Path(id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    proxy_post_json(
        &state,
        &daemon,
        &format!("/v1/agent/sessions/{id}/messages"),
        body,
    )
    .await
}

/// Reverse-proxy POST /v1/rooms/{room_id}/livekit-token.
async fn proxy_livekit_token(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    Path(room_id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    proxy_post_json(&state, &daemon, &livekit_token_daemon_path(&room_id), body).await
}

/// JSON passthrough for an arbitrary method (PATCH/DELETE), mirroring
/// proxy_post_json but with the verb supplied.
async fn proxy_method_json(
    state: &AppState,
    daemon: &ResolvedDaemon,
    method: reqwest::Method,
    path: &str,
    body: Bytes,
) -> Response {
    let url = format!("{}{path}", daemon.base());
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
        Err(err) => device_unreachable(daemon, &err),
    }
}

/// Reverse-proxy POST /v1/requests/{id}/cancel (halt a running turn).
async fn proxy_cancel(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    proxy_post_json(
        &state,
        &daemon,
        &format!("/v1/requests/{id}/cancel"),
        Bytes::new(),
    )
    .await
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
/// A forward that never reached its machine.
///
/// Answers the same typed 503 the gate uses for a device that is not in the
/// roster, so the surface has ONE shape to recognise: "the machine you are
/// attached to did not answer", naming the device the person picked. The
/// transport error is logged and never returned — it stringifies the upstream
/// URL, which is the operator's tailnet address.
fn device_unreachable(daemon: &ResolvedDaemon, error: &reqwest::Error) -> Response {
    tracing::warn!(device = %daemon.device, %error, "device unreachable");
    device_unavailable(&daemon.device, "unreachable")
}

/// Timeout for buffered JSON forwards (TASK-73). Generous enough that a slow
/// but working daemon still answers — a cold model call can take a while —
/// while bounding the case the audit found: a WEDGED daemon previously hung
/// every JSON passthrough forever, because they all shared the untimed client
/// that SSE legitimately requires.
const JSON_FORWARD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Timeout for a room workspace COMMAND forward (`POST .../workspace/...`).
///
/// The daemon's workspace lane waits up to 960s for Bedrock to finish a
/// clone or build (`WORKSPACE_COMMAND_TIMEOUT` in ocean-os's
/// `room_workspace_proxy.rs` — Bedrock's own exec ceiling is 900s and its
/// default build budget alone is 600s). At the 120s JSON default this proxy
/// was the SHORTEST budget on the path: a long clone died here with a 502
/// while continuing upstream — Bedrock records the exec regardless — and the
/// browser read a running command as a failed one. Sitting 30s above the
/// daemon's budget means every timeout that reaches the client is the
/// daemon's own typed answer, never this hop's guess.
const WORKSPACE_COMMAND_FORWARD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(990);

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
    let daemon = resolved_daemon(&state, &req);
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
    let url = format!("{}/v1/longhouse/{rest}{q}", daemon.base());
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
        Err(err) => device_unreachable(&daemon, &err),
    }
}

/// True only for the exact Phase 1 binding mutation routes. Read-only binding
/// and package-preview requests deliberately stay credential-free; every
/// other persistent-room mutation keeps its existing authority model.
fn room_agent_authority_mutation(method: &axum::http::Method, path: &str) -> bool {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let base = segments.len() >= 5
        && segments[0] == "v1"
        && segments[1] == "rooms"
        && segments[2] == "persistent"
        && !segments[3].is_empty()
        && segments[4] == "agents";
    if !base {
        return false;
    }
    match *method {
        axum::http::Method::POST if segments.len() == 5 => true,
        axum::http::Method::POST if segments.len() == 6 && segments[5] == "bootstrap" => true,
        axum::http::Method::POST if segments.len() == 7 => {
            !segments[5].is_empty() && matches!(segments[6], "reauthorize" | "suspend" | "resume")
        }
        axum::http::Method::DELETE if segments.len() == 6 => !segments[5].is_empty(),
        _ => false,
    }
}

/// In auth-off localhost development, an ambient browser request has no
/// session secret to distinguish it from a cross-site form/fetch. Authority
/// mutations therefore accept browser source headers only when every supplied
/// Origin/Referer names the exact loopback Host that received the request.
/// Headerless clients remain supported for local scripts and CLIs.
fn auth_off_room_mutation_source_allowed(headers: &HeaderMap) -> bool {
    let origin = headers.get(header::ORIGIN);
    let referer = headers.get(header::REFERER);
    if origin.is_none() && referer.is_none() {
        return true;
    }

    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Ok(authority) = host.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    let authority_host = authority.host().trim_matches(['[', ']']);
    let loopback = authority_host.eq_ignore_ascii_case("localhost")
        || authority_host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if !loopback {
        return false;
    }

    [origin, referer].into_iter().flatten().all(|value| {
        let Ok(source) = value.to_str() else {
            return false;
        };
        let Ok(uri) = source.parse::<axum::http::Uri>() else {
            return false;
        };
        matches!(uri.scheme_str(), Some("http" | "https"))
            && uri.authority().is_some_and(|source_authority| {
                source_authority.as_str().eq_ignore_ascii_case(host)
            })
    })
}

/// Which persistent-rooms request this is, because three of the shapes under
/// one wildcard route cannot be forwarded the same way.
///
/// Classified from reconstructed path SEGMENTS, never a `contains`/`ends_with`
/// probe: `{key}` is caller-supplied, so a room literally keyed `attachments`
/// or one whose key embeds `/events` would otherwise pick the wrong lane. This
/// is the idiom TASK-11 established for the SSE tail after the axum route
/// conflict; the two attachment lanes join it rather than inventing a second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoomsPersistentShape {
    /// `GET {key}/events` — stream, never buffer.
    EventsTail,
    /// `POST {key}/attachments` — a RAW-BYTES body up to the daemon's 8 MiB
    /// cap, not JSON.
    AttachmentUpload,
    /// `GET {key}/attachments/{id}` — opaque bytes whose upstream
    /// content-type / disposition / nosniff headers ARE the security contract.
    AttachmentDownload,
    /// `POST {key}/workspace/...` — JSON both ways, but the reply can take
    /// as long as a clone or build runs; forwarded with the long command
    /// timeout instead of the JSON default.
    WorkspaceCommand,
    /// Everything else in the subtree: a JSON request, a JSON reply.
    Json,
}

fn rooms_persistent_shape(method: &axum::http::Method, path: &str) -> RoomsPersistentShape {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let keyed = segments.len() >= 5
        && segments[0] == "v1"
        && segments[1] == "rooms"
        && segments[2] == "persistent"
        && !segments[3].is_empty();
    if !keyed {
        return RoomsPersistentShape::Json;
    }
    let is_get = method == axum::http::Method::GET;
    if is_get && segments.len() == 5 && segments[4] == "events" {
        RoomsPersistentShape::EventsTail
    } else if method == axum::http::Method::POST
        && segments.len() == 5
        && segments[4] == "attachments"
    {
        RoomsPersistentShape::AttachmentUpload
    } else if is_get
        && segments.len() == 6
        && segments[4] == "attachments"
        && !segments[5].is_empty()
    {
        RoomsPersistentShape::AttachmentDownload
    } else if method == axum::http::Method::POST
        && segments.len() >= 6
        && segments[4] == "workspace"
    {
        // The daemon's workspace POSTs (exec, repo/clone, repo/build) relay
        // commands that run in a room's container before answering. Length
        // >= 6 because the daemon has no POST on the bare status route, and
        // a room merely KEYED `workspace` puts the word in segment 3, not 4.
        RoomsPersistentShape::WorkspaceCommand
    } else {
        RoomsPersistentShape::Json
    }
}

/// The forward budget one buffered rooms-persistent request gets.
///
/// Keyed off [`RoomsPersistentShape`] in one function because the long
/// command lane used to hang off a lone match arm at the builder site: a
/// reviewer reverted that arm and every proxy test stayed green, leaving a
/// long clone one refactor away from dying here as a 502 again. The buffered
/// non-command shapes answer [`JSON_FORWARD_TIMEOUT`] — the same value the
/// `http_json` client applies by default, so the answer holds even for the
/// GET forwards that never attach an explicit per-request timeout. An
/// [`EventsTail`](RoomsPersistentShape::EventsTail) never gets here: the tail
/// streams on the untimed `state.http` client — any budget would sever every
/// live tail — and returns before this function is consulted.
fn forward_timeout(shape: RoomsPersistentShape) -> std::time::Duration {
    match shape {
        RoomsPersistentShape::WorkspaceCommand => WORKSPACE_COMMAND_FORWARD_TIMEOUT,
        _ => JSON_FORWARD_TIMEOUT,
    }
}

/// Forward an attachment download with its headers intact.
///
/// The daemon answers `application/octet-stream` + `X-Content-Type-Options:
/// nosniff` + `Content-Disposition: attachment` precisely so an
/// uploader-declared `text/html` can never execute on this origin. Re-stamping
/// every buffered reply `application/json` — right for the rest of the subtree
/// — destroyed all three, which made the PROXY the stored-XSS surface the
/// daemon had carefully closed. Only those three headers are copied; the rest
/// of the response is ours.
async fn attachment_download_response(status: StatusCode, resp: reqwest::Response) -> Response {
    let mut headers = HeaderMap::new();
    for name in [
        header::CONTENT_TYPE,
        header::CONTENT_DISPOSITION,
        header::X_CONTENT_TYPE_OPTIONS,
    ] {
        if let Some(value) = resp.headers().get(&name) {
            headers.insert(name, value.clone());
        }
    }
    // A refusal (unknown attachment, malformed id) is a JSON body carrying its
    // own content type, and the copy above already moved it across. The
    // fallback is for an upstream that declared nothing at all: guess opaque,
    // never guess renderable.
    if !headers.contains_key(header::CONTENT_TYPE) {
        headers.insert(
            header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/octet-stream"),
        );
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            axum::http::HeaderValue::from_static("nosniff"),
        );
    }
    let bytes = resp.bytes().await.unwrap_or_default();
    (status, headers, bytes).into_response()
}

/// Reverse-proxy the daemon's persistent-rooms API (`/v1/rooms/persistent`
/// and everything under it). Mirrors `proxy_longhouse` — forwards the method,
/// full path, query string, and body — but also handles DELETE (leave a room:
/// `DELETE /v1/rooms/persistent/{key}/participants/{id}`). One handler serves
/// the whole subtree: list (GET) + create (POST) on the bare path, plus room
/// get (GET), join (POST), leave (DELETE), post-message (POST), transcript
/// (GET, `?after_seq=`), and the live event tail (GET, exact `{key}/events`
/// shape — streamed via sse_stream_response with Last-Event-ID resume).
///
/// [`RoomsPersistentShape`] splits out the three lanes that are not
/// JSON-in / JSON-out: the SSE tail, the raw-bytes attachment upload, and the
/// attachment download whose upstream headers must survive verbatim.
async fn proxy_rooms_persistent(
    State(state): State<Arc<AppState>>,
    req: Request,
) -> impl IntoResponse {
    let daemon = resolved_daemon(&state, &req);
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
    let shape = rooms_persistent_shape(&method, &path);
    let authority_mutation = room_agent_authority_mutation(&method, &path);
    if authority_mutation
        && state.basic_auth.is_none()
        && !auth_off_room_mutation_source_allowed(req.headers())
    {
        return (
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, "application/json")],
            Bytes::from_static(br#"{"ok":false,"error":"cross_site_operator_mutation_refused"}"#),
        )
            .into_response();
    }
    let operator_key = if authority_mutation {
        let Some(key_path) = room_operator_key_path(&daemon) else {
            tracing::warn!(
                daemon = %daemon.base(),
                "room authorization has no credential for resolved daemon"
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [(header::CONTENT_TYPE, "application/json")],
                Bytes::from_static(br#"{"ok":false,"error":"operator_credential_unavailable"}"#),
            )
                .into_response();
        };
        match read_room_operator_key(&key_path) {
            Ok(key) => Some(key),
            Err(error) => {
                tracing::warn!(
                    %error,
                    path = %key_path.display(),
                    "room operator credential unavailable"
                );
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    [(header::CONTENT_TYPE, "application/json")],
                    Bytes::from_static(
                        br#"{"ok":false,"error":"operator_credential_unavailable"}"#,
                    ),
                )
                    .into_response();
            }
        }
    } else {
        None
    };
    if shape == RoomsPersistentShape::EventsTail {
        let url = format!("{}{path}{q}", daemon.base());
        let mut upstream = state.http.get(&url);
        if let Some(last_id) = req.headers().get("last-event-id") {
            if let Ok(val) = last_id.to_str() {
                upstream = upstream.header("Last-Event-ID", val);
            }
        }
        return match upstream.send().await {
            Ok(resp) => sse_stream_response(resp, stream_ends_on_switch(&state, &daemon)),
            Err(err) => device_unreachable(&daemon, &err),
        };
    }

    // The path is always under /v1/rooms/persistent (the only routes wired to
    // this handler); forward it unchanged, with the query string preserved so
    // the transcript tail's ?after_seq= reaches the daemon.
    let url = format!("{}{path}{q}", daemon.base());
    // An attachment upload declares its own type; every other forward in this
    // subtree is JSON. Read it BEFORE the body consumes the request.
    let declared_type = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    // buffer the (small) body so we can forward it on POST/PATCH/DELETE
    // TASK-73: a body over the cap previously became an EMPTY forwarded
    // request via unwrap_or_default() — a truncation that presents upstream as
    // a legitimate call. Refuse it instead.
    //
    // The 1 MiB ceiling is right for JSON and WRONG for an attachment: it made
    // the daemon's 8 MiB cap unreachable from a browser, so every upload over
    // 1 MiB died here with an untyped 413 that no client could explain.
    let body_limit = match shape {
        RoomsPersistentShape::AttachmentUpload => ATTACHMENT_UPLOAD_BODY_LIMIT,
        _ => ROOMS_JSON_BODY_LIMIT,
    };
    let body = match axum::body::to_bytes(req.into_body(), body_limit).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return (StatusCode::PAYLOAD_TOO_LARGE, "request body too large").into_response();
        }
    };
    let builder = if method == axum::http::Method::GET {
        state.http_json.get(&url)
    } else {
        // Raw attachment bytes are not JSON, and saying they are is a lie any
        // middlebox between here and the daemon is entitled to act on. The
        // daemon reads the body as bytes and ignores this header either way.
        let forwarded_type = match shape {
            RoomsPersistentShape::AttachmentUpload => declared_type
                .as_deref()
                .unwrap_or("application/octet-stream"),
            _ => "application/json",
        };
        let builder = state
            .http_json
            .request(method, &url)
            .header(header::CONTENT_TYPE, forwarded_type)
            .body(body.to_vec());
        // A per-request timeout overrides the client's 120s default. The
        // budget is keyed off the shape in forward_timeout — where a test
        // pins it — rather than in a match arm here that a refactor once
        // proved deletable without a single test noticing. For every
        // non-command shape the explicit value equals the default it
        // replaces: workspace READS answer out of Bedrock's state in one
        // round trip and stay on the JSON budget.
        builder.timeout(forward_timeout(shape))
    };
    let builder = match operator_key {
        Some(key) => builder.header("X-Ocean-Operator", key),
        None => builder,
    };
    match builder.send().await {
        Ok(resp) => {
            let status =
                StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            if shape == RoomsPersistentShape::AttachmentDownload {
                return attachment_download_response(status, resp).await;
            }
            let bytes = resp.bytes().await.unwrap_or_default();
            (status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response()
        }
        Err(err) => device_unreachable(&daemon, &err),
    }
}

/// How many selection changes the broadcast buffers before a slow stream
/// misses one. A missed message only costs that stream its teardown, and the
/// receiver treats a lag as "keep going" rather than as a switch — never as a
/// spurious close of somebody's live transcript.
const SELECTION_CHANGE_BACKLOG: usize = 64;

/// A future that resolves when THIS request's selections row changes.
///
/// `None` when the request resolved through no row at all (single-operator
/// mode, or a browser that has never picked): there is nothing that could
/// switch under it, so its stream runs until the client or the daemon ends it,
/// exactly as before.
fn stream_ends_on_switch(
    state: &AppState,
    daemon: &ResolvedDaemon,
) -> Option<std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>> {
    let key = daemon.selection_key.clone()?;
    let mut changes = state.selection_changes.subscribe();
    Some(Box::pin(async move {
        loop {
            match changes.recv().await {
                Ok(changed) if changed == key => return,
                Ok(_) => continue,
                // Lagged: some change was missed. Ending the stream on that
                // suspicion would drop a live transcript for somebody else's
                // switch, so keep reading.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                // The sender is gone, which happens only as the process ends.
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    std::future::pending::<()>().await
                }
            }
        }
    }))
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
fn sse_stream_response(
    resp: reqwest::Response,
    ends_on_switch: Option<std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>>,
) -> Response {
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
    // arrive in real time — and end it if this browser attaches to a different
    // machine, so a tail opened on the old daemon cannot outlive the switch.
    // The client reconnects and lands on the new machine; leaving it connected
    // is what would blend two machines into one transcript.
    let stream = resp.bytes_stream();
    let body = match ends_on_switch {
        Some(stop) => axum::body::Body::from_stream(stream.take_until(stop)),
        None => axum::body::Body::from_stream(stream),
    };
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
    let daemon = resolved_daemon(&state, &req);
    let q = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let url = format!("{}/v1/events{q}", daemon.base());
    match state.http.get(&url).send().await {
        Ok(resp) => sse_stream_response(resp, stream_ends_on_switch(&state, &daemon)),
        Err(err) => device_unreachable(&daemon, &err),
    }
}

/// Reverse-proxy GET /v1/permissions (permission snapshot). The web UI polls
/// this to render pending-permission cards; without it the request fell through
/// to ServeDir → 404 (empty body) → "permission snapshot rejected".
async fn proxy_permissions_snapshot(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
) -> impl IntoResponse {
    proxy_get_json(&state, &daemon, "/v1/permissions").await
}

/// Reverse-proxy POST /v1/permissions/{id}/decision (OCEAN-136). The web UI
/// answers a permission prompt here (Allow/Deny); the body matches the
/// daemon's decision payload. Forwards the {id} + body so the decision reaches
/// the daemon — without it Allow/Deny 404'd and the gated turn stayed stuck.
async fn proxy_permission_decision(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    Path(id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    proxy_post_json(
        &state,
        &daemon,
        &format!("/v1/permissions/{id}/decision"),
        body,
    )
    .await
}

/// Reverse-proxy the daemon's SSE event stream. We stream the upstream body
/// straight through so deltas arrive in real time.
async fn proxy_events(State(state): State<Arc<AppState>>, req: Request) -> impl IntoResponse {
    let daemon = resolved_daemon(&state, &req);
    // Preserve ?session_id= query string — scopes SSE to one session per
    // OCEAN_ECOSYSTEM_CONTRACT.md. Do not strip. The full upstream query is
    // forwarded verbatim so session_id (and any other params like ?all=1)
    // reaches the daemon and the stream stays scoped to the caller's session.
    let q = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let url = format!("{}/v1/agent/events{q}", daemon.base());
    match state.http.get(&url).send().await {
        Ok(resp) => sse_stream_response(resp, stream_ends_on_switch(&state, &daemon)),
        Err(err) => device_unreachable(&daemon, &err),
    }
}

/// Reverse-proxy all read-only Observatory routes with the current daemon-minted
/// credential. Snapshot/replay are buffered JSON; events remains an unbuffered
/// SSE byte stream with Last-Event-ID resume preserved end to end.
async fn proxy_observatory(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let daemon = resolved_daemon(&state, &req);
    let Some(token_path) = observatory_token_path(&daemon) else {
        tracing::warn!(
            daemon = %daemon.base(),
            "observatory has no credential for this daemon; refusing to send another daemon's token"
        );
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "observatory credential unavailable",
        )
            .into_response();
    };
    let token = match read_observer_token(&token_path) {
        Ok(token) => token,
        Err(error) => {
            tracing::warn!(%error, path = %token_path.display(), "observatory credential unavailable");
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
    let url = format!("{}{path}{query}", daemon.base());
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
            return device_unreachable(&daemon, &error);
        }
    };
    if is_stream {
        return sse_stream_response(response, stream_ends_on_switch(&state, &daemon));
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
async fn stt(
    State(state): State<Arc<AppState>>,
    Extension(daemon): Extension<ResolvedDaemon>,
    body: Bytes,
) -> impl IntoResponse {
    let url = format!("{}/v1/voice/stt", daemon.base());

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
    Extension(daemon): Extension<ResolvedDaemon>,
    Json(req): Json<TtsRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let text = req.text.trim().to_string();
    if text.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "text required".to_string()));
    }

    let url = format!("{}/v1/voice/tts", daemon.base());

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
        agent_daemon_path, auth_off_room_mutation_source_allowed, build_app, config_payload,
        constant_time_eq, decode_segment, device_for, device_name_from_url, fallback_daemon,
        forward_timeout, has_dot_segment, has_valid_session, is_hashed_asset,
        livekit_token_daemon_path, load_users, observatory_token_path, percent_encode_path_segment,
        prune_selections, read_observer_token, read_room_operator_key,
        room_agent_authority_mutation, room_operator_key_path, rooms_persistent_shape,
        selection_key, session_auth_gate, session_user, sse_no_buffer_headers,
        stream_ends_on_switch, unix_now, url_host, validate_auth_bind, validate_daemon_url,
        wasm_headers, AppState, DeviceRouting, DeviceSelections, ProxyDevice, ProxyUser,
        ResolvedDaemon, RoomsPersistentShape, Selection, ATTACHMENT_UPLOAD_BODY_LIMIT,
        BROWSER_COOKIE, CALL_PLACE_DAEMON_PATH, JSON_FORWARD_TIMEOUT, MAX_DEVICE_SELECTIONS,
        SELECTION_CHANGE_BACKLOG, SESSION_COOKIE, SESSION_MAX_AGE_SECONDS, WASM_CACHE_CONTROL,
        WORKSPACE_COMMAND_FORWARD_TIMEOUT,
    };
    use axum::http::HeaderMap;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };

    #[test]
    fn auth_off_requires_loopback_bind() {
        assert!(validate_auth_bind("127.0.0.1:8790".parse().unwrap(), true).is_ok());
        assert!(validate_auth_bind("[::1]:8790".parse().unwrap(), true).is_ok());
        assert!(validate_auth_bind("0.0.0.0:8790".parse().unwrap(), true).is_err());
        assert!(validate_auth_bind("[::]:8790".parse().unwrap(), true).is_err());
        assert!(validate_auth_bind("0.0.0.0:8790".parse().unwrap(), false).is_ok());
    }

    #[test]
    fn auth_off_browser_sources_require_the_exact_loopback_authority() {
        for (host, origin) in [
            ("localhost:8790", "http://localhost:8790"),
            ("127.0.0.1:8790", "http://127.0.0.1:8790"),
            ("[::1]:8790", "http://[::1]:8790"),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, host.parse().unwrap());
            headers.insert(header::ORIGIN, origin.parse().unwrap());
            assert!(
                auth_off_room_mutation_source_allowed(&headers),
                "{origin} must match {host}"
            );
        }

        let mut mismatched = HeaderMap::new();
        mismatched.insert(header::HOST, "127.0.0.1:8790".parse().unwrap());
        mismatched.insert(header::ORIGIN, "http://localhost:8790".parse().unwrap());
        assert!(!auth_off_room_mutation_source_allowed(&mismatched));
        assert!(auth_off_room_mutation_source_allowed(&HeaderMap::new()));
    }

    use axum::{
        body::{Body, Bytes},
        extract::DefaultBodyLimit,
        http::{header, HeaderValue, Request, StatusCode},
        middleware,
        response::{IntoResponse, Response},
        routing::{delete, get, post},
        Json, Router,
    };
    use base64::Engine;
    use serde_json::{json, Value};
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
            http_probe: reqwest::Client::new(),
            device_selections: no_selections(),
            selection_changes: tokio::sync::broadcast::channel(SELECTION_CHANGE_BACKLOG).0,
            voice_profile: "leo".to_string(),
            daemon_url: "http://127.0.0.1:4780".to_string(),
            default_livekit_room_id: "project:surface-test".to_string(),
            tldraw_sync_uri: None,
            maps_key: None,
            maps_map_id: "DEMO_MAP_ID".to_string(),
            basic_auth: Some(("ocean".to_string(), "surface".to_string())),
            session_token: "test-session".to_string(),
            users: Vec::new(),
            secure_cookie: true,
            observer_token_path: PathBuf::from("/not-used-in-auth-tests"),
            operator_key_path: PathBuf::from("/not-used-in-auth-tests"),
        })
    }

    // ── multi-user routing ────────────────────────────────────────
    //
    // The property that matters: a login decides WHOSE Ocean you see. If any of
    // these regress, two teammates share one instance and the feature is a lie.

    /// A person with one machine, the shape every pre-devices roster had.
    fn user(name: &str, pass: &str, daemon: &str, token: &str) -> ProxyUser {
        ProxyUser {
            username: name.to_string(),
            password: pass.to_string(),
            devices: vec![device(&device_name_from_url(daemon), daemon)],
            session_token: token.to_string(),
        }
    }

    fn device(name: &str, daemon: &str) -> ProxyDevice {
        ProxyDevice {
            name: name.to_string(),
            daemon_url: daemon.to_string(),
            observer_token_path: None,
            operator_key_path: None,
            is_default: true,
        }
    }

    /// An empty, never-written selection store for a state that does not
    /// exercise device switching.
    fn no_selections() -> Arc<DeviceSelections> {
        Arc::new(DeviceSelections::load(PathBuf::from(
            "/nonexistent/ocean-surface-test/device-selections.json",
        )))
    }

    /// The machine a request lands on, unwrapped. Every test here asserts on a
    /// session that resolves; the unknown-device arm has its own tests.
    fn attached(state: &AppState, headers: &HeaderMap) -> ResolvedDaemon {
        match device_for(state, headers) {
            DeviceRouting::Attached(daemon) => daemon,
            DeviceRouting::Unknown(name) => panic!("expected an attached device, got '{name}'"),
        }
    }

    fn session_headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("{SESSION_COOKIE}={token}").parse().unwrap(),
        );
        headers
    }

    /// One person, in one named browser — the pair a selection is keyed on.
    fn browser_headers(token: &str, browser: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("{SESSION_COOKIE}={token}; {BROWSER_COOKIE}={browser}")
                .parse()
                .unwrap(),
        );
        headers
    }

    // ── observatory credentials ───────────────────────────────────
    //
    // The observatory routes are the one place that presents a credential of
    // its own rather than forwarding the browser's session, so multi-user
    // routing has to reach the TOKEN as well as the URL. A token minted by one
    // daemon means nothing to another and discloses this machine to it.

    #[test]
    fn the_default_daemon_uses_the_process_wide_observer_token() {
        let state = multi_user_state();
        let daemon = attached(&state, &session_headers("tok-ocean"));
        assert_eq!(daemon.base(), "http://127.0.0.1:4780");
        assert_eq!(
            observatory_token_path(&daemon),
            Some(state.observer_token_path.clone())
        );
    }

    #[test]
    fn single_operator_mode_is_untouched_by_the_credential_split() {
        // No session cookie at all: the historical path must still resolve.
        let state = auth_test_state();
        let daemon = attached(&state, &HeaderMap::new());
        assert_eq!(daemon.base(), "http://127.0.0.1:4780");
        assert_eq!(
            observatory_token_path(&daemon),
            Some(state.observer_token_path.clone())
        );
    }

    #[test]
    fn another_users_daemon_never_receives_this_machines_observer_token() {
        // The bug this pins: routing sent the request to Eric's daemon while
        // the credential stayed the local one, handing his machine a token for
        // this one. With no token configured for him the only right answer is
        // none — the route fails closed rather than substituting.
        let state = multi_user_state();
        let daemon = attached(&state, &session_headers("tok-eric"));
        assert_eq!(daemon.base(), "http://100.119.217.76:4780");
        assert_eq!(
            observatory_token_path(&daemon),
            None,
            "must not fall back to the local credential"
        );
    }

    #[test]
    fn a_configured_roster_credential_is_used_for_that_users_daemon() {
        let mut state = multi_user_state();
        let inner = Arc::get_mut(&mut state).expect("sole owner");
        inner.users[1].devices[0].observer_token_path = Some(PathBuf::from("/eric/observer.token"));
        let daemon = attached(&state, &session_headers("tok-eric"));
        assert_eq!(
            observatory_token_path(&daemon),
            Some(PathBuf::from("/eric/observer.token"))
        );
    }

    #[test]
    fn a_credential_is_never_answered_for_a_device_that_did_not_mint_it() {
        // Defence in depth, in the shape the device roster gives it: the
        // upstream and the credential are now resolved TOGETHER, so a session
        // pointed at one machine while carrying another's token is no longer
        // representable. What remains to prove is the no-substitution rule —
        // a machine that names no credential of its own gets none, even while
        // this process and the person's other device both hold one.
        let mut state = multi_user_state();
        let inner = Arc::get_mut(&mut state).expect("sole owner");
        inner.users[1].devices[0].observer_token_path = Some(PathBuf::from("/eric/observer.token"));
        inner.users[1].devices.push(ProxyDevice {
            is_default: false,
            ..device("studio", "http://10.0.0.9:4780")
        });
        let selections = tempfile::tempdir().expect("tempdir");
        inner.device_selections = Arc::new(DeviceSelections::load(
            selections.path().join("device-selections.json"),
        ));
        inner.device_selections.record(
            &selection_key(&inner.users[1].session_token, "browser-a"),
            "studio",
        );

        let daemon = attached(&state, &browser_headers("tok-eric", "browser-a"));
        assert_eq!(daemon.base(), "http://10.0.0.9:4780");
        assert_eq!(observatory_token_path(&daemon), None);
        assert_eq!(room_operator_key_path(&daemon), None);
    }

    #[test]
    fn only_exact_room_agent_mutations_receive_operator_authority() {
        use axum::http::Method;

        assert!(room_agent_authority_mutation(
            &Method::POST,
            "/v1/rooms/persistent/team/agents"
        ));
        assert!(room_agent_authority_mutation(
            &Method::POST,
            "/v1/rooms/persistent/team/agents/bootstrap"
        ));
        assert!(room_agent_authority_mutation(
            &Method::POST,
            "/v1/rooms/persistent/team/agents/member-1/reauthorize"
        ));
        assert!(room_agent_authority_mutation(
            &Method::POST,
            "/v1/rooms/persistent/team/agents/member-1/suspend"
        ));
        assert!(room_agent_authority_mutation(
            &Method::POST,
            "/v1/rooms/persistent/team/agents/member-1/resume"
        ));
        assert!(room_agent_authority_mutation(
            &Method::DELETE,
            "/v1/rooms/persistent/team/agents/member-1"
        ));

        for (method, path) in [
            (Method::GET, "/v1/rooms/persistent/team/agents"),
            (
                Method::GET,
                "/v1/rooms/persistent/team/agents/preview/researcher",
            ),
            (Method::POST, "/v1/rooms/persistent/team/messages"),
            (
                Method::POST,
                "/v1/rooms/persistent/team/agents/member-1/not-authorized",
            ),
            (
                Method::DELETE,
                "/v1/rooms/persistent/team/participants/alice",
            ),
        ] {
            assert!(
                !room_agent_authority_mutation(&method, path),
                "{method} {path} must not receive the operator key"
            );
        }
    }

    #[test]
    fn room_operator_credentials_follow_the_resolved_daemon_without_fallback() {
        let state = multi_user_state();
        assert_eq!(
            room_operator_key_path(&attached(&state, &session_headers("tok-ocean"))),
            Some(state.operator_key_path.clone()),
        );
        assert_eq!(
            room_operator_key_path(&attached(&state, &session_headers("tok-eric"))),
            None,
            "another daemon must never receive the process-wide key",
        );

        let mut configured = state;
        let inner = Arc::get_mut(&mut configured).expect("sole owner");
        inner.users[1].devices[0].operator_key_path = Some(PathBuf::from("/eric/operator.key"));
        assert_eq!(
            room_operator_key_path(&attached(&configured, &session_headers("tok-eric"))),
            Some(PathBuf::from("/eric/operator.key")),
        );
    }

    #[test]
    fn room_operator_key_reader_enforces_custody_and_never_follows_links() {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = dir.path().join("operator.key");
        std::fs::write(&key, "server-side-secret\n").expect("write key");
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).expect("chmod key");
        assert_eq!(
            read_room_operator_key(&key).expect("secure key"),
            "server-side-secret"
        );

        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644))
            .expect("chmod insecure");
        assert!(read_room_operator_key(&key).is_err());
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600))
            .expect("restore mode");

        let hardlink = dir.path().join("operator-copy");
        std::fs::hard_link(&key, &hardlink).expect("hard link");
        assert!(
            read_room_operator_key(&key).is_err(),
            "multi-link key refused"
        );

        let symlink_target = dir.path().join("symlink-target");
        std::fs::write(&symlink_target, "other-server-side-secret\n").expect("write target");
        std::fs::set_permissions(&symlink_target, std::fs::Permissions::from_mode(0o600))
            .expect("chmod target");
        let symlink = dir.path().join("operator-symlink");
        std::os::unix::fs::symlink(&symlink_target, &symlink).expect("symlink");
        assert!(
            read_room_operator_key(&symlink).is_err(),
            "symlink key refused"
        );
    }

    fn multi_user_state() -> Arc<AppState> {
        let mut state = auth_test_state();
        let inner = Arc::get_mut(&mut state).expect("sole owner");
        inner.users = vec![
            user("ocean", "pw-a", "http://127.0.0.1:4780", "tok-ocean"),
            user(
                "ecfromthedc",
                "pw-b",
                "http://100.119.217.76:4780",
                "tok-eric",
            ),
        ];
        state
    }

    fn cookie_headers(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            format!("{SESSION_COOKIE}={token}").parse().unwrap(),
        );
        h
    }

    #[test]
    fn config_publishes_the_signed_in_identity_so_rooms_show_people() {
        let state = multi_user_state();
        let eric = state
            .users
            .iter()
            .find(|u| u.username == "ecfromthedc")
            .expect("eric");

        let signed_in = config_payload(&state, Some(eric));
        assert_eq!(signed_in["user_id"], "ecfromthedc");
        assert_eq!(signed_in["user_display_name"], "ecfromthedc");

        // Single-operator / signed-out publishes nothing, so the client keeps
        // its previous per-browser behaviour instead of adopting a blank id.
        let anon = config_payload(&state, None);
        assert_eq!(anon["user_id"], "");
        assert_eq!(anon["user_display_name"], "");
    }

    #[test]
    fn each_users_session_routes_to_their_own_daemon() {
        let state = multi_user_state();
        assert_eq!(
            attached(&state, &cookie_headers("tok-ocean")).base(),
            "http://127.0.0.1:4780"
        );
        assert_eq!(
            attached(&state, &cookie_headers("tok-eric")).base(),
            "http://100.119.217.76:4780"
        );
    }

    #[test]
    fn one_users_cookie_never_resolves_to_another_users_ocean() {
        let state = multi_user_state();
        let eric = session_user(&state, &cookie_headers("tok-eric")).expect("eric");
        assert_eq!(eric.username, "ecfromthedc");
        assert_ne!(
            eric.default_device().expect("a device").daemon_url,
            "http://127.0.0.1:4780"
        );
    }

    #[test]
    fn an_unknown_cookie_is_not_a_session_and_falls_back_to_the_default() {
        let state = multi_user_state();
        assert!(session_user(&state, &cookie_headers("tok-forged")).is_none());
        // Falling back to the configured default is safe: the auth gate refuses
        // the request before any handler runs.
        assert_eq!(
            attached(&state, &cookie_headers("tok-forged")).base(),
            "http://127.0.0.1:4780"
        );
    }

    #[test]
    fn a_request_with_no_cookie_has_no_session_user() {
        let state = multi_user_state();
        assert!(session_user(&state, &HeaderMap::new()).is_none());
    }

    #[test]
    fn single_operator_mode_is_unchanged_when_no_roster_is_configured() {
        let state = auth_test_state();
        assert!(state.users.is_empty());
        assert!(session_user(&state, &cookie_headers("test-session")).is_none());
        // The legacy single-user token still authenticates...
        assert!(has_valid_session(&state, &cookie_headers("test-session")));
        // ...and still resolves to the one configured daemon.
        assert_eq!(
            attached(&state, &cookie_headers("test-session")).base(),
            "http://127.0.0.1:4780"
        );
    }

    #[test]
    fn roster_sessions_authenticate_alongside_the_legacy_token() {
        let state = multi_user_state();
        assert!(has_valid_session(&state, &cookie_headers("tok-eric")));
        assert!(has_valid_session(&state, &cookie_headers("tok-ocean")));
        assert!(has_valid_session(&state, &cookie_headers("test-session")));
        assert!(!has_valid_session(&state, &cookie_headers("nope")));
    }

    #[test]
    fn a_user_entry_without_a_daemon_inherits_the_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let users = dir.path().join("users.json");
        std::fs::write(
            &users,
            r#"[{"username":"a","password":"p"},
                {"username":"b","password":"q","daemon_url":"http://elsewhere:4780"}]"#,
        )
        .unwrap();
        std::fs::set_permissions(&users, std::fs::Permissions::from_mode(0o600)).unwrap();
        let secret = dir.path().join("secret");
        let loaded = load_users("http://default:4780", &secret, &users).expect("load");
        assert_eq!(loaded.len(), 2);
        // The legacy single `daemon_url` becomes one device named after its
        // host, and an entry with neither inherits the process default.
        assert_eq!(loaded[0].devices.len(), 1);
        assert_eq!(loaded[0].devices[0].daemon_url, "http://default:4780");
        assert_eq!(loaded[0].devices[0].name, "default");
        assert!(loaded[0].devices[0].is_default);
        assert_eq!(loaded[1].devices[0].daemon_url, "http://elsewhere:4780");
        assert_eq!(loaded[1].devices[0].name, "elsewhere");
        // Distinct credentials must yield distinct session tokens, or one login
        // would authenticate as another.
        assert_ne!(loaded[0].session_token, loaded[1].session_token);
    }

    #[test]
    fn a_world_readable_users_file_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let users = dir.path().join("users.json");
        std::fs::write(&users, r#"[{"username":"a","password":"p"}]"#).unwrap();
        std::fs::set_permissions(&users, std::fs::Permissions::from_mode(0o644)).unwrap();
        let secret = dir.path().join("secret");
        let err = load_users("http://default:4780", &secret, &users).unwrap_err();
        assert!(
            err.to_string().contains("0600"),
            "a file holding every teammate's password must not be world-readable: {err}"
        );
    }

    #[test]
    fn duplicate_usernames_are_a_configuration_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let users = dir.path().join("users.json");
        std::fs::write(
            &users,
            r#"[{"username":"a","password":"p"},{"username":"a","password":"q"}]"#,
        )
        .unwrap();
        std::fs::set_permissions(&users, std::fs::Permissions::from_mode(0o600)).unwrap();
        let secret = dir.path().join("secret");
        let err = load_users("http://default:4780", &secret, &users).unwrap_err();
        assert!(err.to_string().contains("duplicate"), "{err}");
    }

    #[test]
    fn a_missing_users_file_means_single_operator_mode_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let secret = dir.path().join("secret");
        let loaded = load_users(
            "http://default:4780",
            &secret,
            &dir.path().join("does-not-exist.json"),
        )
        .expect("absent file is fine");
        assert!(loaded.is_empty());
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
    async fn valid_credentials_ignore_origin_and_set_secure_session_cookie() {
        let dist = tempfile::tempdir().expect("temp dist");
        let resp = build_app(auth_test_state(), dist.path())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/login")
                    // Origin is transport metadata, not an authentication gate.
                    // A browser or tunnel may omit or rewrite it; valid
                    // credentials must still produce a normal session.
                    .header(header::ORIGIN, "https://unfamiliar-device.example")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("username=ocean&password=surface"))
                    .expect("valid login request"),
            )
            .await
            .expect("router should respond");

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/");
        let cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.starts_with("ocean_session=test-session;"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        assert!(cookie.contains("Secure"));
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
            http_probe: reqwest::Client::new(),
            device_selections: no_selections(),
            selection_changes: tokio::sync::broadcast::channel(SELECTION_CHANGE_BACKLOG).0,
            voice_profile: "leo".to_string(),
            daemon_url: "http://127.0.0.1:4780".to_string(),
            default_livekit_room_id: "project/surface demo".to_string(),
            tldraw_sync_uri: Some("http://127.0.0.1:5858/connect".to_string()),
            maps_key: Some("maps".to_string()),
            maps_map_id: "DEMO_MAP_ID".to_string(),
            basic_auth: None,
            session_token: "test-session".to_string(),
            users: Vec::new(),
            secure_cookie: false,
            observer_token_path: PathBuf::from("/not-used-in-config-tests"),
            operator_key_path: PathBuf::from("/not-used-in-config-tests"),
        };

        let payload = config_payload(&state, None);

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
            http_probe: reqwest::Client::new(),
            device_selections: no_selections(),
            selection_changes: tokio::sync::broadcast::channel(SELECTION_CHANGE_BACKLOG).0,
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
            users: Vec::new(),
            secure_cookie: false,
            observer_token_path: PathBuf::from("/not-used"),
            operator_key_path: PathBuf::from("/not-used"),
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
            http_probe: reqwest::Client::new(),
            device_selections: no_selections(),
            selection_changes: tokio::sync::broadcast::channel(SELECTION_CHANGE_BACKLOG).0,
            voice_profile: "leo".to_string(),
            // Closed port → the forward fails and we see the real error body.
            daemon_url: "http://127.0.0.1:9".to_string(),
            default_livekit_room_id: "project:surface-test".to_string(),
            tldraw_sync_uri: None,
            maps_key: None,
            maps_map_id: "DEMO_MAP_ID".to_string(),
            basic_auth: None,
            session_token: "test-session".to_string(),
            users: Vec::new(),
            secure_cookie: false,
            observer_token_path: PathBuf::from("/not-used"),
            operator_key_path: PathBuf::from("/not-used"),
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
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .expect("read body");
        let text = String::from_utf8_lossy(&body);
        let decoded: Value = serde_json::from_str(&text).expect("typed device error");
        assert_eq!(decoded["error"], "device_unavailable");
        assert_eq!(decoded["reason"], "unreachable");
        // The device NAME is the whole payload. It is what the picker shows,
        // and it is all the browser is allowed to learn.
        assert_eq!(decoded["device"], "127.0.0.1");
        // The name of a legacy single-daemon entry IS its host — that is what
        // "named after the host" means, and it is why the ops recipe has
        // people name their devices. Everything else about the upstream stays
        // on this side of the boundary: no scheme, no port, no path, and none
        // of the transport error, which is what TASK-73 actually found being
        // echoed. The body is exactly four known keys.
        assert!(
            !text.contains("http") && !text.contains(":9") && !text.contains('/'),
            "error body must not disclose the upstream address: {text}",
        );
        assert!(
            !text.contains("refused") && !text.contains("connect"),
            "error body must not narrate the transport failure: {text}",
        );
        let object = decoded.as_object().expect("object body");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["device", "error", "ok", "reason"]);
    }

    /// The counterpart: a NAMED device never puts its address in the body at
    /// all. This is the shape a multi-device roster actually has, and the one
    /// the picker renders.
    #[tokio::test]
    async fn an_unreachable_named_device_answers_with_its_name_only() {
        let dist = tempfile::tempdir().expect("tempdir");
        let mut state = multi_user_state();
        {
            let inner = Arc::get_mut(&mut state).expect("sole owner");
            inner.users[1].devices = vec![ProxyDevice {
                name: "studio".to_string(),
                // Closed port on a distinctive address we can grep the body for.
                daemon_url: "http://100.119.217.76:9".to_string(),
                observer_token_path: None,
                operator_key_path: None,
                is_default: true,
            }];
        }
        let app = build_app(state, dist.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/permissions")
                    .header(header::COOKIE, format!("{SESSION_COOKIE}=tok-eric"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .expect("read body");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("studio"), "the picker needs the name: {text}");
        assert!(
            !text.contains("100.119.217.76"),
            "a device's address never reaches the browser: {text}",
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
            http_probe: reqwest::Client::new(),
            device_selections: no_selections(),
            selection_changes: tokio::sync::broadcast::channel(SELECTION_CHANGE_BACKLOG).0,
            voice_profile: "leo".to_string(),
            daemon_url: "http://127.0.0.1:9".to_string(),
            default_livekit_room_id: "project:surface-test".to_string(),
            tldraw_sync_uri: None,
            maps_key: None,
            maps_map_id: "DEMO_MAP_ID".to_string(),
            basic_auth: None,
            session_token: "test-session".to_string(),
            users: Vec::new(),
            secure_cookie: false,
            observer_token_path: PathBuf::from("/not-used"),
            operator_key_path: PathBuf::from("/not-used"),
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
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
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
            http_probe: reqwest::Client::new(),
            device_selections: no_selections(),
            selection_changes: tokio::sync::broadcast::channel(SELECTION_CHANGE_BACKLOG).0,
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
            users: Vec::new(),
            secure_cookie: false,
            observer_token_path: PathBuf::from("/not-used"),
            operator_key_path: PathBuf::from("/not-used"),
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
            // agents/{name} — the agent builder's prefill route. Only
            // single-segment shapes can match `{name}`, and percent-encoding
            // does NOT neutralise them: `.` is unreserved, so `..` survives
            // the encoder intact and would collapse upstream. `%2e%2e` is
            // decoded to `..` by axum's Path extractor before the handler
            // runs; `%252e%252e` decodes to `%2e%2e`, which the guard's own
            // single-pass decode then resolves.
            "/v1/agents/..",
            "/v1/agents/%2e%2e",
            "/v1/agents/%2E%2E",
            "/v1/agents/%252e%252e",
            "/v1/agents/.",
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
            StatusCode::SERVICE_UNAVAILABLE,
            "a legitimate path (incl. dots inside a segment) must still route",
        );
    }

    /// GET /v1/permissions must NOT fall through to ServeDir → 404 — proven
    /// against the PRODUCTION router (`build_app`), not a synthetic one:
    /// deleting the real route flips this test's 503 into the fallback 404.
    /// The mock daemon URL points at a closed port, so reaching the forward
    /// handler yields the typed `device_unavailable` 503 — distinct from both
    /// the fallback and a working daemon, which is exactly the routing proof.
    #[tokio::test]
    async fn permissions_snapshot_routes_through_production_router() {
        let dist = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(AppState {
            http: reqwest::Client::new(),
            http_json: reqwest::Client::new(),
            http_probe: reqwest::Client::new(),
            device_selections: no_selections(),
            selection_changes: tokio::sync::broadcast::channel(SELECTION_CHANGE_BACKLOG).0,
            voice_profile: "leo".to_string(),
            // Closed port: instant connection refusal, never a real daemon.
            daemon_url: "http://127.0.0.1:9".to_string(),
            default_livekit_room_id: "project:surface-test".to_string(),
            tldraw_sync_uri: None,
            maps_key: None,
            maps_map_id: "DEMO_MAP_ID".to_string(),
            basic_auth: None,
            session_token: "test-session".to_string(),
            users: Vec::new(),
            secure_cookie: false,
            observer_token_path: PathBuf::from("/not-used"),
            operator_key_path: PathBuf::from("/not-used"),
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
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

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

    /// The agent builder's create must actually be on the allowlist.
    ///
    /// `/v1/agents` was registered GET-only, so a POST fell through to
    /// ServeDir and came back as an empty 404 — which reaches the browser as
    /// a JSON decode error, not a routing error, and is therefore invisible
    /// as a proxy bug. Driven against the PRODUCTION router so deleting the
    /// `.post(...)` flips this test rather than passing vacuously: a routed
    /// request reaches the forwarder and dies at the closed daemon port with
    /// 502, while a fallthrough is 404.
    #[tokio::test]
    async fn agent_create_routes_through_production_router() {
        let dist = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(AppState {
            http: reqwest::Client::new(),
            http_json: reqwest::Client::new(),
            http_probe: reqwest::Client::new(),
            device_selections: no_selections(),
            selection_changes: tokio::sync::broadcast::channel(SELECTION_CHANGE_BACKLOG).0,
            voice_profile: "leo".to_string(),
            // Closed port: instant connection refusal, never a real daemon.
            daemon_url: "http://127.0.0.1:9".to_string(),
            default_livekit_room_id: "project:surface-test".to_string(),
            tldraw_sync_uri: None,
            maps_key: None,
            maps_map_id: "DEMO_MAP_ID".to_string(),
            basic_auth: None,
            session_token: "test-session".to_string(),
            users: Vec::new(),
            secure_cookie: false,
            observer_token_path: PathBuf::from("/not-used"),
            operator_key_path: PathBuf::from("/not-used"),
        });
        let app = build_app(state, dist.path());

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/agents")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"researcher","instructions":"be useful"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "POST /v1/agents must reach the forwarder, not ServeDir",
        );

        // Control: the pre-existing GET still routes, so the assertion above
        // is about the new verb rather than the path existing at all.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/agents")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

        // Control: a POST the allowlist does not carry is refused without ever
        // reaching the forwarder. 405 (not 404) is exactly what POST
        // /v1/agents itself returned before this route existed — a registered
        // path whose method router has no POST — and it too has an empty body,
        // which is why the surface saw a decode error rather than a 405.
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/agents-nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    /// `/v1/agents/{name}` did not exist on the allowlist AT ALL, so the
    /// builder's edit mode — prefill (GET), save (PUT) and, since the members
    /// rail grew its arm-confirm delete control, remove (DELETE) — had
    /// nowhere to go on web. Same production-router discrimination as the
    /// create test: 502 means the forwarder was reached, 404 means ServeDir
    /// swallowed it, 405 would mean the method router refused the verb.
    #[tokio::test]
    async fn agent_def_update_and_delete_route_through_production_router() {
        let dist = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(AppState {
            http: reqwest::Client::new(),
            http_json: reqwest::Client::new(),
            http_probe: reqwest::Client::new(),
            device_selections: no_selections(),
            selection_changes: tokio::sync::broadcast::channel(SELECTION_CHANGE_BACKLOG).0,
            voice_profile: "leo".to_string(),
            // Closed port: instant connection refusal, never a real daemon.
            daemon_url: "http://127.0.0.1:9".to_string(),
            default_livekit_room_id: "project:surface-test".to_string(),
            tldraw_sync_uri: None,
            maps_key: None,
            maps_map_id: "DEMO_MAP_ID".to_string(),
            basic_auth: None,
            session_token: "test-session".to_string(),
            users: Vec::new(),
            secure_cookie: false,
            observer_token_path: PathBuf::from("/not-used"),
            operator_key_path: PathBuf::from("/not-used"),
        });
        let app = build_app(state, dist.path());

        for (method, body) in [
            ("GET", Body::empty()),
            ("PUT", Body::from(r#"{"instructions":"be useful"}"#)),
            ("DELETE", Body::empty()),
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri("/v1/agents/researcher")
                        .header("content-type", "application/json")
                        .body(body)
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::SERVICE_UNAVAILABLE,
                "{method} /v1/agents/{{name}} must reach the forwarder",
            );
        }

        // The verb this test once pinned OUT of the allowlist ("adding it
        // has to be a decision, not a copy-paste") is in it now — the members
        // rail's delete control is that decision. What still has to hold is
        // the dot-segment guard: the new verb rides agent_daemon_path like
        // GET and PUT, so a traversal name dies here as 400 and never reaches
        // the daemon.
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/v1/agents/%2e%2e")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    /// The room-scoped PATCHes — read cursor and trigger policy — ride the
    /// `{*rest}` wildcard, which was wired get/post/delete only: a browser
    /// PATCH died at the proxy as an empty-bodied 405 the UI could only
    /// report as a decode error, while the daemon route sat healthy and
    /// unreachable. Same production-router discrimination as the agent
    /// tests: 502 means the forwarder was reached, 405 means the method
    /// router refused the verb.
    #[tokio::test]
    async fn rooms_persistent_patch_routes_through_production_router() {
        let dist = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(AppState {
            http: reqwest::Client::new(),
            http_json: reqwest::Client::new(),
            http_probe: reqwest::Client::new(),
            device_selections: no_selections(),
            selection_changes: tokio::sync::broadcast::channel(SELECTION_CHANGE_BACKLOG).0,
            voice_profile: "leo".to_string(),
            // Closed port: instant connection refusal, never a real daemon.
            daemon_url: "http://127.0.0.1:9".to_string(),
            default_livekit_room_id: "project:surface-test".to_string(),
            tldraw_sync_uri: None,
            maps_key: None,
            maps_map_id: "DEMO_MAP_ID".to_string(),
            basic_auth: None,
            session_token: "test-session".to_string(),
            users: Vec::new(),
            secure_cookie: false,
            observer_token_path: PathBuf::from("/not-used"),
            operator_key_path: PathBuf::from("/not-used"),
        });
        let app = build_app(state, dist.path());

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/v1/rooms/persistent/room-key")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"trigger_policy":{"on_build_failure":true}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "PATCH /v1/rooms/persistent/{{key}} must reach the forwarder",
        );

        // Control: the literal `/v1/rooms/persistent` route carries no PATCH
        // — there is nothing to replace on the collection — and answers 405,
        // which is exactly what the wildcard did before the verb was added.
        // Pins the discrimination the assertion above rests on.
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/v1/rooms/persistent")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    /// The encoder alone does not make an agent name safe: `.` is unreserved,
    /// so `percent_encode_path_segment("..")` is `..`, unchanged. This pins
    /// that the path builder refuses rather than encodes, because an encoded
    /// `..` still collapses in `Url::parse` upstream.
    #[test]
    fn agent_daemon_path_refuses_traversal_that_encoding_cannot_neutralise() {
        assert_eq!(
            percent_encode_path_segment(".."),
            "..",
            "the encoder passes dots through, which is why the guard exists",
        );
        assert_eq!(agent_daemon_path(".."), None);
        assert_eq!(agent_daemon_path("."), None);
        assert_eq!(agent_daemon_path("%2e%2e"), None);
        assert_eq!(agent_daemon_path("%2E%2E"), None);

        // Legitimate names still route, including the whole daemon charset.
        assert_eq!(
            agent_daemon_path("code-review_2"),
            Some("/v1/agents/code-review_2".to_string()),
        );
        // A dot INSIDE a name is not a dot segment (same rule the rooms
        // forwarder applies to `room.v2`), and it survives encoding intact.
        assert_eq!(agent_daemon_path("a.b"), Some("/v1/agents/a.b".to_string()),);
        // Anything outside the unreserved set is escaped before it can be
        // read as path structure.
        assert_eq!(
            agent_daemon_path("a b"),
            Some("/v1/agents/a%20b".to_string()),
        );
    }

    // ── Room attachments through the wildcard forwarder ──────────────

    /// The lane a persistent-rooms request takes must come from reconstructed
    /// SEGMENTS, never a substring probe.
    ///
    /// `{key}` is caller-supplied, so a `contains("/attachments")` or an
    /// `ends_with` would let a room named after a route steal that route's
    /// forwarding rules — an 8 MiB body limit or a header-preserving download
    /// applied to something that is neither.
    #[test]
    fn attachment_lanes_are_classified_by_segment_shape() {
        use axum::http::Method;

        assert_eq!(
            rooms_persistent_shape(&Method::GET, "/v1/rooms/persistent/team/events"),
            RoomsPersistentShape::EventsTail
        );
        assert_eq!(
            rooms_persistent_shape(&Method::POST, "/v1/rooms/persistent/team/attachments"),
            RoomsPersistentShape::AttachmentUpload
        );
        assert_eq!(
            rooms_persistent_shape(&Method::GET, "/v1/rooms/persistent/team/attachments/abc123"),
            RoomsPersistentShape::AttachmentDownload
        );

        // The LIST shares the upload's path and is ordinary JSON both ways.
        assert_eq!(
            rooms_persistent_shape(&Method::GET, "/v1/rooms/persistent/team/attachments"),
            RoomsPersistentShape::Json
        );
        // A delete answers JSON; only the GET of the bytes needs the header
        // passthrough.
        assert_eq!(
            rooms_persistent_shape(
                &Method::DELETE,
                "/v1/rooms/persistent/team/attachments/abc123"
            ),
            RoomsPersistentShape::Json
        );
        // Everything else in the subtree stays on the JSON lane.
        assert_eq!(
            rooms_persistent_shape(&Method::POST, "/v1/rooms/persistent/team/messages"),
            RoomsPersistentShape::Json
        );
        assert_eq!(
            rooms_persistent_shape(&Method::GET, "/v1/rooms/persistent/team"),
            RoomsPersistentShape::Json
        );
        assert_eq!(
            rooms_persistent_shape(&Method::GET, "/v1/rooms/persistent"),
            RoomsPersistentShape::Json
        );

        // A room KEYED after a route must not inherit that route's lane —
        // the exact class of mistake a loose ends_with would make.
        assert_eq!(
            rooms_persistent_shape(&Method::POST, "/v1/rooms/persistent/attachments"),
            RoomsPersistentShape::Json
        );
        assert_eq!(
            rooms_persistent_shape(&Method::GET, "/v1/rooms/persistent/events"),
            RoomsPersistentShape::Json
        );
        // Deeper than the route: not ours to special-case.
        assert_eq!(
            rooms_persistent_shape(&Method::GET, "/v1/rooms/persistent/t/attachments/a/b"),
            RoomsPersistentShape::Json
        );
        // An empty id is not an id.
        assert_eq!(
            rooms_persistent_shape(&Method::GET, "/v1/rooms/persistent/t/attachments/"),
            RoomsPersistentShape::Json
        );
    }

    /// The workspace COMMAND lane: the daemon budgets 960s for a clone or
    /// build, so these three POSTs must not ride the 120s JSON default — at
    /// that bound a running clone read back as a 502 while Bedrock recorded
    /// the exec anyway. Reads on the same subtree answer out of Bedrock's
    /// state in one round trip and stay JSON.
    #[test]
    fn workspace_commands_get_the_long_lane_and_reads_do_not() {
        use axum::http::Method;

        assert_eq!(
            rooms_persistent_shape(&Method::POST, "/v1/rooms/persistent/team/workspace/exec"),
            RoomsPersistentShape::WorkspaceCommand
        );
        assert_eq!(
            rooms_persistent_shape(
                &Method::POST,
                "/v1/rooms/persistent/team/workspace/repo/clone"
            ),
            RoomsPersistentShape::WorkspaceCommand
        );
        assert_eq!(
            rooms_persistent_shape(
                &Method::POST,
                "/v1/rooms/persistent/team/workspace/repo/build"
            ),
            RoomsPersistentShape::WorkspaceCommand
        );

        // Every read on the subtree stays on the JSON lane.
        assert_eq!(
            rooms_persistent_shape(&Method::GET, "/v1/rooms/persistent/team/workspace"),
            RoomsPersistentShape::Json
        );
        assert_eq!(
            rooms_persistent_shape(&Method::GET, "/v1/rooms/persistent/team/workspace/repo"),
            RoomsPersistentShape::Json
        );
        assert_eq!(
            rooms_persistent_shape(&Method::GET, "/v1/rooms/persistent/team/workspace/execs"),
            RoomsPersistentShape::Json
        );

        // The daemon has no POST on the bare status route; nothing to slow.
        assert_eq!(
            rooms_persistent_shape(&Method::POST, "/v1/rooms/persistent/team/workspace"),
            RoomsPersistentShape::Json
        );
        // A room merely KEYED `workspace` does not inherit the lane — the
        // exact class of mistake a loose `contains` would make.
        assert_eq!(
            rooms_persistent_shape(&Method::POST, "/v1/rooms/persistent/workspace/messages"),
            RoomsPersistentShape::Json
        );
        // And the room keyed `workspace` CAN still reach its own workspace.
        assert_eq!(
            rooms_persistent_shape(
                &Method::POST,
                "/v1/rooms/persistent/workspace/workspace/exec"
            ),
            RoomsPersistentShape::WorkspaceCommand
        );
    }

    /// The budget the classification buys. The test above pins which requests
    /// classify as WorkspaceCommand; without this one, nothing pinned that
    /// the classification RECEIVES the long lane — reverting the timeout arm
    /// at the builder site once left every proxy test green.
    #[test]
    fn workspace_command_forward_timeout_clears_the_daemon_budget() {
        assert_eq!(
            forward_timeout(RoomsPersistentShape::WorkspaceCommand),
            WORKSPACE_COMMAND_FORWARD_TIMEOUT
        );
        // Strictly above the daemon's 960s workspace budget
        // (WORKSPACE_COMMAND_TIMEOUT in ocean-os's room_workspace_proxy.rs):
        // this hop must outlast that one so the client always reads the
        // daemon's typed answer, never this hop's 502.
        assert!(
            forward_timeout(RoomsPersistentShape::WorkspaceCommand)
                > std::time::Duration::from_secs(960)
        );
    }

    /// Every buffered non-command shape rides the JSON budget — the same
    /// value the http_json client applies as its default, so the answer stays
    /// truthful for the GET forwards that never attach an explicit
    /// per-request timeout. EventsTail is deliberately absent: the tail
    /// streams on the untimed client before forward_timeout is consulted
    /// (untimed_client_is_used_only_by_streaming_handlers pins that), so no
    /// budget at all — least of all a 120s one — is the truth for it.
    #[test]
    fn buffered_non_command_shapes_ride_the_json_budget() {
        for shape in [
            RoomsPersistentShape::AttachmentUpload,
            RoomsPersistentShape::AttachmentDownload,
            RoomsPersistentShape::Json,
        ] {
            assert_eq!(forward_timeout(shape), JSON_FORWARD_TIMEOUT);
        }
    }

    async fn spawn_room_authority_daemon() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>)
    {
        async fn received_headers(
            axum::extract::State(requests): axum::extract::State<Arc<AtomicUsize>>,
            headers: HeaderMap,
        ) -> Json<Value> {
            requests.fetch_add(1, Ordering::Relaxed);
            Json(json!({
                "operator": headers
                    .get("x-ocean-operator")
                    .and_then(|value| value.to_str().ok()),
                "cookie": headers.contains_key(header::COOKIE),
                "origin": headers.contains_key(header::ORIGIN),
                "referer": headers.contains_key(header::REFERER),
            }))
        }

        let requests = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/v1/rooms/persistent/{key}/agents",
                get(received_headers).post(received_headers),
            )
            .route(
                "/v1/rooms/persistent/{key}/agents/bootstrap",
                post(received_headers),
            )
            .route(
                "/v1/rooms/persistent/{key}/agents/preview/{package}",
                get(received_headers),
            )
            .route(
                "/v1/rooms/persistent/{key}/agents/{member}/{action}",
                post(received_headers),
            )
            .route(
                "/v1/rooms/persistent/{key}/agents/{member}",
                delete(received_headers),
            )
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind room authority upstream");
        let addr = listener.local_addr().expect("room authority addr");
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), requests, handle)
    }

    #[tokio::test]
    async fn every_room_authority_mutation_gets_only_the_server_side_key() {
        let (daemon_url, _, upstream) = spawn_room_authority_daemon().await;
        let credential_dir = tempfile::tempdir().expect("credential tempdir");
        let key_path = credential_dir.path().join("operator.key");
        std::fs::write(&key_path, "proxy-owned-authority\n").expect("write operator key");
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
            .expect("chmod operator key");

        let mut state = auth_test_state();
        let inner = Arc::get_mut(&mut state).expect("sole state owner");
        inner.daemon_url = daemon_url;
        inner.basic_auth = None;
        inner.operator_key_path = key_path;
        let dist = tempfile::tempdir().expect("dist tempdir");
        let app = build_app(state, dist.path());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/rooms/persistent/team/agents")
                    // Browser-supplied ambient and forged authority never cross
                    // the proxy boundary; the server-side key replaces them.
                    .header("x-ocean-operator", "browser-forged")
                    .header(header::COOKIE, "ambient=browser")
                    .header(header::HOST, "127.0.0.1:8790")
                    .header(header::ORIGIN, "http://127.0.0.1:8790")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .expect("mutation response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("mutation body");
        let seen: Value = serde_json::from_slice(&body).expect("mutation json");
        assert_eq!(seen["operator"], "proxy-owned-authority");
        assert_eq!(seen["cookie"], false);
        assert_eq!(seen["origin"], false);
        assert_eq!(seen["referer"], false);

        for (method, path) in [
            ("POST", "/v1/rooms/persistent/team/agents/bootstrap"),
            (
                "POST",
                "/v1/rooms/persistent/team/agents/member-1/reauthorize",
            ),
            ("POST", "/v1/rooms/persistent/team/agents/member-1/suspend"),
            ("POST", "/v1/rooms/persistent/team/agents/member-1/resume"),
            ("DELETE", "/v1/rooms/persistent/team/agents/member-1"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .header("x-ocean-operator", "browser-forged")
                        .header(header::HOST, "127.0.0.1:8790")
                        .header(header::REFERER, "http://127.0.0.1:8790/private-room")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from("{}"))
                        .unwrap(),
                )
                .await
                .expect("authority mutation response");
            assert_eq!(response.status(), StatusCode::OK, "{method} {path}");
            let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
                .await
                .expect("authority mutation body");
            let seen: Value = serde_json::from_slice(&body).expect("authority mutation json");
            assert_eq!(
                seen["operator"], "proxy-owned-authority",
                "{method} {path} must use proxy-owned authority"
            );
            assert_eq!(seen["referer"], false, "{method} {path}");
        }

        // Inspection is credential-free by contract. Even explicitly forged
        // browser authority and a room-bearing Referer are stripped rather
        // than forwarded to the daemon or replaced by the proxy-owned key.
        let inspected = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/rooms/persistent/team/agents/preview/researcher")
                    .header("x-ocean-operator", "browser-forged")
                    .header(header::REFERER, "https://surface.example/private-room")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("inspect response");
        assert_eq!(inspected.status(), StatusCode::OK);
        let body = axum::body::to_bytes(inspected.into_body(), 64 * 1024)
            .await
            .expect("inspect body");
        let seen: Value = serde_json::from_slice(&body).expect("inspect json");
        assert_eq!(seen["operator"], Value::Null);
        assert_eq!(seen["referer"], false);
        upstream.abort();
    }

    #[tokio::test]
    async fn auth_off_room_mutations_reject_foreign_browser_sources_before_upstream() {
        let (daemon_url, requests, upstream) = spawn_room_authority_daemon().await;
        let credential_dir = tempfile::tempdir().expect("credential tempdir");
        let key_path = credential_dir.path().join("operator.key");
        std::fs::write(&key_path, "proxy-owned-authority\n").expect("write operator key");
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
            .expect("chmod operator key");

        let mut state = auth_test_state();
        let inner = Arc::get_mut(&mut state).expect("sole state owner");
        inner.daemon_url = daemon_url;
        inner.basic_auth = None;
        inner.operator_key_path = key_path;
        let dist = tempfile::tempdir().expect("dist tempdir");
        let app = build_app(state, dist.path());

        for (name, source_header, source) in [
            ("foreign origin", header::ORIGIN, "https://attacker.example"),
            (
                "foreign referer",
                header::REFERER,
                "https://attacker.example/form",
            ),
            ("opaque origin", header::ORIGIN, "null"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/v1/rooms/persistent/team/agents/bootstrap")
                        .header(header::HOST, "127.0.0.1:8790")
                        .header(source_header, source)
                        // A no-cors form-compatible body must not be upgraded to
                        // trusted JSON before its cross-site source is refused.
                        .header(header::CONTENT_TYPE, "text/plain")
                        .body(Body::from(
                            r#"{"owner_member_id":"human-1","agent_package_id":"researcher"}"#,
                        ))
                        .unwrap(),
                )
                .await
                .expect("cross-site refusal");
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{name}");
            let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
                .await
                .expect("cross-site body");
            assert_eq!(
                serde_json::from_slice::<Value>(&body).expect("cross-site json")["error"],
                "cross_site_operator_mutation_refused",
                "{name}"
            );
            assert_eq!(
                requests.load(Ordering::Relaxed),
                0,
                "{name} must not reach the daemon"
            );
        }

        let rebound = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/rooms/persistent/team/agents/bootstrap")
                    .header(header::HOST, "attacker.example")
                    .header(header::ORIGIN, "https://attacker.example")
                    .header(header::CONTENT_TYPE, "text/plain")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .expect("dns-rebinding refusal");
        assert_eq!(rebound.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            requests.load(Ordering::Relaxed),
            0,
            "a non-loopback Host must not reclassify a foreign browser as local"
        );

        let headerless = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/rooms/persistent/team/agents/bootstrap")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .expect("headerless local client response");
        assert_eq!(headerless.status(), StatusCode::OK);
        assert_eq!(requests.load(Ordering::Relaxed), 1);
        upstream.abort();
    }

    #[tokio::test]
    async fn authenticated_room_mutation_does_not_treat_origin_as_login_authority() {
        let (daemon_url, requests, upstream) = spawn_room_authority_daemon().await;
        let credential_dir = tempfile::tempdir().expect("credential tempdir");
        let key_path = credential_dir.path().join("operator.key");
        std::fs::write(&key_path, "proxy-owned-authority\n").expect("write operator key");
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
            .expect("chmod operator key");
        let mut state = auth_test_state();
        let inner = Arc::get_mut(&mut state).expect("sole state owner");
        inner.daemon_url = daemon_url;
        inner.operator_key_path = key_path;
        let dist = tempfile::tempdir().expect("dist tempdir");
        let app = build_app(state, dist.path());

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/rooms/persistent/team/agents/bootstrap")
                    .header(header::COOKIE, format!("{SESSION_COOKIE}=test-session"))
                    .header(header::ORIGIN, "https://surface.example")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .expect("authenticated authority mutation");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(requests.load(Ordering::Relaxed), 1);
        upstream.abort();
    }

    #[derive(Default)]
    struct FirstAgentFixture {
        bootstrapped: bool,
        active: bool,
    }

    fn fixture_preview(owner_eligible: bool) -> Value {
        json!({
            "ok": true,
            "package_id": "researcher",
            "display_name": "Researcher",
            "definition_digest": format!("sha256:{}", "a".repeat(64)),
            "requested_capabilities": ["read"],
            "grantable_capabilities": ["read"],
            "unavailable_capabilities": [],
            "binding": null,
            "agent_member_id": owner_eligible.then_some("researcher"),
            "owner_member_id": owner_eligible.then_some("human-1"),
            "owner_eligible": owner_eligible,
        })
    }

    fn fixture_binding() -> Value {
        json!({
            "room_id": "team",
            "agent_member_id": "researcher",
            "agent_package_id": "researcher",
            "agent_definition_digest": format!("sha256:{}", "a".repeat(64)),
            "agent_definition_revision": null,
            "display_name": "Researcher",
            "owner_member_id": "human-1",
            "activation_policy": "explicit_only",
            "context_policy": "invocation_only",
            "memory_scope": "none",
            "requested_capabilities": ["read"],
            "room_capability_grants": ["read"],
            "status": "active",
            "owner_eligible": true,
            "generation": 1,
        })
    }

    async fn spawn_first_agent_daemon() -> (
        String,
        Arc<Mutex<FirstAgentFixture>>,
        tokio::task::JoinHandle<()>,
    ) {
        async fn list_bindings(
            axum::extract::State(state): axum::extract::State<Arc<Mutex<FirstAgentFixture>>>,
        ) -> Json<Value> {
            let state = state.lock().expect("first-agent fixture lock");
            Json(json!({
                "ok": true,
                "owner_eligible": state.bootstrapped,
                "bindings": if state.active { vec![fixture_binding()] } else { Vec::new() },
            }))
        }

        async fn bootstrap(
            axum::extract::State(state): axum::extract::State<Arc<Mutex<FirstAgentFixture>>>,
            headers: HeaderMap,
            Json(body): Json<Value>,
        ) -> Response {
            if headers
                .get("x-ocean-operator")
                .and_then(|value| value.to_str().ok())
                != Some("proxy-owned-authority")
            {
                return (StatusCode::FORBIDDEN, Json(json!({"ok": false}))).into_response();
            }
            if body["agent_package_id"] != "researcher" {
                return (StatusCode::BAD_REQUEST, Json(json!({"ok": false}))).into_response();
            }
            let mut state = state.lock().expect("first-agent fixture lock");
            if state.bootstrapped && body["owner_member_id"] != "human-1" {
                return (
                    StatusCode::CONFLICT,
                    Json(json!({
                        "ok": false,
                        "error": "room_agent_bootstrap_conflict"
                    })),
                )
                    .into_response();
            }
            if body["owner_member_id"] != "human-1" {
                return (StatusCode::FORBIDDEN, Json(json!({"ok": false}))).into_response();
            }
            let created = !state.bootstrapped;
            state.bootstrapped = true;
            Json(json!({
                "ok": true,
                "created": created,
                "room_id": "team",
                "owner_member_id": "human-1",
                "agent_member_id": "researcher",
                "agent_package_id": "researcher",
                "owner_eligible": true,
                "room": {
                    "id": "team",
                    "name": "Team",
                    "participants": [
                        {"id": "human-1", "kind": "human", "display_name": "Human"},
                        {"id": "researcher", "kind": "agent", "display_name": "Researcher"}
                    ]
                },
                "package_preview": fixture_preview(true),
            }))
            .into_response()
        }

        async fn preview(
            axum::extract::State(state): axum::extract::State<Arc<Mutex<FirstAgentFixture>>>,
        ) -> Json<Value> {
            let state = state.lock().expect("first-agent fixture lock");
            Json(fixture_preview(state.bootstrapped))
        }

        async fn authorize(
            axum::extract::State(state): axum::extract::State<Arc<Mutex<FirstAgentFixture>>>,
            headers: HeaderMap,
            Json(body): Json<Value>,
        ) -> Response {
            if headers
                .get("x-ocean-operator")
                .and_then(|value| value.to_str().ok())
                != Some("proxy-owned-authority")
            {
                return (StatusCode::FORBIDDEN, Json(json!({"ok": false}))).into_response();
            }
            let mut state = state.lock().expect("first-agent fixture lock");
            if !state.bootstrapped
                || body["owner_member_id"] != "human-1"
                || body["agent_member_id"] != "researcher"
                || body["agent_package_id"] != "researcher"
            {
                return (StatusCode::CONFLICT, Json(json!({"ok": false}))).into_response();
            }
            state.active = true;
            Json(json!({"ok": true, "binding": fixture_binding()})).into_response()
        }

        let state = Arc::new(Mutex::new(FirstAgentFixture::default()));
        let app = Router::new()
            .route(
                "/v1/rooms/persistent/{key}/agents",
                get(list_bindings).post(authorize),
            )
            .route(
                "/v1/rooms/persistent/{key}/agents/bootstrap",
                post(bootstrap),
            )
            .route(
                "/v1/rooms/persistent/{key}/agents/preview/{package}",
                get(preview),
            )
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind first-agent upstream");
        let addr = listener.local_addr().expect("first-agent addr");
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), state, handle)
    }

    #[tokio::test]
    async fn empty_room_bootstraps_then_authorizes_the_first_active_binding() {
        let (daemon_url, fixture, upstream) = spawn_first_agent_daemon().await;
        let credential_dir = tempfile::tempdir().expect("credential tempdir");
        let key_path = credential_dir.path().join("operator.key");
        std::fs::write(&key_path, "proxy-owned-authority\n").expect("write operator key");
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
            .expect("chmod operator key");
        let mut state = auth_test_state();
        let inner = Arc::get_mut(&mut state).expect("sole state owner");
        inner.daemon_url = daemon_url;
        inner.basic_auth = None;
        inner.operator_key_path = key_path;
        let dist = tempfile::tempdir().expect("dist tempdir");
        let app = build_app(state, dist.path());

        let empty = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/rooms/persistent/team/agents")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("empty binding list");
        let body = axum::body::to_bytes(empty.into_body(), 64 * 1024)
            .await
            .expect("empty list body");
        let body: Value = serde_json::from_slice(&body).expect("empty list json");
        assert_eq!(body["owner_eligible"], false);
        assert_eq!(body["bindings"], json!([]));

        let bootstrap = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/rooms/persistent/team/agents/bootstrap")
                    .header(header::HOST, "127.0.0.1:8790")
                    .header(header::ORIGIN, "http://127.0.0.1:8790")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"owner_member_id":"human-1","agent_package_id":"researcher"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .expect("bootstrap response");
        assert_eq!(bootstrap.status(), StatusCode::OK);
        let body = axum::body::to_bytes(bootstrap.into_body(), 64 * 1024)
            .await
            .expect("bootstrap body");
        let body: Value = serde_json::from_slice(&body).expect("bootstrap json");
        assert_eq!(body["created"], true);
        assert_eq!(body["room"]["participants"].as_array().unwrap().len(), 2);
        assert_eq!(body["package_preview"]["owner_eligible"], true);
        assert!(!fixture.lock().expect("fixture lock").active);

        let replay = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/rooms/persistent/team/agents/bootstrap")
                    .header(header::HOST, "127.0.0.1:8790")
                    .header(header::ORIGIN, "http://127.0.0.1:8790")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"owner_member_id":"human-1","agent_package_id":"researcher"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .expect("bootstrap replay");
        assert_eq!(replay.status(), StatusCode::OK);
        let body = axum::body::to_bytes(replay.into_body(), 64 * 1024)
            .await
            .expect("bootstrap replay body");
        let body: Value = serde_json::from_slice(&body).expect("bootstrap replay json");
        assert_eq!(body["created"], false);
        assert!(!fixture.lock().expect("fixture lock").active);

        let nonowner = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/rooms/persistent/team/agents/bootstrap")
                    .header(header::HOST, "127.0.0.1:8790")
                    .header(header::ORIGIN, "http://127.0.0.1:8790")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"owner_member_id":"human-2","agent_package_id":"researcher"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .expect("nonowner bootstrap refusal");
        assert_eq!(nonowner.status(), StatusCode::CONFLICT);
        assert!(!fixture.lock().expect("fixture lock").active);

        let preview = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/rooms/persistent/team/agents/preview/researcher")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("preview response");
        let body = axum::body::to_bytes(preview.into_body(), 64 * 1024)
            .await
            .expect("preview body");
        let body: Value = serde_json::from_slice(&body).expect("preview json");
        assert_eq!(body["owner_member_id"], "human-1");
        assert_eq!(body["agent_member_id"], "researcher");

        let authorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/rooms/persistent/team/agents")
                    .header(header::HOST, "127.0.0.1:8790")
                    .header(header::REFERER, "http://127.0.0.1:8790/rooms/team")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"owner_member_id":"human-1","agent_member_id":"researcher","agent_package_id":"researcher","decision_id":"018f0000-0000-4000-8000-000000000001","activation_policy":"explicit_only","context_policy":"invocation_only","memory_scope":"none","room_capability_grants":["read"]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .expect("authorize response");
        assert_eq!(authorized.status(), StatusCode::OK);

        let active = app
            .oneshot(
                Request::builder()
                    .uri("/v1/rooms/persistent/team/agents")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("active binding list");
        let body = axum::body::to_bytes(active.into_body(), 64 * 1024)
            .await
            .expect("active list body");
        let body: Value = serde_json::from_slice(&body).expect("active list json");
        assert_eq!(body["bindings"][0]["status"], "active");
        assert!(fixture.lock().expect("fixture lock").active);
        upstream.abort();
    }

    #[tokio::test]
    async fn room_authority_mutation_fails_closed_before_upstream_without_key() {
        let (daemon_url, requests, upstream) = spawn_room_authority_daemon().await;
        let mut state = auth_test_state();
        let inner = Arc::get_mut(&mut state).expect("sole state owner");
        inner.daemon_url = daemon_url;
        inner.basic_auth = None;
        inner.operator_key_path = PathBuf::from("/definitely/missing/operator.key");
        let dist = tempfile::tempdir().expect("dist tempdir");
        let app = build_app(state, dist.path());

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/rooms/persistent/team/agents")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .expect("closed response");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("closed body");
        assert_eq!(
            serde_json::from_slice::<Value>(&body).expect("closed json")["error"],
            "operator_credential_unavailable"
        );
        assert_eq!(requests.load(Ordering::Relaxed), 0);
        upstream.abort();
    }

    /// A stand-in daemon for the two forwarding tests below. Mirrors the real
    /// attachment routes' shapes: the upload's `DefaultBodyLimit`, and the
    /// download's octet-stream + nosniff + disposition triple.
    async fn spawn_attachment_daemon() -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route(
                "/v1/rooms/persistent/{key}/attachments",
                post(|headers: HeaderMap, body: Bytes| async move {
                    let declared = headers
                        .get(header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    (
                        StatusCode::CREATED,
                        Json(json!({ "ok": true, "bytes": body.len(), "declared": declared })),
                    )
                })
                .layer(DefaultBodyLimit::max(ATTACHMENT_UPLOAD_BODY_LIMIT))
                .get(|| async { Json(json!({ "ok": true, "attachments": [] })) }),
            )
            .route(
                "/v1/rooms/persistent/{key}/attachments/{id}",
                get(|| async {
                    let mut headers = HeaderMap::new();
                    headers.insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/octet-stream"),
                    );
                    headers.insert(
                        header::X_CONTENT_TYPE_OPTIONS,
                        HeaderValue::from_static("nosniff"),
                    );
                    headers.insert(
                        header::CONTENT_DISPOSITION,
                        HeaderValue::from_static("attachment; filename=\"notes.md\""),
                    );
                    (StatusCode::OK, headers, Bytes::from_static(b"# notes"))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let addr = listener.local_addr().expect("upstream addr");
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), handle)
    }

    fn attachment_proxy_state(daemon_url: String) -> Arc<AppState> {
        Arc::new(AppState {
            http: reqwest::Client::new(),
            http_json: reqwest::Client::new(),
            http_probe: reqwest::Client::new(),
            device_selections: no_selections(),
            selection_changes: tokio::sync::broadcast::channel(SELECTION_CHANGE_BACKLOG).0,
            voice_profile: "leo".to_string(),
            daemon_url,
            default_livekit_room_id: "project:surface-test".to_string(),
            tldraw_sync_uri: None,
            maps_key: None,
            maps_map_id: "DEMO_MAP_ID".to_string(),
            basic_auth: None,
            session_token: "test-session".to_string(),
            users: Vec::new(),
            secure_cookie: false,
            observer_token_path: PathBuf::from("/not-used"),
            operator_key_path: PathBuf::from("/not-used"),
        })
    }

    /// An attachment upload must reach the daemon whole.
    ///
    /// The forwarder buffered every persistent-rooms body at 1 MiB, so the
    /// daemon's 8 MiB cap was unreachable from a browser: a 2 MiB spec died at
    /// this proxy with an untyped 413 and no client could explain why. The
    /// ceiling is raised on THIS SHAPE ONLY — the message route below still
    /// gets the JSON ceiling — and the declared content type is forwarded
    /// rather than overwritten with `application/json`, which the body is not.
    ///
    /// Drives `build_app` against a real upstream: a limit that stayed on the
    /// wrong constant, or a shape check that missed this route, fails here.
    #[tokio::test]
    async fn an_attachment_upload_over_a_megabyte_reaches_the_daemon() {
        let (daemon_url, upstream) = spawn_attachment_daemon().await;
        let dist = tempfile::tempdir().expect("tempdir");
        let app = build_app(attachment_proxy_state(daemon_url), dist.path());

        let payload = vec![b'x'; 2 * 1024 * 1024];
        let sent = payload.len();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(
                        "/v1/rooms/persistent/team/attachments\
                         ?filename=spec.md&content_type=text/markdown&uploader_id=smaths",
                    )
                    .header(header::CONTENT_TYPE, "text/markdown")
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .expect("read body");
        let seen: Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(
            seen["bytes"].as_u64(),
            Some(sent as u64),
            "the daemon must receive every byte, not a truncated body",
        );
        assert_eq!(
            seen["declared"].as_str(),
            Some("text/markdown"),
            "raw attachment bytes must not be forwarded as application/json",
        );
        upstream.abort();
    }

    /// The raised ceiling is for attachments alone.
    ///
    /// A room message has no business being megabytes, and widening the limit
    /// for every persistent-rooms POST would hand an unauthenticated-shaped
    /// forward eight times the buffer it needs.
    #[tokio::test]
    async fn the_raised_ceiling_does_not_leak_to_other_rooms_routes() {
        let (daemon_url, upstream) = spawn_attachment_daemon().await;
        let dist = tempfile::tempdir().expect("tempdir");
        let app = build_app(attachment_proxy_state(daemon_url), dist.path());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/rooms/persistent/team/messages")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(vec![b'x'; 2 * 1024 * 1024]))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        upstream.abort();
    }

    /// A download's headers ARE the security contract, and the proxy used to
    /// destroy all three.
    ///
    /// The daemon answers every download `application/octet-stream` +
    /// `nosniff` + `Content-Disposition: attachment` precisely so an
    /// uploader-declared `text/html` can never execute on this origin. The
    /// forwarder re-stamped every buffered reply `application/json` — correct
    /// for the rest of the subtree — which stripped the disposition and the
    /// nosniff and mislabelled the bytes. That made the PROXY the stored-XSS
    /// surface the daemon had closed.
    #[tokio::test]
    async fn a_download_keeps_the_daemons_octet_stream_contract() {
        let (daemon_url, upstream) = spawn_attachment_daemon().await;
        let dist = tempfile::tempdir().expect("tempdir");
        let app = build_app(attachment_proxy_state(daemon_url), dist.path());

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/rooms/persistent/team/attachments/0123456789abcdef0123456789abcdef")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/octet-stream",
        );
        assert_eq!(
            resp.headers().get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
            "nosniff",
        );
        assert_eq!(
            resp.headers().get(header::CONTENT_DISPOSITION).unwrap(),
            "attachment; filename=\"notes.md\"",
        );
        let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .expect("read body");
        assert_eq!(&body[..], b"# notes", "the bytes must arrive unaltered");

        // And the JSON lane is untouched: the LIST on the same path prefix
        // still comes back labelled application/json.
        let listed = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/rooms/persistent/team/attachments")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        assert_eq!(
            listed.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json",
        );
        upstream.abort();
    }

    // ── device profiles ───────────────────────────────────────────
    //
    // The product rule these hold up: signing in at the public surface shows
    // you the machines that are YOURS, landing on one shows you ITS sessions,
    // and switching machines is a click rather than a second login. Every
    // proxied route — JSON, the SSE tail, and the voice relay — has to move
    // together, or a switch would leave a transcript streaming from the
    // machine you just left.

    #[test]
    fn a_daemon_url_yields_the_host_it_names() {
        assert_eq!(device_name_from_url("http://127.0.0.1:4780"), "127.0.0.1");
        assert_eq!(
            device_name_from_url("https://mac-mini.tailnet.ts.net:4780/"),
            "mac-mini.tailnet.ts.net"
        );
        assert_eq!(device_name_from_url("http://[fd7a::1]:4780"), "[fd7a::1]");
        assert_eq!(url_host("http://user:pw@studio.local:4780"), "studio.local");
    }

    #[test]
    fn a_daemon_url_must_be_an_absolute_http_url() {
        assert!(validate_daemon_url("http://127.0.0.1:4780").is_ok());
        assert!(validate_daemon_url("https://mini.tailnet.ts.net:4780").is_ok());
        for bad in [
            "",
            "127.0.0.1:4780",
            "ftp://mini:4780",
            "http://",
            " http://mini:4780",
            "http://mini :4780",
        ] {
            assert!(
                validate_daemon_url(bad).is_err(),
                "'{bad}' must be refused as a daemon url"
            );
        }
    }

    fn write_users(dir: &std::path::Path, body: &str) -> PathBuf {
        let users = dir.join("users.json");
        std::fs::write(&users, body).expect("write users file");
        std::fs::set_permissions(&users, std::fs::Permissions::from_mode(0o600))
            .expect("chmod users file");
        users
    }

    #[test]
    fn a_devices_list_loads_in_roster_order_with_one_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let users = write_users(
            dir.path(),
            r#"[{"username":"a","password":"p","devices":[
                {"name":"mini","daemon_url":"http://100.64.0.1:4780",
                 "observer_token_path":"/mini/observer.token"},
                {"name":"studio","daemon_url":"http://100.64.0.2:4780","default":true}
            ]}]"#,
        );
        let loaded =
            load_users("http://127.0.0.1:4780", &dir.path().join("secret"), &users).expect("load");
        let devices = &loaded[0].devices;
        assert_eq!(
            devices.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["mini", "studio"],
            "roster order is the order a person reads",
        );
        assert_eq!(
            devices[0].observer_token_path,
            Some(PathBuf::from("/mini/observer.token"))
        );
        assert!(!devices[0].is_default);
        assert!(devices[1].is_default);
        assert_eq!(loaded[0].default_device().expect("default").name, "studio");
    }

    #[test]
    fn with_no_device_marked_default_roster_order_decides() {
        let dir = tempfile::tempdir().expect("tempdir");
        let users = write_users(
            dir.path(),
            r#"[{"username":"a","password":"p","devices":[
                {"name":"mini","daemon_url":"http://100.64.0.1:4780"},
                {"name":"studio","daemon_url":"http://100.64.0.2:4780"}
            ]}]"#,
        );
        let loaded =
            load_users("http://127.0.0.1:4780", &dir.path().join("secret"), &users).expect("load");
        assert_eq!(loaded[0].default_device().expect("default").name, "mini");
    }

    #[test]
    fn a_malformed_device_roster_is_refused_at_load() {
        let cases: [(&str, &str); 5] = [
            (
                r#"[{"username":"a","password":"p","daemon_url":"http://x:4780","devices":[
                    {"name":"mini","daemon_url":"http://y:4780"}]}]"#,
                "both daemon_url and devices",
            ),
            (
                r#"[{"username":"a","password":"p","devices":[
                    {"name":"mini","daemon_url":"http://x:4780"},
                    {"name":"mini","daemon_url":"http://y:4780"}]}]"#,
                "two devices named",
            ),
            (
                r#"[{"username":"a","password":"p","devices":[
                    {"name":"mini","daemon_url":"http://x:4780","default":true},
                    {"name":"studio","daemon_url":"http://y:4780","default":true}]}]"#,
                "mark at most one",
            ),
            (
                r#"[{"username":"a","password":"p","devices":[
                    {"name":"  ","daemon_url":"http://x:4780"}]}]"#,
                "no name",
            ),
            (
                r#"[{"username":"a","password":"p","devices":[
                    {"name":"mini","daemon_url":"mini:4780"}]}]"#,
                "http:// or https://",
            ),
        ];
        for (body, expected) in cases {
            let dir = tempfile::tempdir().expect("tempdir");
            let users = write_users(dir.path(), body);
            let error = load_users("http://127.0.0.1:4780", &dir.path().join("secret"), &users)
                .expect_err("must refuse");
            let text = format!("{error}");
            assert!(
                text.contains(expected),
                "expected an error naming '{expected}', got: {text}"
            );
        }
    }

    // ── selection persistence ─────────────────────────────────────

    #[test]
    fn a_selection_outlives_the_proxy_that_recorded_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("device-selections.json");
        let key = selection_key("tok-eric", "browser-a");
        let before = DeviceSelections::load(path.clone());
        before.record(&key, "studio");
        assert_eq!(before.selected(&key).as_deref(), Some("studio"));

        // A restart is a fresh load off the same file. Without this, every
        // deploy would land everyone back on their default machine.
        let after = DeviceSelections::load(path.clone());
        assert_eq!(after.selected(&key).as_deref(), Some("studio"));
        assert_eq!(
            after.selected(&selection_key("tok-eric", "browser-b")),
            None
        );
        assert_eq!(
            after.selected(&selection_key("tok-other", "browser-a")),
            None
        );

        let mode =
            std::os::unix::fs::MetadataExt::mode(&std::fs::metadata(&path).expect("metadata"))
                & 0o777;
        assert_eq!(
            mode, 0o600,
            "the selections file is private, like the roster"
        );

        // A later choice replaces the earlier one rather than accumulating.
        after.record(&key, "mini");
        let reloaded = DeviceSelections::load(path);
        assert_eq!(reloaded.selected(&key).as_deref(), Some("mini"));
    }

    #[test]
    fn one_persons_two_browsers_hold_two_separate_choices() {
        // The finding this pins: keying a selection on the session token alone
        // keys it on the PERSON, because this proxy derives that token from
        // their username and password so an installed PWA stays signed in.
        // Every browser they own then shares one row, and picking a machine on
        // the phone re-points the desktop's next request. Two browsers, two
        // rows, and neither is the other's.
        let dir = tempfile::tempdir().expect("tempdir");
        let selections = DeviceSelections::load(dir.path().join("device-selections.json"));
        let phone = selection_key("tok-eric", "phone");
        let desktop = selection_key("tok-eric", "desktop");
        assert_ne!(phone, desktop);
        selections.record(&phone, "studio");
        selections.record(&desktop, "mini");
        assert_eq!(selections.selected(&phone).as_deref(), Some("studio"));
        assert_eq!(selections.selected(&desktop).as_deref(), Some("mini"));

        // And a browser id is not a key on its own: the same id under another
        // person's session addresses a different row, so a cookie lifted from
        // one browser cannot read or re-point another person's routing.
        assert_ne!(selection_key("tok-ocean", "phone"), phone);
    }

    #[test]
    fn concurrent_selections_all_survive_in_the_file() {
        // The finding this pins: snapshotting under the lock and persisting
        // outside it let two writers race their file writes through one
        // pid-named temp file, so a write could truncate or rename over
        // another and the file would disagree with memory until the next
        // restart — at which point somebody silently gets a machine they did
        // not pick. Eight writers, eight distinct rows, all of them present.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("device-selections.json");
        let selections = Arc::new(DeviceSelections::load(path.clone()));
        let mut writers = Vec::new();
        for index in 0..8 {
            let selections = selections.clone();
            writers.push(std::thread::spawn(move || {
                selections.record(
                    &selection_key("tok-eric", &format!("browser-{index}")),
                    "studio",
                );
            }));
        }
        for writer in writers {
            writer.join().expect("writer");
        }
        let reloaded = DeviceSelections::load(path);
        for index in 0..8 {
            assert_eq!(
                reloaded
                    .selected(&selection_key("tok-eric", &format!("browser-{index}")))
                    .as_deref(),
                Some("studio"),
                "browser-{index}'s choice did not survive the concurrent writes"
            );
        }
    }

    #[test]
    fn rows_no_live_browser_can_use_are_pruned() {
        // One row per (person, browser) is bounded by the people, but a
        // private window is a new browser — so without pruning this file grows
        // for as long as the proxy runs.
        let mut entries = std::collections::BTreeMap::new();
        entries.insert(
            "fresh".to_string(),
            Selection {
                device: "mini".into(),
                updated: unix_now(),
            },
        );
        entries.insert(
            "expired".to_string(),
            Selection {
                device: "studio".into(),
                // Older than the session cookie itself: no browser can still
                // present a cookie that would look this row up.
                updated: unix_now() - SESSION_MAX_AGE_SECONDS - 1,
            },
        );
        prune_selections(&mut entries);
        assert!(entries.contains_key("fresh"));
        assert!(!entries.contains_key("expired"));

        // Over the cap, the OLDEST go first, so an active browser's row is
        // never the one dropped.
        let mut many = std::collections::BTreeMap::new();
        for index in 0..(MAX_DEVICE_SELECTIONS + 10) {
            many.insert(
                format!("key-{index:05}"),
                Selection {
                    device: "mini".into(),
                    updated: unix_now() - (MAX_DEVICE_SELECTIONS + 10 - index) as u64,
                },
            );
        }
        prune_selections(&mut many);
        assert_eq!(many.len(), MAX_DEVICE_SELECTIONS);
        assert!(many.contains_key(&format!("key-{:05}", MAX_DEVICE_SELECTIONS + 9)));
        assert!(!many.contains_key("key-00000"));
    }

    #[test]
    fn the_selections_file_stores_a_digest_and_never_the_session_token() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("device-selections.json");
        let selections = DeviceSelections::load(path.clone());
        selections.record(
            &selection_key("super-secret-session-token", "secret-browser-id"),
            "studio",
        );
        let raw = std::fs::read_to_string(&path).expect("read selections");
        assert!(
            !raw.contains("super-secret-session-token") && !raw.contains("secret-browser-id"),
            "no cookie value may be written to disk: {raw}"
        );
        assert!(raw.contains("studio"));
    }

    #[test]
    fn a_group_readable_selections_file_is_ignored_rather_than_trusted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("device-selections.json");
        let key = selection_key("tok-eric", "browser-a");
        let selections = DeviceSelections::load(path.clone());
        selections.record(&key, "studio");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");

        // Losing a remembered choice costs one click; honouring a file anyone
        // could have written costs somebody's turns landing on a machine they
        // did not pick.
        let reloaded = DeviceSelections::load(path);
        assert_eq!(reloaded.selected(&key), None);
    }

    // ── per-session routing across devices ────────────────────────

    /// A stub daemon that says which machine it is on every route the surface
    /// actually drives: buffered JSON, the SSE tail, and the voice relay.
    async fn spawn_named_daemon(name: &'static str) -> (String, tokio::task::JoinHandle<()>) {
        let app =
            Router::new()
                .route(
                    "/health",
                    get(move || async move {
                        Json(json!({ "ok": true, "version": "9.9.9", "rev": name }))
                    }),
                )
                .route(
                    "/v1/permissions",
                    get(move || async move { Json(json!({ "ok": true, "device": name })) }),
                )
                .route(
                    "/v1/agent/events",
                    get(move || async move {
                        (
                            [(header::CONTENT_TYPE, "text/event-stream")],
                            format!("event: hello\ndata: {name}\n\n"),
                        )
                    }),
                )
                .route(
                    "/v1/voice/stt",
                    post(move || async move { Json(json!({ "ok": true, "text": name })) }),
                );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stub daemon");
        let addr = listener.local_addr().expect("stub addr");
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), handle)
    }

    async fn body_text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024)
            .await
            .expect("read body");
        String::from_utf8_lossy(&bytes).to_string()
    }

    /// Ask the production router for one path as one signed-in person, in a
    /// browser that has no id yet.
    async fn as_user(app: &Router, token: &str, method: &str, uri: &str) -> Response {
        request_as(app, token, None, method, uri, Body::empty()).await
    }

    /// The same, from one NAMED browser — the pair a selection is keyed on.
    async fn as_browser(
        app: &Router,
        token: &str,
        browser: &str,
        method: &str,
        uri: &str,
    ) -> Response {
        request_as(app, token, Some(browser), method, uri, Body::empty()).await
    }

    async fn request_as(
        app: &Router,
        token: &str,
        browser: Option<&str>,
        method: &str,
        uri: &str,
        body: Body,
    ) -> Response {
        let cookie = match browser {
            Some(browser) => format!("{SESSION_COOKIE}={token}; {BROWSER_COOKIE}={browser}"),
            None => format!("{SESSION_COOKIE}={token}"),
        };
        app.clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(header::COOKIE, cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(body)
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    /// Attach one browser to one machine through the production route.
    async fn select(app: &Router, token: &str, browser: &str, name: &str) -> Response {
        request_as(
            app,
            token,
            Some(browser),
            "POST",
            "/api/devices/select",
            Body::from(format!(r#"{{"name":"{name}"}}"#)),
        )
        .await
    }

    #[tokio::test]
    async fn every_proxied_route_follows_the_device_this_session_selected() {
        let dist = tempfile::tempdir().expect("tempdir");
        let selections_dir = tempfile::tempdir().expect("tempdir");
        let (eric_mini, a1) = spawn_named_daemon("eric-mini").await;
        let (eric_studio, a2) = spawn_named_daemon("eric-studio").await;
        let (ocean_mini, b1) = spawn_named_daemon("ocean-mini").await;
        let (ocean_laptop, b2) = spawn_named_daemon("ocean-laptop").await;

        let mut state = multi_user_state();
        {
            let inner = Arc::get_mut(&mut state).expect("sole owner");
            inner.device_selections = Arc::new(DeviceSelections::load(
                selections_dir.path().join("device-selections.json"),
            ));
            inner.users[0].devices = vec![
                device("mini", &ocean_mini),
                ProxyDevice {
                    is_default: false,
                    ..device("laptop", &ocean_laptop)
                },
            ];
            inner.users[1].devices = vec![
                device("mini", &eric_mini),
                ProxyDevice {
                    is_default: false,
                    ..device("studio", &eric_studio)
                },
            ];
        }
        let selections = state.device_selections.clone();
        let app = build_app(state, dist.path());

        // With nothing chosen, each person lands on their own default machine.
        let eric = as_browser(&app, "tok-eric", "laptop", "GET", "/v1/permissions").await;
        assert!(body_text(eric).await.contains("eric-mini"));
        let ocean = as_browser(&app, "tok-ocean", "desk", "GET", "/v1/permissions").await;
        assert!(body_text(ocean).await.contains("ocean-mini"));

        // Eric picks his studio. This is the whole product: one POST, no
        // second login, no URL typed anywhere.
        assert_eq!(
            select(&app, "tok-eric", "laptop", "studio").await.status(),
            StatusCode::OK
        );

        // Buffered JSON, the SSE tail, and the voice relay all move together.
        let json = as_browser(&app, "tok-eric", "laptop", "GET", "/v1/permissions").await;
        assert!(body_text(json).await.contains("eric-studio"));
        let stream = as_browser(&app, "tok-eric", "laptop", "GET", "/v1/agent/events").await;
        assert!(body_text(stream).await.contains("eric-studio"));
        let voice = request_as(
            &app,
            "tok-eric",
            Some("laptop"),
            "POST",
            "/api/stt",
            Body::from(vec![0_u8, 1, 2, 3]),
        )
        .await;
        assert!(body_text(voice).await.contains("eric-studio"));

        // And nobody else moved. A shared proxy where one person's switch
        // relocates another person's transcript is worse than no switching.
        let ocean = as_browser(&app, "tok-ocean", "desk", "GET", "/v1/permissions").await;
        assert!(body_text(ocean).await.contains("ocean-mini"));

        // Nor did Eric's OTHER browser. A selection is per browser: picking a
        // machine on the phone must not re-point the desktop he left running.
        let phone = as_browser(&app, "tok-eric", "phone", "GET", "/v1/permissions").await;
        assert!(
            body_text(phone).await.contains("eric-mini"),
            "one person's browsers hold their own selections"
        );

        // The choice was recorded server-side against a digest, so a restart
        // keeps it (proven directly in the persistence test above).
        assert_eq!(
            selections
                .selected(&selection_key("tok-eric", "laptop"))
                .as_deref(),
            Some("studio")
        );

        for handle in [a1, a2, b1, b2] {
            handle.abort();
        }
    }

    #[tokio::test]
    async fn a_selection_the_roster_no_longer_has_is_a_typed_503_the_picker_survives() {
        let dist = tempfile::tempdir().expect("tempdir");
        let selections_dir = tempfile::tempdir().expect("tempdir");
        let mut state = multi_user_state();
        {
            let inner = Arc::get_mut(&mut state).expect("sole owner");
            inner.device_selections = Arc::new(DeviceSelections::load(
                selections_dir.path().join("device-selections.json"),
            ));
            // The operator removed 'studio' from the roster while Eric's
            // browser was still attached to it.
            inner
                .device_selections
                .record(&selection_key("tok-eric", "laptop"), "studio");
        }
        let app = build_app(state, dist.path());

        let refused = as_browser(&app, "tok-eric", "laptop", "GET", "/v1/permissions").await;
        assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
        let decoded: Value =
            serde_json::from_str(&body_text(refused).await).expect("typed device error");
        assert_eq!(decoded["error"], "device_unavailable");
        assert_eq!(decoded["reason"], "unknown_device");
        assert_eq!(decoded["device"], "studio");

        // The two routes that let the surface RECOVER must still answer, or a
        // stale selection would be a locked door.
        let config = as_browser(&app, "tok-eric", "laptop", "GET", "/api/config").await;
        assert_eq!(config.status(), StatusCode::OK);
        let devices = as_browser(&app, "tok-eric", "laptop", "GET", "/api/devices").await;
        assert_eq!(devices.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn the_device_list_reports_health_and_whether_anyone_has_chosen() {
        let dist = tempfile::tempdir().expect("tempdir");
        let selections_dir = tempfile::tempdir().expect("tempdir");
        let (live, running) = spawn_named_daemon("eric-mini").await;

        let mut state = multi_user_state();
        {
            let inner = Arc::get_mut(&mut state).expect("sole owner");
            inner.device_selections = Arc::new(DeviceSelections::load(
                selections_dir.path().join("device-selections.json"),
            ));
            inner.users[1].devices = vec![
                device("mini", &live),
                // A closed port stands in for the laptop that is asleep.
                ProxyDevice {
                    is_default: false,
                    ..device("studio", "http://127.0.0.1:9")
                },
            ];
        }
        let app = build_app(state, dist.path());

        let listed = as_browser(&app, "tok-eric", "laptop", "GET", "/api/devices").await;
        let listed: Value = serde_json::from_str(&body_text(listed).await).expect("device list");
        assert_eq!(listed["selected"], "mini");
        assert_eq!(
            listed["selection_explicit"], false,
            "nobody has chosen yet, which is what makes the picker worth showing once",
        );
        let rows = listed["devices"].as_array().expect("rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], "mini");
        assert_eq!(rows[0]["selected"], true);
        assert_eq!(rows[0]["health"]["state"], "ok");
        assert_eq!(rows[0]["health"]["version"], "9.9.9");
        assert_eq!(rows[0]["health"]["rev"], "eric-mini");
        assert_eq!(rows[1]["name"], "studio");
        assert_eq!(rows[1]["health"]["state"], "unreachable");
        // No address anywhere in the payload the browser reads.
        let raw = listed.to_string();
        assert!(
            !raw.contains("http://"),
            "device urls never reach the browser: {raw}"
        );

        assert_eq!(
            select(&app, "tok-eric", "laptop", "studio").await.status(),
            StatusCode::OK
        );

        let listed = as_browser(&app, "tok-eric", "laptop", "GET", "/api/devices").await;
        let listed: Value = serde_json::from_str(&body_text(listed).await).expect("device list");
        assert_eq!(listed["selected"], "studio");
        assert_eq!(listed["selection_explicit"], true, "asked and answered");

        // The OTHER browser is still unasked, and still on the default.
        let phone = as_browser(&app, "tok-eric", "phone", "GET", "/api/devices").await;
        let phone: Value = serde_json::from_str(&body_text(phone).await).expect("device list");
        assert_eq!(phone["selected"], "mini");
        assert_eq!(phone["selection_explicit"], false);

        // A machine this person does not have is a 404, not a silent no-op.
        assert_eq!(
            select(&app, "tok-eric", "laptop", "someone-elses-mac")
                .await
                .status(),
            StatusCode::NOT_FOUND
        );

        running.abort();
    }

    #[tokio::test]
    async fn a_browser_with_no_id_is_given_one_before_it_can_choose() {
        let dist = tempfile::tempdir().expect("tempdir");
        let selections_dir = tempfile::tempdir().expect("tempdir");
        let mut state = multi_user_state();
        {
            let inner = Arc::get_mut(&mut state).expect("sole owner");
            inner.device_selections = Arc::new(DeviceSelections::load(
                selections_dir.path().join("device-selections.json"),
            ));
            inner.users[1].devices = vec![
                device("mini", "http://127.0.0.1:9"),
                ProxyDevice {
                    is_default: false,
                    ..device("studio", "http://127.0.0.1:9")
                },
            ];
        }
        let app = build_app(state, dist.path());

        // Listing devices is the first thing the surface does, and it is where
        // a browser earns the id its choice will be recorded against. Without
        // this, the first selection would key on nothing and land in the row
        // every one of this person's browsers reads.
        let listed = as_user(&app, "tok-eric", "GET", "/api/devices").await;
        let cookie = listed
            .headers()
            .get(header::SET_COOKIE)
            .expect("a browser id is issued")
            .to_str()
            .expect("ascii cookie")
            .to_string();
        assert!(cookie.starts_with(&format!("{BROWSER_COOKIE}=")));
        assert!(
            cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"),
            "the browser id gets the session cookie's hygiene: {cookie}"
        );
        let id = cookie
            .trim_start_matches(&format!("{BROWSER_COOKIE}="))
            .split(';')
            .next()
            .expect("id")
            .to_string();
        assert!(id.len() >= 16, "an id has to be unguessable: {id}");

        // A browser that already has one is not handed another — a new id
        // every request would mean a selection that never survives the next.
        let again = as_browser(&app, "tok-eric", &id, "GET", "/api/devices").await;
        assert!(again.headers().get(header::SET_COOKIE).is_none());

        // And selecting from a browser that never listed still works: it is
        // given an id in that response instead.
        let picked = select(&app, "tok-eric", "", "mini").await;
        assert_eq!(picked.status(), StatusCode::OK);
        assert!(
            picked
                .headers()
                .get(header::SET_COOKIE)
                .is_some_and(|value| value
                    .to_str()
                    .unwrap_or_default()
                    .starts_with(&format!("{BROWSER_COOKIE}="))),
            "a selection from an unknown browser mints its id",
        );
    }

    /// A stream open on the machine being left has to END when the browser
    /// attaches to another one.
    ///
    /// The finding this pins: recording a selection only affects FUTURE
    /// requests, so a tab whose SSE tail is already connected keeps receiving
    /// the old machine's events while its turns and decisions go to the new
    /// one — two machines blended into one transcript, which is exactly what
    /// the session contract forbids. The client reconnects on its own and
    /// lands on the new machine.
    #[tokio::test]
    async fn a_switch_ends_the_stream_that_was_open_on_the_old_machine() {
        // A daemon whose event stream never ends on its own, so the only thing
        // that can end the proxied body is the switch.
        type Frames = tokio::sync::mpsc::Receiver<Result<Bytes, std::io::Error>>;
        async fn held_stream(
            axum::extract::State(frames): axum::extract::State<
                Arc<tokio::sync::Mutex<Option<Frames>>>,
            >,
        ) -> Response {
            let rx = frames.lock().await.take().expect("one subscriber");
            (
                [(header::CONTENT_TYPE, "text/event-stream")],
                Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx)),
            )
                .into_response()
        }

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(4);
        tx.send(Ok(Bytes::from_static(b"event: hello\ndata: mini\n\n")))
            .await
            .expect("first frame");
        // Leaked on purpose: while a sender is alive the receiver never ends,
        // so nothing but the teardown can close this stream.
        std::mem::forget(tx);
        let app = Router::new()
            .route("/v1/agent/events", get(held_stream))
            .with_state(Arc::new(tokio::sync::Mutex::new(Some(rx))));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stub daemon");
        let addr = listener.local_addr().expect("stub addr");
        let upstream = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let dist = tempfile::tempdir().expect("tempdir");
        let selections_dir = tempfile::tempdir().expect("tempdir");
        let mut state = multi_user_state();
        {
            let inner = Arc::get_mut(&mut state).expect("sole owner");
            inner.device_selections = Arc::new(DeviceSelections::load(
                selections_dir.path().join("device-selections.json"),
            ));
            inner.users[1].devices = vec![
                device("mini", &format!("http://{addr}")),
                ProxyDevice {
                    is_default: false,
                    ..device("studio", &format!("http://{addr}"))
                },
            ];
        }
        let app = build_app(state, dist.path());

        let stream = as_browser(&app, "tok-eric", "laptop", "GET", "/v1/agent/events").await;
        assert_eq!(stream.status(), StatusCode::OK);

        // Switch while the tail above is still open.
        let switcher = {
            let app = app.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                select(&app, "tok-eric", "laptop", "studio").await
            })
        };

        // Reading to completion is the assertion: without the teardown this
        // body never ends and the timeout fires.
        let drained = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            axum::body::to_bytes(stream.into_body(), 64 * 1024),
        )
        .await;
        let body = drained
            .expect("the stream must end when the browser attaches elsewhere")
            .expect("read body");
        assert!(String::from_utf8_lossy(&body).contains("mini"));
        assert_eq!(
            switcher.await.expect("switch task").status(),
            StatusCode::OK
        );

        upstream.abort();
    }

    /// The teardown is scoped to ONE selections row.
    ///
    /// The broadcast carries the row key, never a device name: two people can
    /// both be sitting on a machine called "studio" and only one of them
    /// switched. Ending both streams would drop a live transcript belonging to
    /// somebody who did nothing.
    #[tokio::test]
    async fn a_switch_ends_only_the_streams_of_the_browser_that_switched() {
        let state = auth_test_state();
        let mine = ResolvedDaemon {
            selection_key: Some("row-a".to_string()),
            ..fallback_daemon(&state)
        };
        let theirs = ResolvedDaemon {
            selection_key: Some("row-b".to_string()),
            ..fallback_daemon(&state)
        };
        let unswitchable = fallback_daemon(&state);

        let mine = stream_ends_on_switch(&state, &mine).expect("a row to watch");
        let theirs = stream_ends_on_switch(&state, &theirs).expect("a row to watch");
        assert!(
            stream_ends_on_switch(&state, &unswitchable).is_none(),
            "a request that resolved through no row has nothing that could switch under it",
        );

        state
            .selection_changes
            .send("row-a".to_string())
            .expect("two subscribers");

        tokio::time::timeout(std::time::Duration::from_secs(2), mine)
            .await
            .expect("the switching browser's stream ends");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(200), theirs)
                .await
                .is_err(),
            "somebody else's live stream must not end because I switched",
        );
    }

    #[tokio::test]
    async fn the_device_routes_are_behind_the_login_like_every_other_api() {
        let dist = tempfile::tempdir().expect("tempdir");
        let app = build_app(auth_test_state(), dist.path());
        for (method, uri) in [("GET", "/api/devices"), ("POST", "/api/devices/select")] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {uri} must require a session"
            );
        }
    }

    /// Every daemon route must take its upstream from the ONE resolver.
    ///
    /// This is the control the compiler does not hold: a new handler that
    /// builds its URL from `state.daemon_url` compiles, passes every other
    /// test, and quietly pins that one route to the process default — so a
    /// person who switched machines keeps streaming events from the one they
    /// left. The resolver is the auth gate's `ResolvedDaemon` extension, read
    /// either as an extractor or through `resolved_daemon`; nothing else may
    /// name an upstream.
    #[test]
    fn every_daemon_route_resolves_its_upstream_through_one_resolver() {
        let src = include_str!("main.rs");
        let production = src
            .split("#[cfg(test)]")
            .next()
            .expect("production half of the module");
        let router = production
            .split_once("fn build_app(")
            .expect("build_app")
            .1
            .split_once("\n}\n")
            .expect("end of build_app")
            .0;

        // Collect the handlers registered on every /v1/ route.
        let mut handlers: Vec<String> = Vec::new();
        for chunk in router.split(".route(").skip(1) {
            let path = chunk
                .split_once('"')
                .and_then(|(_, rest)| rest.split_once('"'))
                .map(|(path, _)| path)
                .unwrap_or_default();
            if !path.starts_with("/v1/") {
                continue;
            }
            let registration = chunk.split_once(')').map(|(head, _)| head).unwrap_or(chunk);
            for verb in ["get(", "post(", "put(", "patch(", "delete("] {
                let mut rest = registration;
                while let Some((_, tail)) = rest.split_once(verb) {
                    let name: String = tail
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if !name.is_empty() {
                        handlers.push(name);
                    }
                    rest = tail;
                }
            }
        }
        handlers.sort();
        handlers.dedup();
        assert!(
            handlers.len() > 20,
            "the scan found only {} daemon handlers; it stopped matching the router",
            handlers.len()
        );

        for handler in &handlers {
            let signature = format!("async fn {handler}(");
            let body = production
                .split_once(signature.as_str())
                .unwrap_or_else(|| panic!("no handler named {handler}"))
                .1;
            // Up to the next item at column 0 — the whole function.
            let body = body.split("\n}\n").next().unwrap_or(body);
            assert!(
                body.contains("Extension<ResolvedDaemon>")
                    || body.contains("resolved_daemon(&state, &req)"),
                "{handler} does not take its upstream from the request's resolved device",
            );
        }

        // And nothing outside the resolver may name the process-wide upstream.
        let allowed = [
            "fn fallback_daemon(",
            "fn credentials_for_device(",
            "fn devices_for_request(",
            "fn current_selection(",
            "fn devices_for_entry(",
            "fn select_device(",
        ];
        for (index, _) in production.match_indices("state.daemon_url") {
            let preceding = &production[..index];
            let owner = allowed
                .iter()
                .filter_map(|marker| preceding.rfind(marker).map(|at| (at, marker)))
                .max_by_key(|(at, _)| *at);
            let ends_before = preceding.rfind("\n}\n").unwrap_or(0);
            assert!(
                owner.is_some_and(|(at, _)| at > ends_before),
                "state.daemon_url is read outside the device resolver at byte {index}",
            );
        }
    }
}
