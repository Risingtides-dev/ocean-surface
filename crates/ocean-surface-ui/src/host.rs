//! Host bridge — the ONLY module in the surface that knows the Tauri shell
//! exists. Panels and the palette call these typed wrappers; on non-Tauri
//! hosts every call degrades to `None`/`false`/no-op so the same bundle runs
//! unchanged in the browser PWA and the Chrome extension.
//!
//! Contract (frozen — see docs/OCEAN_DESKTOP_NORTH_STAR.md "Host bridge"):
//! signatures do not change. Nothing outside this module may reference
//! `__TAURI_INTERNALS__` or `@tauri-apps` APIs.
//!
//! ## Interop mechanism
//!
//! We talk to Tauri 2 through `window.__TAURI_INTERNALS__` (the same global
//! the daemon uses for `running_as_tauri()` detection). This path is always
//! present in a Tauri webview and requires **no** `tauri.conf.json` change —
//! no `withGlobalTauri`, no CSP relaxation, no extra plugin. Every call is
//! gated on `running_in_tauri()`; on any other host the functions return
//! their documented fallback values.
//!
//! * `__TAURI_INTERNALS__.invoke(cmd, args?)` — typed command invocations.
//!   Args are a JS object whose keys match the Rust `#[tauri::command]`
//!   parameter names. The returned promise resolves to the command's `Ok`
//!   value (already deserialised by Tauri's IPC layer).
//! * `__TAURI_INTERNALS__.event.listen(event, handler)` — subscribes to
//!   shell-emitted events. Each call creates an independent listener
//!   (multiple subscribers are supported natively).
//!
//! Serialisation between the JS values Tauri hands us and the Rust structs
//! (`PathEvent`, `RepoState`, `CommitInfo`) uses a `JSON.stringify` →
//! `serde_json::from_str` round-trip. That is slightly wasteful but
//! self-contained — no extra crate dependency like `serde-wasm-bindgen`.

use js_sys::{Array, Function, Object, Promise, Reflect};
use serde::Deserialize;
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Notification, NotificationOptions, NotificationPermission};

// ── types (frozen) ──────────────────────────────────────────────────────

/// One filesystem change surfaced by the shell watcher (`path-changed`).
/// `kind` is `created` | `modified` | `removed` (renames arrive as a
/// create+remove pair — see `ocean-tauri/src/lib.rs`).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PathEvent {
    pub path: String,
    pub kind: String,
}

/// A recent commit in the session repo, read natively by the shell.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct CommitInfo {
    pub id_short: String,
    pub summary: String,
    pub author: String,
    pub when_epoch: i64,
}

/// Local repo state read natively by the shell (git2 over the working copy —
/// local file reading, NOT a provider call). GH-API depth (PRs, CI) arrives
/// later via daemon tools and extends this struct additively.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct RepoState {
    pub branch: String,
    pub ahead: usize,
    pub behind: usize,
    pub dirty: usize,
    pub staged: usize,
    pub commits: Vec<CommitInfo>,
}

// ── public API ──────────────────────────────────────────────────────────

/// True inside the Tauri shell (WKWebView/WebView2). Wraps the daemon
/// module's detection so callers have one import point.
pub fn running_in_tauri() -> bool {
    crate::daemon::running_as_tauri()
}

/// Open the native folder picker. `None` on non-Tauri hosts or user cancel.
pub async fn pick_folder() -> Option<String> {
    if !running_in_tauri() {
        return None;
    }
    match tauri_invoke("pick_folder", &JsValue::NULL).await {
        Ok(val) if val.is_null() || val.is_undefined() => None,
        Ok(val) => val.as_string(),
        Err(_) => None,
    }
}

/// Start recursive watchers on `paths`; events arrive via `on_path_changed`.
/// Returns false on non-Tauri hosts.
pub async fn watch_paths(paths: &[String]) -> bool {
    if !running_in_tauri() {
        return false;
    }
    let args = Object::new();
    let arr = Array::new();
    for p in paths {
        arr.push(&JsValue::from_str(p));
    }
    let _ = Reflect::set(&args, &JsValue::from_str("paths"), &arr);
    tauri_invoke("watch_paths", &args).await.is_ok()
}

/// Stop watching `paths`. Returns false on non-Tauri hosts.
pub async fn unwatch_paths(paths: &[String]) -> bool {
    if !running_in_tauri() {
        return false;
    }
    let args = Object::new();
    let arr = Array::new();
    for p in paths {
        arr.push(&JsValue::from_str(p));
    }
    let _ = Reflect::set(&args, &JsValue::from_str("paths"), &arr);
    tauri_invoke("unwatch_paths", &args).await.is_ok()
}

/// Subscribe to shell `path-changed` events. No-op on non-Tauri hosts.
/// Multiple subscribers are allowed; callbacks run on the main thread.
pub fn on_path_changed(cb: impl Fn(PathEvent) + 'static) {
    if !running_in_tauri() {
        return;
    }
    wasm_bindgen_futures::spawn_local(tauri_listen("path-changed", move |payload| {
        let event = jsval_to::<PathEvent>(&payload);
        cb(event);
    }));
}

/// Subscribe to shell `menu-command` events, fired when the user picks an item
/// from the native app-menu "Commands" submenu. The payload is the command id
/// string (e.g. `"toggle-files"`); the handler routes it to
/// `CommandRegistry::run`. No-op on non-Tauri hosts — the browser PWA and the
/// extension have no native menubar, so there is nothing to listen for. As with
/// `on_path_changed`, multiple subscribers are allowed and callbacks run on the
/// main thread.
pub fn on_menu_command(handler: impl Fn(String) + 'static) {
    if !running_in_tauri() {
        return;
    }
    wasm_bindgen_futures::spawn_local(tauri_listen("menu-command", move |payload| {
        let id = jsval_to::<String>(&payload);
        handler(id);
    }));
}

/// Tell the Tauri shell the webview has attached its `menu-command` listener,
/// so native app-menu selections should stop being queued and any that fired
/// pre-attach replayed. Called once from the app-menu Effect in app.rs,
/// strictly after `on_menu_command` registers its subscriber — both IPCs
/// share the webview's single FIFO channel, so the subscriber lands before
/// the shell drains. No-op on non-Tauri hosts — the replay buffer only
/// exists behind the shell.
pub async fn notify_ui_ready() {
    if !running_in_tauri() {
        return;
    }
    let _ = tauri_invoke("ui_ready", &JsValue::NULL).await;
}

/// Subscribe to shell `deep-link` events. The shell emits these — one per
/// `ocean://` URL the OS asked Ocean to open — carrying the raw URL string as
/// the payload; the handler parses it into an action (e.g.
/// `ocean://session/<id>` → select that session). No-op on non-Tauri hosts:
/// `ocean://` is only registered with the OS for the bundled native app, so
/// the browser PWA and extension never receive these. As with
/// `on_path_changed` / `on_menu_command`, multiple subscribers are allowed and
/// callbacks run on the main thread.
pub fn on_deep_link(handler: impl Fn(String) + 'static) {
    if !running_in_tauri() {
        return;
    }
    wasm_bindgen_futures::spawn_local(tauri_listen("deep-link", move |payload| {
        let url = jsval_to::<String>(&payload);
        handler(url);
    }));
}

/// Read local repo state for the repo containing `root`. `None` on non-Tauri
/// hosts or when `root` is not inside a git repository.
pub async fn repo_state(root: &str) -> Option<RepoState> {
    if !running_in_tauri() {
        return None;
    }
    let args = Object::new();
    let _ = Reflect::set(&args, &JsValue::from_str("root"), &JsValue::from_str(root));
    match tauri_invoke("repo_state", &args).await {
        Ok(val) if val.is_null() || val.is_undefined() => None,
        Ok(val) => jsval_to::<RepoState>(&val).into(),
        Err(_) => None,
    }
}

/// Show a desktop notification. On the Tauri shell the notification plugin
/// polyfills `window.Notification` (its `init()` overwrites the global so the
/// standard Web API routes through the OS notification center — the same
/// path the plugin's own guest bindings take); in the browser PWA and the
/// extension the same call IS the standard Web Notifications API. One
/// implementation serves both hosts: we request permission on the first call
/// (while it is still `"default"`), then post the notification once granted.
/// Any failure (no window, denied/withheld permission, constructor throw)
/// degrades to a silent no-op so a notification hiccup can never break the
/// turn-completion flow that calls this.
pub async fn notify(title: &str, body: &str) {
    // Defensive: if the host exposes no `window.Notification` at all (shell
    // plugin failed to init, or an embedder without the API), drop the
    // notification instead of throwing through wasm-bindgen.
    let Some(win) = web_sys::window() else { return };
    if !Reflect::has(&win, &JsValue::from_str("Notification")).unwrap_or(false) {
        return;
    }
    // `permission()` reads the polyfilled `window.Notification.permission` on
    // Tauri and the standard permission state on the web.
    if Notification::permission() == NotificationPermission::Default {
        if let Ok(promise) = Notification::request_permission() {
            let _ = JsFuture::from(promise).await;
        }
    }
    if Notification::permission() != NotificationPermission::Granted {
        return;
    }
    let options = NotificationOptions::new();
    options.set_body(body);
    let _ = Notification::new_with_options(title, &options);
}

/// Set the dock/taskbar badge count. `Some(n)` displays `n` on the macOS dock
/// icon (the shell's `set_badge` command maps to `Window::set_badge_count`);
/// `None` clears it. No-op on non-Tauri hosts — browsers expose no portable
/// badge API, so the count is simply dropped.
pub async fn set_badge(count: Option<i64>) {
    if !running_in_tauri() {
        return;
    }
    let args = Object::new();
    // `i64` has no exact JS-number mapping; badge counts are tiny integers,
    // so an f64 round-trips exactly and Tauri deserialises it back to
    // `Option<i64>`. `None` is sent as `null` so the command clears the badge.
    let count_val = match count {
        Some(n) => JsValue::from_f64(n as f64),
        None => JsValue::NULL,
    };
    let _ = Reflect::set(&args, &JsValue::from_str("count"), &count_val);
    let _ = tauri_invoke("set_badge", &args).await;
}

// ── daemon supervision (Tauri shell only) ────────────────────────────────
//
// The Tauri shell owns the ocean-daemon lifecycle. These wrappers expose the
// four supervision commands + the `daemon-status` event stream. Every call
// degrades on non-Tauri hosts: `None`/`false`/no-op, so the browser PWA and
// the Chrome extension (which talk to an already-running daemon) are
// unaffected.

/// Shell-reported daemon supervision state, mirroring the Tauri side's
/// `DaemonStatusDto`. `state` is one of `running` | `stopped` | `starting` |
/// `unreachable`; `pid` is the spawned child's pid when the shell owns the
/// process, else `None` (an external daemon it can reach but didn't start).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct DaemonStatus {
    pub state: String,
    #[serde(default)]
    pub pid: Option<u32>,
}

/// Current daemon supervision state (cached from the shell's background
/// poller). `None` on non-Tauri hosts — the browser/extension never supervise
/// a daemon, so there is no shell state to report.
pub async fn daemon_status() -> Option<DaemonStatus> {
    if !running_in_tauri() {
        return None;
    }
    match tauri_invoke("daemon_status", &JsValue::NULL).await {
        Ok(val) if !val.is_null() && !val.is_undefined() => Some(jsval_to::<DaemonStatus>(&val)),
        _ => None,
    }
}

/// Start the ocean-daemon the shell supervises. `bin` overrides
/// `OCEAN_DAEMON_BIN` → PATH resolution. On Tauri, spawning onto an
/// already-served port is a graceful no-op (the shell never launches a second
/// daemon). Returns `false` on non-Tauri hosts or if the invoke rejects.
pub async fn daemon_start(bin: Option<String>) -> bool {
    if !running_in_tauri() {
        return false;
    }
    let args = Object::new();
    if let Some(b) = bin {
        let _ = Reflect::set(&args, &JsValue::from_str("bin"), &JsValue::from_str(&b));
    }
    tauri_invoke("daemon_start", &args).await.is_ok()
}

/// Stop the daemon the shell owns. Never affects an external daemon the shell
/// didn't spawn. Returns `false` on non-Tauri hosts.
#[allow(dead_code)] // host-seam symmetry with daemon_start/daemon_restart; no surface affordance wires stop yet
pub async fn daemon_stop() -> bool {
    if !running_in_tauri() {
        return false;
    }
    tauri_invoke("daemon_stop", &JsValue::NULL).await.is_ok()
}

/// Restart the shell-owned daemon (stop + start). `bin` overrides
/// `OCEAN_DAEMON_BIN` → PATH. Returns `false` on non-Tauri hosts.
pub async fn daemon_restart(bin: Option<String>) -> bool {
    if !running_in_tauri() {
        return false;
    }
    let args = Object::new();
    if let Some(b) = bin {
        let _ = Reflect::set(&args, &JsValue::from_str("bin"), &JsValue::from_str(&b));
    }
    tauri_invoke("daemon_restart", &args).await.is_ok()
}

/// Subscribe to shell `daemon-status` events (emitted on change only). No-op
/// on non-Tauri hosts. The callback receives the latest [`DaemonStatus`];
/// multiple subscribers are allowed.
pub fn on_daemon_status(cb: impl Fn(DaemonStatus) + 'static) {
    if !running_in_tauri() {
        return;
    }
    wasm_bindgen_futures::spawn_local(tauri_listen("daemon-status", move |payload| {
        let status = jsval_to::<DaemonStatus>(&payload);
        cb(status);
    }));
}

// ── open_externally (B0: Open Externally) ───────────────────────────────

/// Open a file with the OS default application (Tauri only).
/// Returns `false` on non-Tauri hosts (no-op).
///
/// * `root` — session workspace root, used by the shell to gate path escape.
/// * `path` — absolute path of the file to open.
pub async fn open_externally(root: &str, path: &str) -> bool {
    if !running_in_tauri() {
        return false;
    }
    let args = Object::new();
    if Reflect::set(&args, &JsValue::from_str("root"), &JsValue::from_str(root)).is_err()
        || Reflect::set(&args, &JsValue::from_str("path"), &JsValue::from_str(path)).is_err()
    {
        return false;
    }
    tauri_invoke("open_file", &args).await.is_ok()
}

// ── internals ───────────────────────────────────────────────────────────

/// Low-level: call `__TAURI_INTERNALS__.invoke(cmd, args)` and await the
/// returned promise.  On the JS side the promise resolves to the command's
/// `Ok` payload (already JSON-deserialised by Tauri's IPC layer) or rejects
/// with the error string.
async fn tauri_invoke(cmd: &str, args: &JsValue) -> Result<JsValue, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let internals = Reflect::get(&window, &JsValue::from_str("__TAURI_INTERNALS__"))?;
    let invoke_val = Reflect::get(&internals, &JsValue::from_str("invoke"))?;
    let invoke_fn: Function = invoke_val
        .dyn_into()
        .map_err(|_| JsValue::from_str("invoke is not a function"))?;

    let call_args = Array::new();
    call_args.push(&JsValue::from_str(cmd));
    call_args.push(args);

    let result = Reflect::apply(&invoke_fn, &internals, &call_args)?;
    JsFuture::from(Promise::from(result)).await
}

/// Low-level: subscribe to a Tauri shell event via
/// `__TAURI_INTERNALS__.event.listen(event_name, handler)`.  `handler`
/// receives the **payload** field of the Tauri event object (the
/// `{ event, id, payload }` wrapper is stripped).  The returned `UnlistenFn`
/// is leaked so the subscription lives for the lifetime of the app.
async fn tauri_listen<F>(event_name: &str, handler: F)
where
    F: Fn(JsValue) + 'static,
{
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let internals = match Reflect::get(&window, &JsValue::from_str("__TAURI_INTERNALS__")) {
        Ok(i) => i,
        Err(_) => return,
    };
    let event_obj = match Reflect::get(&internals, &JsValue::from_str("event")) {
        Ok(e) => e,
        Err(_) => return,
    };
    let listen: Function = match Reflect::get(&event_obj, &JsValue::from_str("listen")) {
        Ok(f) => match f.dyn_into() {
            Ok(f) => f,
            Err(_) => return,
        },
        Err(_) => return,
    };

    let closure = Closure::wrap(Box::new(move |raw: JsValue| {
        // Tauri's event object: { event, id, payload }.  Unwrap payload.
        if let Ok(payload) = Reflect::get(&raw, &JsValue::from_str("payload")) {
            handler(payload);
        }
    }) as Box<dyn Fn(JsValue)>);

    let call_args = Array::new();
    call_args.push(&JsValue::from_str(event_name));
    call_args.push(closure.as_ref().unchecked_ref());

    if let Ok(promise) = Reflect::apply(&listen, &event_obj, &call_args) {
        if JsFuture::from(Promise::from(promise)).await.is_ok() {
            closure.forget(); // subscription lives forever
        }
    }
}

/// Round-trip a JsValue through JSON to a Rust `Deserialize` type.
fn jsval_to<T: serde::de::DeserializeOwned + Default>(val: &JsValue) -> T {
    let json = js_sys::JSON::stringify(val).unwrap_or_default();
    let s = json.as_string().unwrap_or_default();
    // Use a default on parse failure so a single malformed payload
    // doesn't poison the subscriber callback chain.
    serde_json::from_str(&s).unwrap_or_else(|_| serde_json::from_str("null").unwrap_or_default())
}
