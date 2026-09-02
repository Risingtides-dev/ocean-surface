//! Native backend for the ocean-surface Tauri shell.
//!
//! Hosts the same Leptos WASM bundle (`../../dist`) the browser PWA loads and
//! exposes the native affordances GPUI used to provide: a system folder picker
//! (replaces `rfd`) and recursive path watchers that debounce filesystem events
//! back to the webview as `path-changed` (replaces `ocean-gui/shell/watcher.rs`).

use std::collections::HashMap;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use git2::Repository;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

/// One surfaced filesystem change, serialized to the webview as `path-changed`.
///
/// `kind` is created/modified/removed. notify surfaces a rename as a
/// create+remove pair, which the frontend reassembles — there is no separate
/// "renamed" event at the OS layer that we can report portably.
#[derive(Clone, Serialize)]
struct PathEvent {
    path: String,
    kind: String,
}

/// Live watchers keyed by canonical path. The value is the spawned debounce
/// task's handle; aborting it drops the `RecommendedWatcher` the task owns,
/// which unregisters the OS watch. (A bare channel sender, as an earlier draft
/// sketched, cannot actually stop a watcher — a task handle can.)
/// Shell-managed state. `daemon` is an `Arc` so the background liveness poller
/// (spawned from `run`) shares the same handle as the command handlers; the
/// `Arc` is constructed explicitly in `run` (not via `Default`) because the
/// poller's host:port comes from `OCEAN_DAEMON_URL`.
/// Readiness gate + replay buffer for native menu selections that fire
/// before the webview's `menu-command` listener attaches. Tauri drops events
/// with no subscriber, so boot-time app-menu clicks (which can arrive the
/// instant the native menu is wired, well before WASM mounts) would vanish.
/// `pending` holds them in arrival order; `ui_ready` flips `ready` and
/// drains, so replayed commands run in the order the user clicked them.
struct MenuBridge {
    ready: bool,
    pending: Vec<String>,
}

struct AppState {
    watchers: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
    daemon: Arc<DaemonSup>,
    menu: Mutex<MenuBridge>,
}

fn kind_str(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::Create(_) => "created",
        EventKind::Remove(_) => "removed",
        EventKind::Modify(_) => "modified",
        EventKind::Access(_) => "modified",
        EventKind::Any | EventKind::Other => "modified",
    }
}

/// System folder picker. Returns the chosen path or `None` if cancelled.
/// Replaces GPUI's `rfd` flow in `ocean-gui/src/shell/surface.rs`.
#[tauri::command]
async fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    // tauri-plugin-dialog's picker is callback-based (the native modal loop
    // runs on the app's main thread); bridge into the async command with a
    // oneshot so the caller can `.await` the result.
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |folder| {
        let _ = tx.send(folder);
    });
    let folder = rx
        .await
        .map_err(|_| "folder picker channel closed".to_string())?;
    Ok(folder.map(|f| f.to_string()))
}

/// A requested path that could not be admitted to the watcher set, with the
/// reason. Serialized to the webview so a partial or zero-watch batch stays
/// legible instead of collapsing to a single bool.
#[derive(Clone, Serialize)]
struct WatchFailure {
    /// The raw path as requested by the caller.
    path: String,
    /// Human-readable admission error (canonicalize / create / watch).
    error: String,
}

/// Aggregate outcome of a [`watch_paths`] admission pass. `watched` holds the
/// canonical keys now actively watched by this call (duplicates collapsed);
/// `failed` holds the paths that could not be admitted. The two vectors let the
/// host wrapper distinguish full success (`failed` empty), partial success
/// (both non-empty), and zero active watchers (`watched` empty, `failed` not).
#[derive(Clone, Serialize)]
struct WatchOutcome {
    watched: Vec<String>,
    failed: Vec<WatchFailure>,
}

/// Resolve a request path to the canonical key used in the watcher map. While
/// the path exists this is a plain `canonicalize`. Once it is deleted (an
/// unwatch after removal), `canonicalize` fails, so fall back to canonicalizing
/// the parent and re-joining the final component — that still matches the key
/// installed while the path existed (e.g. macOS `/tmp` → `/private/tmp` symlink
/// resolution). Only a path whose parent is also gone degrades to the raw
/// string.
fn resolve_watch_key(raw: &str) -> String {
    let path = Path::new(raw);
    if let Ok(canonical) = path.canonicalize() {
        return canonical.to_string_lossy().into_owned();
    }
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        if let Ok(canonical_parent) = parent.canonicalize() {
            return canonical_parent.join(name).to_string_lossy().into_owned();
        }
    }
    raw.to_string()
}

/// Admit each requested path to the watcher map independently, returning a
/// typed per-batch outcome. Pure over the handle type `H` and its three
/// injected effects, so the admission policy is unit-tested without a real
/// filesystem, notify watcher, or Tauri runtime:
///
/// * `canonicalize` — map a raw path to its watcher-map key or an error; a
///   missing/stale entry fails here and the loop continues to later paths;
/// * `install` — build and register the watcher for a key, returning its
///   handle (called at most once per unique key in the batch);
/// * `retire` — dispose of the handle a successful re-watch replaced.
///
/// A replacement never removes the prior watcher until the new one is live, so
/// a failed `install` leaves the map — and the existing watcher for that key —
/// untouched.
fn admit_watches<H>(
    watchers: &mut HashMap<String, H>,
    paths: Vec<String>,
    canonicalize: impl Fn(&str) -> Result<String, String>,
    mut install: impl FnMut(&str) -> Result<H, String>,
    mut retire: impl FnMut(H),
) -> WatchOutcome {
    let mut watched: Vec<String> = Vec::new();
    let mut failed: Vec<WatchFailure> = Vec::new();
    for raw in paths {
        let key = match canonicalize(&raw) {
            Ok(key) => key,
            Err(error) => {
                failed.push(WatchFailure { path: raw, error });
                continue;
            }
        };
        // Duplicate within this batch: the first occurrence already installed
        // (or replaced) the watcher for this key, so don't build a rival.
        if watched.contains(&key) {
            continue;
        }
        match install(&key) {
            Ok(handle) => {
                if let Some(prior) = watchers.insert(key.clone(), handle) {
                    retire(prior);
                }
                watched.push(key);
            }
            Err(error) => {
                // The map was not touched, so any prior watcher for this key is
                // still installed and running.
                failed.push(WatchFailure { path: raw, error });
            }
        }
    }
    WatchOutcome { watched, failed }
}

/// Watch the given paths recursively, emitting `path-changed` events to the
/// webview, coalesced to a 200ms quiet window so editor saves don't fan out
/// into a burst. Replaces `ocean-gui/src/shell/watcher.rs`.
///
/// Each path is admitted independently (see [`admit_watches`]): a missing or
/// stale entry is recorded and skipped rather than aborting the batch, so later
/// valid paths are still watched. Returns a [`WatchOutcome`] the host wrapper
/// classifies into full / partial / zero-watchable.
#[tauri::command]
async fn watch_paths(
    paths: Vec<String>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<WatchOutcome, String> {
    let mut watchers = state.watchers.lock();

    let canonicalize = |raw: &str| -> Result<String, String> {
        Path::new(raw)
            .canonicalize()
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(|e| format!("{raw}: {e}"))
    };

    let install = |key: &str| -> Result<tauri::async_runtime::JoinHandle<()>, String> {
        // notify's callback runs on its own thread, so bridge into the async
        // world with a bounded channel; try_send keeps that thread non-blocking
        // (drops under backpressure, which the debounce absorbs anyway).
        let (tx, mut rx) = tokio::sync::mpsc::channel::<PathEvent>(64);
        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<notify::Event>| {
                let Ok(event) = res else { return };
                let kind = kind_str(&event.kind).to_owned();
                for p in &event.paths {
                    let _ = tx.try_send(PathEvent {
                        path: p.to_string_lossy().into_owned(),
                        kind: kind.clone(),
                    });
                }
            },
            notify::Config::default(),
        )
        .map_err(|e| e.to_string())?;

        watcher
            .watch(Path::new(key), RecursiveMode::Recursive)
            .map_err(|e| format!("{key}: {e}"))?;

        let emit_app = app.clone();
        let handle = tauri::async_runtime::spawn(async move {
            // `watcher` must outlive the task or the OS watch is torn down.
            let _watcher = watcher;
            let mut buffer: Vec<PathEvent> = Vec::new();
            loop {
                tokio::select! {
                    ev = rx.recv() => match ev {
                        Some(e) => buffer.push(e),
                        None => break,
                    },
                    _ = tokio::time::sleep(Duration::from_millis(200)),
                        if !buffer.is_empty() =>
                    {
                        for e in buffer.drain(..) {
                            let _ = emit_app.emit("path-changed", e);
                        }
                    }
                }
            }
        });
        Ok(handle)
    };

    let outcome = admit_watches(&mut *watchers, paths, canonicalize, install, |handle| {
        handle.abort()
    });
    Ok(outcome)
}

/// Stop watching the given paths. Paths resolve to the same canonical key the
/// watcher was installed under — including after the path has been deleted (see
/// [`resolve_watch_key`]) — so an unwatch that follows a removal still tears the
/// watcher down instead of leaking it.
#[tauri::command]
async fn unwatch_paths(paths: Vec<String>, state: State<'_, AppState>) -> Result<(), String> {
    let mut watchers = state.watchers.lock();
    for raw in paths {
        let key = resolve_watch_key(&raw);
        if let Some(handle) = watchers.remove(&key) {
            handle.abort();
        }
    }
    Ok(())
}

#[cfg(test)]
mod watch_admission_tests {
    use super::*;
    use std::cell::RefCell;

    /// Canonicalize stub: identity, but any path containing "missing" fails —
    /// the pure stand-in for a stale/absent entry.
    fn canon(raw: &str) -> Result<String, String> {
        if raw.contains("missing") {
            Err(format!("{raw}: not found"))
        } else {
            Ok(raw.to_string())
        }
    }

    #[test]
    fn mixed_batch_watches_valid_and_records_missing() {
        // [valid, missing, valid] must install both valid paths and record the
        // one failure — the earlier bug returned on the missing entry.
        let mut map: HashMap<String, u32> = HashMap::new();
        let mut next = 0u32;
        let outcome = admit_watches(
            &mut map,
            vec!["/a".into(), "/missing".into(), "/b".into()],
            canon,
            |_key| {
                next += 1;
                Ok(next)
            },
            |_h| {},
        );
        assert_eq!(outcome.watched, vec!["/a".to_string(), "/b".to_string()]);
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].path, "/missing");
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("/a") && map.contains_key("/b"));
    }

    #[test]
    fn install_failure_after_canonicalize_is_recorded_not_fatal() {
        // A watcher-construction failure for one key is captured; later keys
        // are still attempted.
        let mut map: HashMap<String, u32> = HashMap::new();
        let outcome = admit_watches(
            &mut map,
            vec!["/a".into(), "/b".into(), "/c".into()],
            canon,
            |key| {
                if key == "/b" {
                    Err("watch construction failed".into())
                } else {
                    Ok(1)
                }
            },
            |_h| {},
        );
        assert_eq!(outcome.watched, vec!["/a".to_string(), "/c".to_string()]);
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].path, "/b");
        assert!(map.contains_key("/a") && map.contains_key("/c"));
        assert!(!map.contains_key("/b"));
    }

    #[test]
    fn replacement_failure_preserves_prior_watcher() {
        // A re-watch whose install fails must leave the prior handle installed
        // and un-retired — the map stays consistent.
        let mut map: HashMap<String, u32> = HashMap::new();
        map.insert("/a".into(), 111);
        let retired = RefCell::new(Vec::new());
        let outcome = admit_watches(
            &mut map,
            vec!["/a".into()],
            canon,
            |_key| Err("could not build watcher".into()),
            |h| retired.borrow_mut().push(h),
        );
        assert!(outcome.watched.is_empty());
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(map.get("/a"), Some(&111));
        assert!(retired.borrow().is_empty());
    }

    #[test]
    fn successful_replacement_retires_prior_handle() {
        // A re-watch that succeeds installs the new handle and retires the old.
        let mut map: HashMap<String, u32> = HashMap::new();
        map.insert("/a".into(), 111);
        let retired = RefCell::new(Vec::new());
        let outcome = admit_watches(
            &mut map,
            vec!["/a".into()],
            canon,
            |_key| Ok(222),
            |h| retired.borrow_mut().push(h),
        );
        assert_eq!(outcome.watched, vec!["/a".to_string()]);
        assert_eq!(map.get("/a"), Some(&222));
        assert_eq!(*retired.borrow(), vec![111]);
    }

    #[test]
    fn duplicate_paths_install_once() {
        // Duplicate keys within a batch install a single watcher.
        let mut map: HashMap<String, u32> = HashMap::new();
        let installs = RefCell::new(0u32);
        let outcome = admit_watches(
            &mut map,
            vec!["/a".into(), "/a".into(), "/a".into()],
            canon,
            |_key| {
                *installs.borrow_mut() += 1;
                Ok(1)
            },
            |_h| {},
        );
        assert_eq!(outcome.watched, vec!["/a".to_string()]);
        assert_eq!(*installs.borrow(), 1);
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn zero_paths_is_noop() {
        // A zero-path call installs nothing and reports an empty outcome.
        let mut map: HashMap<String, u32> = HashMap::new();
        let installs = RefCell::new(0u32);
        let outcome = admit_watches(
            &mut map,
            vec![],
            canon,
            |_key| {
                *installs.borrow_mut() += 1;
                Ok(1)
            },
            |_h| {},
        );
        assert!(outcome.watched.is_empty());
        assert!(outcome.failed.is_empty());
        assert_eq!(*installs.borrow(), 0);
        assert!(map.is_empty());
    }

    #[test]
    fn resolve_watch_key_matches_after_deletion() {
        // The key an unwatch resolves after deletion must equal the key the
        // watch installed while the path existed, so the watcher is removed
        // rather than leaked.
        let dir = tempfile::tempdir().expect("tempdir");
        let sub = dir.path().join("watched-sub");
        std::fs::create_dir(&sub).expect("mkdir sub");
        let raw = sub.to_string_lossy().into_owned();

        let key_when_present = resolve_watch_key(&raw);
        assert_eq!(
            key_when_present,
            sub.canonicalize().unwrap().to_string_lossy()
        );

        std::fs::remove_dir(&sub).expect("rmdir sub");
        let key_after_delete = resolve_watch_key(&raw);
        assert_eq!(
            key_after_delete, key_when_present,
            "unwatch must resolve to the same key after deletion"
        );
    }

    #[test]
    fn watch_outcome_serializes_watched_and_failed() {
        // Wire contract the host wrapper decodes: `watched` + `failed[].path`.
        let outcome = WatchOutcome {
            watched: vec!["/private/tmp/a".into()],
            failed: vec![WatchFailure {
                path: "/nope".into(),
                error: "/nope: not found".into(),
            }],
        };
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["watched"][0].as_str(), Some("/private/tmp/a"));
        assert_eq!(json["failed"][0]["path"].as_str(), Some("/nope"));
        assert_eq!(
            json["failed"][0]["error"].as_str(),
            Some("/nope: not found")
        );
    }
}

// ── repo_state ────────────────────────────────────────────────────────────

/// Lightweight commit record shipped to the webview.
#[derive(Clone, Serialize)]
struct CommitInfoDto {
    id_short: String,
    summary: String,
    author: String,
    #[serde(rename = "when_epoch")]
    when_epoch: i64,
}

/// Local repo snapshot shipped to the webview.  Field names are camelCase in
/// JSON (the default with serde + `#[serde(rename = …)]` on the two fields
/// whose Rust name diverges) so the wasm-side `#[derive(Deserialize)]` structs
/// match without any rename annotations on that side.
#[derive(Clone, Serialize)]
struct RepoStateDto {
    branch: String,
    ahead: usize,
    behind: usize,
    dirty: usize,
    staged: usize,
    commits: Vec<CommitInfoDto>,
}

/// Read local repo state for the repo containing `root`.  Returns `None`
/// when `root` is not inside a git repository.
#[tauri::command]
fn repo_state(root: String) -> Result<Option<RepoStateDto>, String> {
    let repo = match Repository::discover(&root) {
        Ok(r) => r,
        Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(e) => return Err(format!("git error: {e}")),
    };

    let branch = repo_head_name(&repo);
    let (ahead, behind) = repo_ahead_behind(&repo);
    let (dirty, staged) = repo_status_counts(&repo);
    let commits = repo_recent_commits(&repo, 8);

    Ok(Some(RepoStateDto {
        branch,
        ahead,
        behind,
        dirty,
        staged,
        commits,
    }))
}

// ── dock badge ────────────────────────────────────────────────────────────

/// Set or clear the dock-icon badge. `Some(n)` shows `n`; `None` clears.
///
/// The wasm side maps the pending permission-prompt count onto this so an
/// agent waiting on approval is visible from the dock (host.rs `set_badge`).
#[tauri::command]
fn set_badge(app: AppHandle, count: Option<i64>) -> Result<(), String> {
    let Some(win) = app.get_webview_window("main") else {
        return Ok(());
    };
    win.set_badge_count(count).map_err(|e| e.to_string())
}

// ── open_file (B0: Open Externally) ───────────────────────────────────────

/// File types macOS `open(1)` EXECUTES rather than opens in an application
/// (TASK-85). `opener::open` is `open(1)` on macOS, so handing it one of
/// these runs code.
const EXECUTING_EXTENSIONS: &[&str] = &[
    "command",     // shell script run by Terminal
    "terminal",    // Terminal settings file that runs a command
    "workflow",    // Automator, runs actions
    "app",         // bundle (also excluded by the is_file check, belt+braces)
    "scpt",        // compiled AppleScript
    "applescript", // AppleScript source
    "action",      // Automator action bundle
    "osascript",
];

/// True when opening this target would EXECUTE it rather than view it —
/// either because macOS treats the extension as runnable, or because the
/// file carries an executable bit (a `#!` script `open(1)` will hand to a
/// shell).
fn opens_as_executable(target: &Path) -> bool {
    if let Some(ext) = target.extension().and_then(|e| e.to_str()) {
        let ext = ext.to_ascii_lowercase();
        if EXECUTING_EXTENSIONS.iter().any(|e| *e == ext) {
            return true;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if let Ok(meta) = std::fs::metadata(target) {
            if meta.permissions().mode() & 0o111 != 0 {
                return true;
            }
        }
    }
    false
}

/// Open a file with the OS default application.
///
/// * `root` — session workspace root, as the CALLER believes it to be.
/// * `path` — absolute path of the file to open.
///
/// SECURITY (TASK-85): the `root` containment check below is defence in
/// depth ONLY, and deliberately documented as such. Both `root` and `path`
/// arrive from the same IPC caller, so `target.starts_with(&root)` is
/// self-satisfiable — invoke with `root: "/"` and the check passes for any
/// absolute path. Tauri 2's capability ACL does not gate
/// `generate_handler!` commands, so nothing upstream constrains it either.
/// Making `root` trustworthy would mean the shell independently knowing the
/// session workspace, which it does not today.
///
/// So the real gate is the consequence, not the location: this refuses to
/// open anything macOS would EXECUTE (`opens_as_executable`). That closes
/// the primitive that matters here — the daemon runs tools ungated, so it
/// can already write a payload and mark it executable; without this check
/// `open_file` was the launcher. Viewing a document in its default app,
/// which is what this command exists for, is unaffected.
#[tauri::command]
fn open_file(root: String, path: String) -> Result<(), String> {
    let root = Path::new(&root)
        .canonicalize()
        .map_err(|e| format!("canonicalize root: {e}"))?;
    let target = Path::new(&path)
        .canonicalize()
        .map_err(|e| format!("canonicalize path: {e}"))?;
    // Defence in depth — see the SECURITY note above for why this is not
    // sufficient on its own.
    if !target.starts_with(&root) {
        return Err("path escapes workspace root".into());
    }
    if !target.is_file() {
        return Err("path is not a regular file".into());
    }
    // The load-bearing check.
    if opens_as_executable(&target) {
        return Err("refusing to open an executable file".into());
    }
    opener::open(&target).map_err(|e| e.to_string())
}

/// Mark the webview's `menu-command` listener as attached and replay any
/// native app-menu selections that fired before it registered. Called once
/// from the wasm bundle (host::notify_ui_ready) right after `on_menu_command`
/// registers its subscriber — by then the burst of boot-time menu clicks that
/// would otherwise have been dropped is sitting in `pending`. Drains in
/// arrival order so replayed commands run in the order the user clicked them.
///
/// Race safety: both the `event.listen` IPC (registering the subscriber) and
/// this command travel the webview's single FIFO IPC channel, and `listen` is
/// dispatched first (see app.rs), so the subscriber is registered before the
/// drain emits — no replayed event is lost.
#[tauri::command]
fn ui_ready(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let pending = {
        let mut bridge = state.menu.lock();
        bridge.ready = true;
        std::mem::take(&mut bridge.pending)
    };
    for id in pending {
        let _ = app.emit("menu-command", id);
    }
    Ok(())
}

/// Resize the native window during an explicitly opted-in UI diagnostic run.
/// The command is unavailable in ordinary launches even though it remains in
/// the static invoke table, preventing product code from using test authority.
#[tauri::command]
fn ui_debug_resize(window: tauri::WebviewWindow, width: f64, height: f64) -> Result<(), String> {
    if std::env::var_os("OCEAN_UI_DEBUG_SCRIPT").is_none() {
        return Err("UI diagnostics are not enabled".into());
    }
    window
        .set_size(tauri::LogicalSize::new(width, height))
        .map_err(|error| error.to_string())
}

// ── daemon supervision ───────────────────────────────────────────────────
//
// The Tauri shell owns the ocean-daemon lifecycle when nothing else is
// serving on `OCEAN_DAEMON_URL` (default 127.0.0.1:4780, mirroring the
// daemon's own `OCEAN_BIND`). Supervision is EXPLICIT in v1 — there is no
// auto-spawn at boot; the user starts/restarts from the palette or the tray,
// and a background poller reports liveness to the webview as `daemon-status`
// events (emitted on change only).
//
// `ocean-daemon` is the binary in ocean-os/crates/ocean-daemon
// (`[[bin]] name = "ocean-daemon"`), served via `axum::serve` over a
// TcpListener — so a raw TCP connect to host:port is a sufficient liveness
// probe without pulling in reqwest.

/// Liveness probe timeout. The daemon binds its port within tens of ms of
/// boot, so half a second is ample and keeps the poller (and the explicit
/// start-check) from stalling.
const DAEMON_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
/// Poll interval for the background liveness poller.
const DAEMON_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Default daemon URL — matches the wasm surface's `DEFAULT_DAEMON_URL` and
/// the daemon's `OCEAN_BIND` default (`127.0.0.1:4780`). Reading
/// `OCEAN_DAEMON_URL` keeps the shell and the wasm bundle pointed at one
/// configurable origin.
const DEFAULT_DAEMON_URL: &str = "http://127.0.0.1:4780";

/// Liveness state of the supervised daemon, reported to the webview. Wire
/// discriminants are lowercase via `rename_all` — `running` | `stopped` |
/// `starting` | `unreachable`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize)]
#[serde(rename_all = "lowercase")]
enum DaemonState {
    /// Port reachable. `pid` is present iff the shell owns the child.
    Running,
    /// No child and the port is not reachable (never started / just stopped).
    /// Default: the shell reports Stopped before its first poll.
    #[default]
    Stopped,
    /// The shell owns a live child but the port is not up yet (just spawned).
    Starting,
    /// Port not reachable after previously being up, with no owned child —
    /// an external daemon that went away.
    Unreachable,
}

/// Pure state decision (unit-tested, no I/O). Maps the raw signals — port
/// reachable, owned child alive, port ever reached — to the reported state.
fn decide_daemon_state(reachable: bool, child_alive: bool, reachable_ever: bool) -> DaemonState {
    match (reachable, child_alive) {
        (true, _) => DaemonState::Running,
        (false, true) => DaemonState::Starting,
        (false, false) => {
            if reachable_ever {
                DaemonState::Unreachable
            } else {
                DaemonState::Stopped
            }
        }
    }
}

/// Payload for the `daemon-status` event and the `daemon_status` command.
/// `pid` is omitted on the wire when the shell doesn't own the child (an
/// external daemon it can reach but didn't spawn).
#[derive(Clone, Serialize)]
struct DaemonStatusDto {
    state: DaemonState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
}

/// Supervised ocean-daemon process + liveness tracking. Shared between the
/// command handlers and the background poller via `Arc` (cloned into the
/// poller thread; the original lives in [`AppState`]). Every method acquires
/// at most one inner mutex at a time, so there is no lock-ordering hazard
/// across `probe` / `snapshot` / `start` / `stop`.
struct DaemonSup {
    /// The child we spawned. `std::process::Child` does NOT kill on drop, so
    /// the daemon survives a shell crash — supervision is explicit
    /// (start/stop/restart), never `kill_on_drop`.
    child: Mutex<Option<Child>>,
    /// Last state computed by the poller; `daemon_status` reads this so the
    /// command stays non-blocking (the poller is the freshness mechanism).
    last: Mutex<DaemonState>,
    /// Set once the port has ever been reachable, so a later loss of
    /// reachability with no owned child reads as `Unreachable` (an external
    /// daemon that stopped) rather than `Stopped` (never started).
    reachable_ever: Mutex<bool>,
    /// host:port parsed from the daemon URL — the probe target.
    host: String,
    port: u16,
}

impl DaemonSup {
    fn new(host: String, port: u16) -> Self {
        Self {
            child: Mutex::new(None),
            last: Mutex::new(DaemonState::Stopped),
            reachable_ever: Mutex::new(false),
            host,
            port,
        }
    }

    /// pid of the owned child, if any (`Child::id`).
    fn pid(&self) -> Option<u32> {
        self.child.lock().as_ref().map(|c| c.id())
    }

    /// True iff we own a child that has not yet exited — `try_wait` returns
    /// `Ok(None)` while the process is still running.
    fn child_alive(&self) -> bool {
        self.child
            .lock()
            .as_mut()
            .map(|c| c.try_wait().ok().flatten().is_none())
            .unwrap_or(false)
    }

    /// Kill + reap the owned child if any, then clear the slot. `kill` errors
    /// (already-exited) are ignored; `wait` reaps any zombie so the OS does
    /// not retain it. Blocking — call only on an explicit stop/restart.
    fn reap_child(&self) {
        if let Some(mut child) = self.child.lock().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Is the daemon port currently accepting connections? Blocking (short
    /// timeout) — call from the poller thread or an explicit user action.
    fn reachable(&self) -> bool {
        tcp_reachable(&self.host, self.port)
    }

    /// Start the daemon unless we already own a live child or the port is
    /// already served — a graceful no-op in both cases so we never spawn a
    /// second daemon that would fail to bind.
    fn start(&self, bin: &str) -> Result<(), String> {
        if self.child_alive() {
            return Ok(());
        }
        if self.reachable() {
            // An external daemon is already serving — don't spawn a rival.
            return Ok(());
        }
        self.reap_child();
        let child = spawn_daemon(bin)?;
        *self.child.lock() = Some(child);
        Ok(())
    }

    /// Stop the daemon the shell owns. Never touches an external daemon.
    fn stop(&self) {
        self.reap_child();
    }

    /// Restart the shell-owned daemon (stop + start). On an external daemon
    /// this reduces to a start that no-ops because the port is served.
    fn restart(&self, bin: &str) -> Result<(), String> {
        self.stop();
        self.start(bin)
    }

    /// Recompute liveness from a TCP probe + child state, persisting the
    /// result as `last`. Blocking — poller-thread only.
    fn probe(&self) -> DaemonState {
        let reachable = self.reachable();
        let alive = self.child_alive();
        let ever = *self.reachable_ever.lock();
        let state = decide_daemon_state(reachable, alive, ever);
        if reachable {
            *self.reachable_ever.lock() = true;
        }
        if !alive {
            // Reap a dead slot so a subsequent start() doesn't see a zombie.
            self.reap_child();
        }
        *self.last.lock() = state;
        state
    }

    /// Cached snapshot for the `daemon_status` command (non-blocking). The
    /// poller refreshes `last` every [`DAEMON_POLL_INTERVAL`].
    fn snapshot(&self) -> DaemonStatusDto {
        let state = *self.last.lock();
        let pid = self.pid();
        DaemonStatusDto { state, pid }
    }
}

/// Resolve the daemon URL from `OCEAN_DAEMON_URL` (matching the wasm surface)
/// or the loopback default.
fn daemon_url_from_env() -> String {
    std::env::var("OCEAN_DAEMON_URL").unwrap_or_else(|_| DEFAULT_DAEMON_URL.to_string())
}

/// Binary resolution order (testable via [`resolve_daemon_bin_with`]):
/// `OCEAN_DAEMON_BIN` env → `ocean-daemon` on PATH.
///
/// TASK-78: there is deliberately NO caller-supplied override. `daemon_start`
/// and `daemon_restart` used to accept an optional binary-path argument
/// straight from IPC, which reached `ProcessCommand::spawn` after only a trim
/// and a non-empty check — an arbitrary-executable primitive callable by
/// anything running in the webview. Tauri 2's capability ACL does NOT cover commands
/// registered through `generate_handler!` (it gates plugin/`core:` commands),
/// so the crate's otherwise-minimal capability set gave no protection there.
/// The env var is a trusted operator-set channel; an IPC argument is not.
fn resolve_daemon_bin() -> String {
    resolve_daemon_bin_with(std::env::var("OCEAN_DAEMON_BIN").ok().as_deref())
}

/// Pure core of [`resolve_daemon_bin`] — takes the env value as a parameter
/// so the resolution order is unit-testable without mutating process env.
fn resolve_daemon_bin_with(env_bin: Option<&str>) -> String {
    if let Some(bin) = env_bin.map(str::trim).filter(|s| !s.is_empty()) {
        return bin.to_string();
    }
    "ocean-daemon".to_string()
}

/// Parse a `http://host:port[/…]` URL into `(host, port)`. IPv6 brackets are
/// stripped so the host feeds `ToSocketAddrs`; a missing port defaults to
/// 4780. Returns `None` only for an empty host.
fn parse_host_port(url: &str) -> Option<(String, u16)> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let (host, port) = match authority.rsplit_once(':') {
        // Guard against a bare `host:` (empty port) and `:port` (empty host).
        Some((h, p)) if !h.is_empty() => (h.to_string(), p.parse::<u16>().ok()),
        _ => (authority.to_string(), None),
    };
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return None;
    }
    Some((host.to_string(), port.unwrap_or(4780)))
}

/// Log file for the supervised daemon's stdout/stderr. Prefers the XDG-style
/// `~/.local/state/ocean/ocean-daemon.log`; falls back to the OS temp dir so
/// a missing `$HOME` or an unwritable state dir never blocks the spawn.
fn daemon_log_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            let dir = PathBuf::from(home).join(".local/state/ocean");
            if std::fs::create_dir_all(&dir).is_ok() {
                return dir.join("ocean-daemon.log");
            }
        }
    }
    std::env::temp_dir().join("ocean-daemon.log")
}

/// Spawn the daemon binary detached: stdin null, stdout+stderr appended to
/// the log file. The returned `Child` is NOT `kill_on_drop`, so it outlives
/// the shell if the shell crashes — supervision stays explicit.
fn spawn_daemon(bin: &str) -> Result<Child, String> {
    let log_path = daemon_log_path();
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("open daemon log {}: {e}", log_path.display()))?;
    let stderr = log
        .try_clone()
        .map_err(|e| format!("dup daemon log: {e}"))?;
    ProcessCommand::new(bin)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|e| format!("spawn {bin}: {e}"))
}

/// Blocking TCP connect probe — `true` when something is accepting on
/// `host:port`. `ToSocketAddrs` resolves a hostname, but the loopback IP the
/// daemon binds resolves instantly.
fn tcp_reachable(host: &str, port: u16) -> bool {
    use std::net::ToSocketAddrs;
    let Ok(mut addrs) = (host, port).to_socket_addrs() else {
        return false;
    };
    let Some(addr) = addrs.next() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, DAEMON_PROBE_TIMEOUT).is_ok()
}

/// Background liveness poller: probe every [`DAEMON_POLL_INTERVAL`] and emit a
/// `daemon-status` event to the webview ON CHANGE ONLY. Probes before
/// sleeping, so the first event fires within one timeout of boot.
fn spawn_daemon_poller(app: AppHandle, daemon: Arc<DaemonSup>) {
    thread::spawn(move || loop {
        let prev = daemon.snapshot().state;
        let state = daemon.probe();
        if state != prev {
            let _ = app.emit(
                "daemon-status",
                DaemonStatusDto {
                    state,
                    pid: daemon.pid(),
                },
            );
        }
        thread::sleep(DAEMON_POLL_INTERVAL);
    });
}

/// Current daemon supervision state (cached from the poller). The wasm side
/// reads this on mount to seed its indicator before the first on-change event.
#[tauri::command]
async fn daemon_status(state: State<'_, AppState>) -> Result<DaemonStatusDto, String> {
    Ok(state.daemon.snapshot())
}

/// Start the supervised ocean-daemon. `bin` overrides `OCEAN_DAEMON_BIN` →
/// PATH. A graceful no-op when the shell already owns a live child or the
/// port is already served. Emits a `daemon-status` event immediately so the
/// UI flips without waiting for the next poll.
#[tauri::command]
async fn daemon_start(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<DaemonStatusDto, String> {
    // TASK-78: no caller-supplied binary path — see `resolve_daemon_bin`.
    state.daemon.start(&resolve_daemon_bin())?;
    let snap = state.daemon.snapshot();
    let _ = app.emit("daemon-status", snap.clone());
    Ok(snap)
}

/// Stop the daemon the shell owns (never an external one). Emits a final
/// `daemon-status` event.
#[tauri::command]
async fn daemon_stop(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<DaemonStatusDto, String> {
    state.daemon.stop();
    let snap = state.daemon.snapshot();
    let _ = app.emit("daemon-status", snap.clone());
    Ok(snap)
}

/// Restart the shell-owned daemon (stop + start). Emits a `daemon-status`
/// event.
#[tauri::command]
async fn daemon_restart(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<DaemonStatusDto, String> {
    // TASK-78: no caller-supplied binary path — see `resolve_daemon_bin`.
    state.daemon.restart(&resolve_daemon_bin())?;
    let snap = state.daemon.snapshot();
    let _ = app.emit("daemon-status", snap.clone());
    Ok(snap)
}

// ── helpers ───────────────────────────────────────────────────────────────

fn repo_head_name(repo: &Repository) -> String {
    match repo.head() {
        Ok(head) => head.shorthand().unwrap_or("HEAD").to_string(),
        Err(_) => "HEAD".to_string(),
    }
}

fn repo_ahead_behind(repo: &Repository) -> (usize, usize) {
    let head = match repo.head() {
        Ok(h) if h.is_branch() => h,
        _ => return (0, 0),
    };
    let branch_name = match head.shorthand() {
        Some(n) => n,
        None => return (0, 0),
    };
    let branch = match repo.find_branch(branch_name, git2::BranchType::Local) {
        Ok(b) => b,
        Err(_) => return (0, 0),
    };
    let upstream = match branch.upstream() {
        Ok(up) => up,
        Err(_) => return (0, 0),
    };

    let local_oid = match branch.get().target() {
        Some(oid) => oid,
        None => return (0, 0),
    };
    let upstream_oid = match upstream.get().target() {
        Some(oid) => oid,
        None => return (0, 0),
    };

    match repo.graph_ahead_behind(local_oid, upstream_oid) {
        Ok((a, b)) => (a, b),
        Err(_) => (0, 0),
    }
}

fn repo_status_counts(repo: &Repository) -> (usize, usize) {
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true);

    let statuses = match repo.statuses(Some(&mut opts)) {
        Ok(s) => s,
        Err(_) => return (0, 0),
    };

    let index_mask = git2::Status::INDEX_NEW
        | git2::Status::INDEX_MODIFIED
        | git2::Status::INDEX_DELETED
        | git2::Status::INDEX_RENAMED
        | git2::Status::INDEX_TYPECHANGE;

    let wt_mask = git2::Status::WT_NEW
        | git2::Status::WT_MODIFIED
        | git2::Status::WT_DELETED
        | git2::Status::WT_RENAMED
        | git2::Status::WT_TYPECHANGE;

    let mut dirty = 0usize;
    let mut staged = 0usize;

    for entry in statuses.iter() {
        let s = entry.status();
        if s.intersects(index_mask) {
            staged += 1;
        }
        if s.intersects(wt_mask) {
            dirty += 1;
        }
    }

    (dirty, staged)
}

fn repo_recent_commits(repo: &Repository, n: usize) -> Vec<CommitInfoDto> {
    let head_commit = match repo
        .head()
        .ok()
        .and_then(|h| h.target())
        .and_then(|oid| repo.find_commit(oid).ok())
    {
        Some(c) => c,
        None => return vec![],
    };

    let mut walk = match repo.revwalk() {
        Ok(w) => w,
        Err(_) => return vec![],
    };
    walk.set_sorting(git2::Sort::TIME).ok();
    if walk.push(head_commit.id()).is_err() {
        return vec![];
    }

    let mut commits = Vec::with_capacity(n);
    for oid in walk.flatten() {
        let commit = match repo.find_commit(oid) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let oid_str = oid.to_string();
        let id_short = oid_str[..7.min(oid_str.len())].to_string();
        let summary = commit.summary().unwrap_or("").to_string();
        let author = commit.author().name().unwrap_or("").to_string();
        let when_epoch = commit.time().seconds();

        commits.push(CommitInfoDto {
            id_short,
            summary,
            author,
            when_epoch,
        });

        if commits.len() >= n {
            break;
        }
    }

    commits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dto_json_shape_matches_wasm_contract() {
        let state = RepoStateDto {
            branch: "main".into(),
            ahead: 2,
            behind: 1,
            dirty: 3,
            staged: 1,
            commits: vec![CommitInfoDto {
                id_short: "abc1234".into(),
                summary: "fix: oops".into(),
                author: "You".into(),
                when_epoch: 1751932800,
            }],
        };

        let json = serde_json::to_value(&state).unwrap();

        // Top-level fields are camelCase (serde default).
        assert_eq!(json["branch"].as_str(), Some("main"));
        assert_eq!(json["ahead"].as_u64(), Some(2));
        assert_eq!(json["behind"].as_u64(), Some(1));
        assert_eq!(json["dirty"].as_u64(), Some(3));
        assert_eq!(json["staged"].as_u64(), Some(1));

        let c = &json["commits"][0];
        assert_eq!(c["id_short"].as_str(), Some("abc1234"));
        assert_eq!(c["summary"].as_str(), Some("fix: oops"));
        assert_eq!(c["author"].as_str(), Some("You"));
        // `when_epoch` is the only field with an explicit serde rename.
        assert_eq!(c["when_epoch"].as_i64(), Some(1751932800));
        // `whenEpoch` — the camelCase form of `when_epoch` — MUST NOT appear.
        assert!(c.get("whenEpoch").is_none());
    }

    #[test]
    fn repo_state_on_temp_init_returns_branch_and_zero_counts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = Repository::init(dir.path()).expect("init");

        // At least one commit so there's a HEAD ref to read.
        let sig = git2::Signature::now("Test", "test@ocean.dev").unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();

        let result = repo_state(dir.path().to_string_lossy().into_owned())
            .expect("should not err")
            .expect("should find repo");

        // git2::Repository::init defaults to "master"; what matters is
        // that we got a non-empty branch, zero counts, and one commit.
        assert!(!result.branch.is_empty(), "branch should not be empty");
        assert_eq!(result.dirty, 0);
        assert_eq!(result.staged, 0);
        assert_eq!(result.commits.len(), 1);
    }

    #[test]
    fn repo_state_outside_repo_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = repo_state(dir.path().to_string_lossy().into_owned()).expect("should not err");
        assert!(result.is_none());
    }
    // ── daemon supervision: pure logic ───────────────────────────────────

    #[test]
    fn parse_host_port_loopback_default() {
        let (h, p) = parse_host_port("http://127.0.0.1:4780").unwrap();
        assert_eq!(h, "127.0.0.1");
        assert_eq!(p, 4780);
    }

    #[test]
    fn parse_host_port_strips_path_and_scheme() {
        let (h, p) = parse_host_port("http://localhost:4780/v1/agent/models").unwrap();
        assert_eq!(h, "localhost");
        assert_eq!(p, 4780);
    }

    #[test]
    fn parse_host_port_defaults_missing_port_to_4780() {
        let (h, p) = parse_host_port("http://127.0.0.1").unwrap();
        assert_eq!(h, "127.0.0.1");
        assert_eq!(p, 4780);
    }

    #[test]
    fn parse_host_port_strips_ipv6_brackets() {
        let (h, p) = parse_host_port("http://[::1]:4780").unwrap();
        assert_eq!(h, "::1");
        assert_eq!(p, 4780);
    }

    #[test]
    fn parse_host_port_https_and_custom_port() {
        let (h, p) = parse_host_port("https://ocean.dev:9000/").unwrap();
        assert_eq!(h, "ocean.dev");
        assert_eq!(p, 9000);
    }

    #[test]
    fn parse_host_port_rejects_empty_host() {
        assert!(parse_host_port("").is_none());
        assert!(parse_host_port("http://").is_none());
    }

    /// TASK-78: the resolver takes ONLY the env value. There is no
    /// caller-supplied override, because that argument was reachable from the
    /// webview and ended in `ProcessCommand::spawn`. If someone re-adds an
    /// `explicit` parameter here, this test stops compiling — which is the
    /// point: the signature IS the security boundary.
    /// TASK-85: `open_file`'s root containment is self-satisfiable (the
    /// caller supplies both root and path), so the load-bearing gate is
    /// refusing targets macOS would EXECUTE. These drive the real predicate
    /// against real files on disk.
    #[test]
    fn open_file_refuses_targets_that_would_execute() {
        let dir = tempfile::tempdir().expect("tempdir");

        // Extension-based: macOS `open(1)` runs these regardless of mode.
        for name in ["payload.command", "x.terminal", "a.workflow", "s.scpt"] {
            let p = dir.path().join(name);
            std::fs::write(&p, b"#!/bin/sh\necho pwned\n").unwrap();
            assert!(
                opens_as_executable(&p),
                "{name} must be refused — open(1) executes it",
            );
        }
        // Case-insensitive: .COMMAND is the same file type.
        let upper = dir.path().join("payload.COMMAND");
        std::fs::write(&upper, b"x").unwrap();
        assert!(opens_as_executable(&upper));

        // Mode-based: no dangerous extension, but the exec bit is set — the
        // exact shape a tool-writing daemon would produce.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let p = dir.path().join("innocuous.txt");
            std::fs::write(&p, b"#!/bin/sh\necho pwned\n").unwrap();
            let mut perms = std::fs::metadata(&p).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&p, perms).unwrap();
            assert!(
                opens_as_executable(&p),
                "an executable-bit file must be refused even with a safe extension",
            );
        }

        // And the feature still works: ordinary documents open. A guard that
        // blocks the thing it protects is not a fix.
        for name in ["notes.md", "data.json", "photo.png", "report.pdf"] {
            let p = dir.path().join(name);
            std::fs::write(&p, b"content").unwrap();
            assert!(
                !opens_as_executable(&p),
                "{name} is a document and must still open",
            );
        }
        // No extension, not executable — still a document.
        let plain = dir.path().join("LICENSE");
        std::fs::write(&plain, b"text").unwrap();
        assert!(!opens_as_executable(&plain));
    }

    #[test]
    fn resolve_daemon_bin_env_wins_over_path_default() {
        assert_eq!(resolve_daemon_bin_with(Some("/env/od")), "/env/od");
        // Whitespace-only env falls through to the PATH default.
        assert_eq!(resolve_daemon_bin_with(Some("  ")), "ocean-daemon");
    }

    #[test]
    fn resolve_daemon_bin_path_default_when_nothing_set() {
        assert_eq!(resolve_daemon_bin_with(None), "ocean-daemon");
    }

    /// The IPC commands must expose no path parameter at all. Source-pinned
    /// because the commands need a live Tauri runtime to invoke.
    #[test]
    fn daemon_commands_take_no_caller_supplied_binary() {
        let src = include_str!("lib.rs");
        let needle = format!("bin: Option<{}>", "String");
        assert!(
            !src.contains(needle.as_str()),
            "daemon_start/daemon_restart must not accept a binary path from IPC \
             (TASK-78): Tauri 2 capabilities do not gate generate_handler! commands, \
             so any script in the webview could spawn an arbitrary executable",
        );
    }

    #[test]
    fn decide_state_running_when_port_reachable() {
        // Reachable is Running regardless of child ownership (pid distinguishes
        // our-child vs external-daemon at the DTO layer, not here).
        assert_eq!(decide_daemon_state(true, true, false), DaemonState::Running);
        assert_eq!(
            decide_daemon_state(true, false, false),
            DaemonState::Running
        );
        assert_eq!(decide_daemon_state(true, true, true), DaemonState::Running);
    }

    #[test]
    fn decide_state_starting_when_child_alive_but_port_not_up() {
        assert_eq!(
            decide_daemon_state(false, true, false),
            DaemonState::Starting
        );
        assert_eq!(
            decide_daemon_state(false, true, true),
            DaemonState::Starting
        );
    }

    #[test]
    fn decide_state_stopped_vs_unreachable_uses_reachable_ever() {
        // Never reached + nothing serving + no child → Stopped.
        assert_eq!(
            decide_daemon_state(false, false, false),
            DaemonState::Stopped
        );
        // Previously reached + now gone + no child → Unreachable (external daemon stopped).
        assert_eq!(
            decide_daemon_state(false, false, true),
            DaemonState::Unreachable
        );
    }

    #[test]
    fn daemon_status_dto_serializes_state_lowercase_and_skips_absent_pid() {
        let dto = DaemonStatusDto {
            state: DaemonState::Running,
            pid: None,
        };
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["state"].as_str(), Some("running"));
        // `pid` is skipped when None (the wasm side treats it as optional).
        assert!(json.get("pid").is_none());
    }

    #[test]
    fn daemon_status_dto_includes_pid_when_owned() {
        let dto = DaemonStatusDto {
            state: DaemonState::Starting,
            pid: Some(4242),
        };
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["state"].as_str(), Some("starting"));
        assert_eq!(json["pid"].as_u64(), Some(4242));
    }
}

// ── tray + global hotkey (menubar-app presence) ───────────────────────────

/// Bring the main window to the front: unminimize, reveal, and focus it.
/// Used by the tray "Show Ocean" item and the Cmd+Shift+Space summon hotkey.
fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Toggle-summon: if the main window is focused, hide it to the tray;
/// otherwise summon it to the front.
fn toggle_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_focused().unwrap_or(false) {
            let _ = window.hide();
        } else {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}

/// Tray-menu daemon action selector for [`tray_daemon_action`].
enum DaemonTrayAction {
    Start,
    Restart,
}

/// Run a daemon supervision action from a tray-menu click on the async
/// runtime, then push a `daemon-status` event with the resulting snapshot.
/// A graceful no-op when the daemon is already in the target state — the
/// underlying `DaemonSup` methods never double-spawn.
fn tray_daemon_action(app: &AppHandle, action: DaemonTrayAction) {
    let daemon = app.state::<AppState>().daemon.clone();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let bin = resolve_daemon_bin();
        let _ = match action {
            DaemonTrayAction::Start => daemon.start(&bin),
            DaemonTrayAction::Restart => daemon.restart(&bin),
        };
        let snap = daemon.snapshot();
        let _ = app.emit("daemon-status", snap);
    });
}

/// Only schemes intentionally emitted by Ocean's Markdown renderer may leave
/// the app. Keep this validation native as a second boundary behind the WASM
/// renderer's own URL allowlist.
fn allowed_external_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed != url || trimmed.chars().any(char::is_control) {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:")
        || lower.starts_with("tel:")
}

/// Open a rendered transcript/component link in the OS default application.
/// The shared UI calls this only from an explicit primary-button click.
#[tauri::command]
fn open_external_url(url: String) -> Result<(), String> {
    if !allowed_external_url(&url) {
        return Err("unsupported external URL".into());
    }
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|error| error.to_string())
}

#[cfg(test)]
mod external_url_tests {
    use super::allowed_external_url;

    #[test]
    fn accepts_external_schemes_rendered_by_markdown() {
        assert!(allowed_external_url("https://www.tiktok.com/@creator"));
        assert!(allowed_external_url("http://localhost:8790"));
        assert!(allowed_external_url("mailto:creator@example.com"));
        assert!(allowed_external_url("tel:+15551234567"));
    }

    #[test]
    fn rejects_relative_dangerous_and_obfuscated_urls() {
        assert!(!allowed_external_url("/internal"));
        assert!(!allowed_external_url("javascript:alert(1)"));
        assert!(!allowed_external_url("data:text/html,boom"));
        assert!(!allowed_external_url(" https://example.com"));
        assert!(!allowed_external_url("https://example.com\n"));
    }
}

// ── privileged room-authority transport (spec 1.12) ──────────────────────
//
// The browser PWA can run the Room-agent authorization ceremony because the
// proxy injects its mode-0600 operator key server-side on six exact daemon
// routes; the key never enters the shared WASM bundle. The Tauri shell was
// read-only for the same reason — it had no equivalent privileged transport.
// It runs beside the daemon and already supervises it, so it can own one:
// this command is that transport, and it is deliberately the same shape as
// the proxy's forwarder.
//
// What this command does NOT do: take a URL, take a header, or return the
// key. The webview names a METHOD and a PATH; the origin comes from
// `OCEAN_DAEMON_URL` (operator-set, exactly as the daemon probe does), the
// credential comes from disk under a custody check, and the reply carries
// only the daemon's status and body. Tauri 2 capabilities do NOT gate
// `generate_handler!` commands (see TASK-78 above), so the ALLOWLIST here —
// not the ACL — is the security boundary.

/// Where the room-operator credential lives, mirroring the proxy's resolution
/// (`OCEAN_OPERATOR_KEY_FILE` → `<config dir>/operator.key`) so one machine's
/// two surfaces read one file.
fn room_operator_key_path() -> PathBuf {
    if let Some(path) = std::env::var_os("OCEAN_OPERATOR_KEY_FILE") {
        return PathBuf::from(path);
    }
    ocean_config_dir().join("operator.key")
}

/// The `ocean-rs` configuration directory, resolved exactly as
/// `ocean-surface-proxy` resolves it.
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

/// Read the room-operator credential without weakening its custody contract.
/// Possession of this key permits durable local execution-authority
/// mutations, so the file must be owner-owned, single-linked, mode 0600, a
/// regular file, and opened without following symlinks — the same five
/// conditions `ocean-surface-proxy::read_room_operator_key` enforces. The
/// value is returned only to the forwarder below; no error message and no
/// command reply ever carries it.
#[cfg(unix)]
fn read_room_operator_key(path: &Path) -> Result<String, String> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let link = std::fs::symlink_metadata(path)
        .map_err(|error| format!("room operator credential unavailable: {error}"))?;
    if link.file_type().is_symlink() || !link.is_file() {
        return Err("room operator credential must be a regular file".to_owned());
    }
    let file = OpenOptions::new()
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
    {
        use std::io::Read as _;
        let mut file = file;
        file.read_to_string(&mut key)
            .map_err(|error| format!("room operator credential unavailable: {error}"))?;
    }
    let key = key.trim();
    if key.is_empty() {
        return Err("room operator credential is empty".to_owned());
    }
    if key.bytes().any(|b| !(0x20..0x7f).contains(&b)) {
        return Err("room operator credential is not a valid header value".to_owned());
    }
    Ok(key.to_owned())
}

// The custody contract above is expressed in POSIX ownership, link count,
// mode and `O_NOFOLLOW`. A non-unix host cannot satisfy it.
//
// The first draft of this shipped a stub returning `Err` on such a host, and
// that was the wrong shape: `host::room_authority_mutations_for_host` reports
// Tauri as authority-capable unconditionally, so a Windows build would have
// rendered the full ceremony — Authorize, suspend, resume, revoke — with
// every one of them guaranteed to fail on a credential that could not be
// read. The platform contract's rule is "absence, not errors"
// (AGENTS.md 30-32), and a control that is present and always fails is the
// exact opposite.
//
// Refusing to build is the honest version. This shell is macOS-first and has
// never been built for Windows, so nothing regresses today; what changes is
// that the requirement is explicit instead of latent. A Windows shell needs
// two things, and the compile error names both: a real custody equivalent
// (an ACL check — POSIX mode bits have no meaning there), and a capability
// handshake so the surface can render the ceremony ABSENT rather than
// writable. Linux is unix and is unaffected.
#[cfg(not(unix))]
compile_error!(
    "ocean-tauri's Room-agent authority forwarder has no custody implementation \
     for this target. Its contract is POSIX (owner, link count, mode 0600, \
     O_NOFOLLOW). Implementing it here needs an ACL-based equivalent AND a \
     capability handshake so `room_authority_mutations_supported()` reports \
     absence rather than a ceremony whose every action fails."
);

/// True only for the exact six daemon routes the operator credential may be
/// attached to. A byte-for-byte mirror of the proxy's
/// `room_agent_authority_mutation`: bootstrap, authorize, reauthorize,
/// suspend, resume, revoke. Read-only binding inspection and package preview
/// stay credential-free and are not reachable through this command at all.
///
/// `path` arrives percent-encoded from the surface (`rooms::encode` escapes
/// `/`), so a room key or member id can never split into extra segments here.
fn room_agent_authority_mutation(method: &str, path: &str) -> bool {
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
    match method {
        "POST" if segments.len() == 5 => true,
        "POST" if segments.len() == 6 && segments[5] == "bootstrap" => true,
        "POST" if segments.len() == 7 => {
            !segments[5].is_empty() && matches!(segments[6], "reauthorize" | "suspend" | "resume")
        }
        "DELETE" if segments.len() == 6 => !segments[5].is_empty(),
        _ => false,
    }
}

/// Shape of a request path this command will forward: absolute, single-line,
/// no query or fragment, no dot segment. The allowlist above decides WHICH
/// route; this decides that the string is a route at all, before it is
/// concatenated onto the daemon origin.
fn forwardable_operator_path(path: &str) -> bool {
    if !path.starts_with('/') || path.len() > 1024 {
        return false;
    }
    if path
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '?' | '#' | '\\'))
    {
        return false;
    }
    // Dot segments are tested on the DECODED segment, not the literal one.
    // `%2e%2e` is not `..` to a string comparison, but it IS to the URL
    // parser: it normalises the escape and collapses the segment, so
    // `DELETE /v1/rooms/persistent/team/agents/%2e%2e` would pass a literal
    // check, pass the six-route allowlist as a non-empty member id, and then
    // carry the operator credential to `DELETE /v1/rooms/persistent/team/` —
    // a destructive route on no allowlist. The proxy never had this hole
    // because `has_dot_segment` runs there on axum's already-decoded capture
    // (main.rs 2264); this mirror had copied the allowlist and not that guard.
    !path.split('/').any(is_dot_segment)
}

/// True when a path segment IS a dot segment once percent-decoding is
/// accounted for. Any run of dots counts, not just `.` and `..`: the URL
/// parser only acts on those two, but a segment that is nothing but dots has
/// no legitimate meaning here and refusing the whole shape leaves no edge to
/// re-derive.
fn is_dot_segment(segment: &str) -> bool {
    let decoded = percent_decode_segment(segment);
    !decoded.is_empty() && decoded.bytes().all(|b| b == b'.')
}

/// Percent-decode one path segment, byte-wise. Enough to answer "what does
/// the URL parser see here"; an invalid or truncated escape is left literal,
/// which is safe because the only question asked of the result is whether it
/// is a run of dots. Not a general-purpose decoder and not used to build the
/// request — the path is forwarded exactly as given.
fn percent_decode_segment(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((((hi << 4) | lo) as u8) as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// The daemon's answer, verbatim: status plus body text. The surface decodes
/// the body itself, exactly as it decodes the browser proxy's reply, so one
/// ceremony reads one wire shape on both hosts.
#[derive(Clone, Serialize)]
struct OperatorReplyDto {
    status: u16,
    body: String,
}

/// Forward one Room-agent authority mutation to the supervised daemon with the
/// operator credential attached. Errors are human-readable and credential-free.
#[tauri::command]
async fn daemon_operator_request(
    method: String,
    path: String,
    body: Option<String>,
) -> Result<OperatorReplyDto, String> {
    if method != "POST" && method != "DELETE" {
        return Err("unsupported operator request method".to_owned());
    }
    if !forwardable_operator_path(&path) {
        return Err("operator request path is not a forwardable daemon route".to_owned());
    }
    if !room_agent_authority_mutation(&method, &path) {
        return Err("not a room authority mutation route".to_owned());
    }
    let url = format!("{}{path}", daemon_url_from_env().trim_end_matches('/'));
    // Belt and braces, and the half that matters more than the pre-check:
    // parse the URL and require that it STILL addresses the path the
    // allowlist just approved. Every check above reasons about a string; this
    // one reasons about what will actually be sent. `%2e%2e` was one way a
    // parser could rewrite a path out from under an allowlist decision, and
    // the class is bigger than that instance — normalisation rules change,
    // and the next one arrives as a dependency bump rather than as a diff
    // here. Refusing on any disagreement makes the whole class inert.
    //
    // Ordering is deliberate: this runs BEFORE the credential is read, so a
    // path the parser would rewrite never reaches the key at all.
    let parsed =
        reqwest::Url::parse(&url).map_err(|error| format!("operator request URL: {error}"))?;
    if parsed.path() != path {
        return Err("operator request path would not survive URL parsing".to_owned());
    }
    let key = read_room_operator_key(&room_operator_key_path())?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("operator transport unavailable: {error}"))?;
    let request = if method == "DELETE" {
        client.delete(&url)
    } else {
        client.post(&url)
    }
    .header("content-type", "application/json")
    .header("X-Ocean-Operator", key)
    .body(body.unwrap_or_default());
    let response = request
        .send()
        .await
        .map_err(|error| format!("daemon is unreachable: {error}"))?;
    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .map_err(|error| format!("daemon reply could not be read: {error}"))?;
    Ok(OperatorReplyDto { status, body })
}

#[cfg(test)]
mod operator_transport_tests {
    use super::{
        forwardable_operator_path, is_dot_segment, read_room_operator_key,
        room_agent_authority_mutation, room_operator_key_path,
    };

    const ROOM: &str = "/v1/rooms/persistent/team-blue/agents";

    #[test]
    fn the_allowlist_admits_exactly_the_six_ceremony_routes() {
        // The six the proxy injects on, and nothing else.
        assert!(room_agent_authority_mutation("POST", ROOM));
        assert!(room_agent_authority_mutation(
            "POST",
            &format!("{ROOM}/bootstrap")
        ));
        for action in ["reauthorize", "suspend", "resume"] {
            assert!(room_agent_authority_mutation(
                "POST",
                &format!("{ROOM}/agent-1/{action}")
            ));
        }
        assert!(room_agent_authority_mutation(
            "DELETE",
            &format!("{ROOM}/agent-1")
        ));
    }

    #[test]
    fn the_allowlist_refuses_every_other_route_and_method() {
        // Inspection and preview are credential-free and must stay that way.
        assert!(!room_agent_authority_mutation("GET", ROOM));
        assert!(!room_agent_authority_mutation(
            "POST",
            &format!("{ROOM}/preview/reviewer")
        ));
        // Neighbouring room routes the ceremony also calls, but unprivileged.
        assert!(!room_agent_authority_mutation(
            "POST",
            "/v1/rooms/persistent/team-blue/members/agents"
        ));
        assert!(!room_agent_authority_mutation(
            "POST",
            "/v1/rooms/persistent/team-blue/messages"
        ));
        // Wrong verb on an allowlisted shape.
        assert!(!room_agent_authority_mutation(
            "DELETE",
            &format!("{ROOM}/agent-1/suspend")
        ));
        assert!(!room_agent_authority_mutation("DELETE", ROOM));
        // Unknown action verb, empty member, wrong prefix, too short.
        assert!(!room_agent_authority_mutation(
            "POST",
            &format!("{ROOM}/agent-1/promote")
        ));
        assert!(!room_agent_authority_mutation(
            "POST",
            &format!("{ROOM}//suspend")
        ));
        assert!(!room_agent_authority_mutation("POST", "/v1/agents"));
        assert!(!room_agent_authority_mutation(
            "POST",
            "/v1/rooms/persistent//agents"
        ));
        assert!(!room_agent_authority_mutation("POST", "/"));
    }

    #[test]
    fn a_path_must_be_an_absolute_dot_free_single_line_route() {
        assert!(forwardable_operator_path(ROOM));
        assert!(!forwardable_operator_path("v1/rooms/persistent/x/agents"));
        assert!(!forwardable_operator_path(
            "/v1/rooms/persistent/x/../../agents"
        ));
        assert!(!forwardable_operator_path("/v1/./agents"));
        assert!(!forwardable_operator_path("/v1/agents?x=1"));
        assert!(!forwardable_operator_path("/v1/agents#frag"));
        assert!(!forwardable_operator_path("/v1/agents\nHost: evil"));
        assert!(!forwardable_operator_path("/v1/ agents"));
        assert!(!forwardable_operator_path(&format!(
            "/{}",
            "a".repeat(1024)
        )));
    }

    /// A path cannot name the origin: no scheme, no authority, ever. This is
    /// what keeps the credential pointed at the supervised daemon.
    /// P1 from review: `%2e%2e` is not `..` to a string comparison but IS to
    /// the URL parser, which normalises the escape and collapses the segment.
    /// Measured against the real parser: `/v1/rooms/persistent/team/agents/%2e%2e`
    /// resolves to `/v1/rooms/persistent/team/`, so a literal-only check would
    /// have carried the operator credential to a destructive route on no
    /// allowlist.
    #[test]
    fn percent_encoded_dot_segments_are_refused() {
        for hostile in [
            "/v1/rooms/persistent/team/agents/%2e%2e",
            "/v1/rooms/persistent/team/agents/%2E%2E",
            "/v1/rooms/persistent/team/agents/%2e",
            "/v1/rooms/persistent/team/agents/%2E",
            "/v1/rooms/persistent/%2e%2e/agents",
            "/v1/rooms/persistent/team/agents/%2e%2e%2e",
            // Mixed literal and encoded, and a nested escape of the percent.
            "/v1/rooms/persistent/team/agents/.%2e",
            "/v1/rooms/persistent/team/agents/%2e.",
        ] {
            assert!(
                !forwardable_operator_path(hostile),
                "{hostile} must be refused: the URL parser reads it as a dot segment",
            );
        }
        // And the feature still works. A guard that blocks the thing it
        // protects is not a fix: an encoded room key or member id is ordinary
        // here, because `rooms::encode` produces exactly these.
        for ok in [
            "/v1/rooms/persistent/team%2Fblue/agents/agent-1",
            "/v1/rooms/persistent/team.blue/agents/agent-1",
            "/v1/rooms/persistent/team/agents/a%2e%2eb",
            "/v1/rooms/persistent/team/agents/review%20agent",
        ] {
            assert!(
                forwardable_operator_path(ok),
                "{ok} is a legitimate encoded route and must forward",
            );
        }
    }

    /// The decoder answers one question — "is this a run of dots" — and a
    /// truncated or invalid escape must not make it answer wrongly.
    #[test]
    fn the_segment_decoder_survives_malformed_escapes() {
        assert!(is_dot_segment("%2e"));
        assert!(is_dot_segment("."));
        assert!(!is_dot_segment(""));
        assert!(!is_dot_segment("%2"));
        assert!(!is_dot_segment("%"));
        assert!(!is_dot_segment("%zz"));
        assert!(!is_dot_segment("agent-1"));
        assert!(!is_dot_segment("a."));
    }

    #[test]
    fn a_path_can_never_carry_an_origin() {
        for hostile in [
            "http://evil.example/v1/rooms/persistent/x/agents",
            "//evil.example/v1/rooms/persistent/x/agents",
        ] {
            assert!(
                !room_agent_authority_mutation("POST", hostile),
                "{hostile} must not be an authority mutation",
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_credential_reader_enforces_custody_and_never_follows_links() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("tempdir");
        let key = dir.path().join("operator.key");
        std::fs::write(&key, "secret-value\n").unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            read_room_operator_key(&key).expect("secure key"),
            "secret-value"
        );

        // Group/world-readable is refused.
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_room_operator_key(&key).is_err());
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();

        // A second hard link means a second custodian.
        let hard = dir.path().join("hardlink.key");
        std::fs::hard_link(&key, &hard).unwrap();
        assert!(read_room_operator_key(&key).is_err());
        std::fs::remove_file(&hard).unwrap();

        // A symlink is refused before it is opened.
        let symlink = dir.path().join("symlink.key");
        std::os::unix::fs::symlink(&key, &symlink).unwrap();
        assert!(read_room_operator_key(&symlink).is_err());

        // Empty is refused: an empty header is not a credential.
        let empty = dir.path().join("empty.key");
        std::fs::write(&empty, "   \n").unwrap();
        std::fs::set_permissions(&empty, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(read_room_operator_key(&empty).is_err());

        // A missing file is an error, never a credential-free forward.
        assert!(read_room_operator_key(&dir.path().join("absent.key")).is_err());
    }

    #[test]
    fn the_key_path_follows_the_proxys_resolution() {
        // Both surfaces read one file on one machine; the env override is the
        // seam an operator uses to move it.
        let path = room_operator_key_path();
        assert!(
            path.ends_with("operator.key"),
            "resolved key path {} must name operator.key",
            path.display(),
        );
    }
}

pub fn run() {
    // Daemon probe target: parse once from `OCEAN_DAEMON_URL` so the shell
    // and the wasm bundle share one configurable origin (default 127.0.0.1:4780).
    let url = daemon_url_from_env();
    let (host, port) = parse_host_port(&url).unwrap_or_else(|| ("127.0.0.1".to_string(), 4780));
    tauri::Builder::default()
        // Single-instance guard (QA-005). Close-requested hides the main
        // window to the tray instead of quitting, so an "invisible" Ocean is
        // often still alive when the user launches again (./run-tauri.sh, or
        // opening the packaged .app). Without this guard each launch is a new
        // process with its own competing "Ocean Desktop" window. With it, the
        // second process notifies the first — which unhides/focuses its
        // window — and exits before creating any window. Registered FIRST so
        // the duplicate process bails before any other plugin or window init.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        toggle_main_window(app);
                    }
                })
                .build(),
        )
        .manage(AppState {
            watchers: Default::default(),
            daemon: Arc::new(DaemonSup::new(host, port)),
            menu: Mutex::new(MenuBridge {
                ready: false,
                pending: Vec::new(),
            }),
        })
        .setup(|app| {
            // Background daemon-supervision liveness poller: probes the daemon
            // port every ~5s and emits `daemon-status` to the webview ON CHANGE
            // ONLY. Spawned first so the first event fires within one probe of
            // boot, before the tray/menu wiring below.
            let daemon_sup = app.state::<AppState>().daemon.clone();
            spawn_daemon_poller(app.handle().clone(), daemon_sup);

            // Opt-in UI diagnostics for native acceptance runs. The script is
            // loaded only when the operator supplies an explicit path; normal
            // builds and launches do not evaluate or expose test code.
            if let Ok(script_path) = std::env::var("OCEAN_UI_DEBUG_SCRIPT") {
                match (
                    std::fs::read_to_string(&script_path),
                    app.get_webview_window("main"),
                ) {
                    (Ok(script), Some(webview)) => {
                        // Devtools ship only in debug builds: the `devtools`
                        // Cargo feature is off, so `open_devtools` exists solely
                        // under `debug_assertions`. Release bundles carry no
                        // WKWebView inspector even with the env var set.
                        #[cfg(debug_assertions)]
                        if std::env::var_os("OCEAN_UI_DEBUG_DEVTOOLS").is_some() {
                            webview.open_devtools();
                        }
                        tauri::async_runtime::spawn(async move {
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            if let Err(error) = webview.eval(&script) {
                                eprintln!("Ocean UI debug script failed: {error}");
                            }
                        });
                    }
                    (Err(error), _) => {
                        eprintln!("Ocean UI debug script unreadable ({script_path}): {error}");
                    }
                    (_, None) => eprintln!("Ocean UI debug script: main webview unavailable"),
                }
            }
            // ocean:// deep links: bring the window forward and forward the
            // URL to the wasm bundle as a `deep-link` event (host::on_deep_link
            // parses `ocean://session/<id>` and switches sessions). OS-level
            // scheme registration requires a bundled app (Info.plist) — dev
            // builds only receive URLs via the plugin's runtime registration.
            {
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    show_main_window(&handle);
                    for url in event.urls() {
                        let _ = handle.emit("deep-link", url.to_string());
                    }
                });
            }
            // System-tray icon: app icon, "Ocean" tooltip, Show / Daemon / Quit.
            let show = MenuItem::with_id(app, "show", "Show Ocean", true, None::<&str>)?;
            let daemon_start_item =
                MenuItem::with_id(app, "daemon-start", "Start Daemon", true, None::<&str>)?;
            let daemon_restart_item =
                MenuItem::with_id(app, "daemon-restart", "Restart Daemon", true, None::<&str>)?;
            let sep_top = PredefinedMenuItem::separator(app)?;
            let sep_daemon = PredefinedMenuItem::separator(app)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &show,
                    &sep_top,
                    &daemon_start_item,
                    &daemon_restart_item,
                    &sep_daemon,
                    &quit,
                ],
            )?;

            let mut tray = TrayIconBuilder::new()
                .tooltip("Ocean")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    "daemon-start" => tray_daemon_action(app, DaemonTrayAction::Start),
                    "daemon-restart" => tray_daemon_action(app, DaemonTrayAction::Restart),
                    "quit" => app.exit(0),
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon().cloned() {
                tray = tray.icon(icon);
            }
            // `.build` registers the icon with the app's resources table, so it
            // outlives this setup closure without us holding the handle.
            tray.build(app)?;

            // Native app menu: a macOS app-name submenu (About/Quit roles, so
            // Cmd+Q and the bold app menu behave natively) plus a "Commands"
            // submenu mirroring the wasm CommandRegistry. Menu item ids EXACTLY
            // match the wasm command ids (app.rs registrations) — a selection
            // is re-emitted as `menu-command` (see `.on_menu_event` on the
            // builder) and routed back through `CommandRegistry::run`.
            //
            // Static v1: seven known items, hard-coded here. Dynamic sync from
            // the live registry (add/remove/enabled-state) is a wave-2 TODO —
            // see docs/OCEAN_DESKTOP_NORTH_STAR.md "Sequencing".
            // Standard macOS application menu. These predefined roles are not
            // cosmetic: AppKit owns their accelerators and behavior (including
            // Services, Cmd+H, Cmd+Option+H, and Cmd+Q).
            let about = PredefinedMenuItem::about(app, None, None)?;
            let app_sep_top = PredefinedMenuItem::separator(app)?;
            let services = PredefinedMenuItem::services(app, None)?;
            let app_sep_visibility = PredefinedMenuItem::separator(app)?;
            let hide = PredefinedMenuItem::hide(app, None)?;
            let hide_others = PredefinedMenuItem::hide_others(app, None)?;
            let show_all = PredefinedMenuItem::show_all(app, None)?;
            let app_sep_quit = PredefinedMenuItem::separator(app)?;
            let app_quit = PredefinedMenuItem::quit(app, None::<&str>)?;
            let app_submenu = Submenu::with_items(
                app,
                "Ocean",
                true,
                &[
                    &about,
                    &app_sep_top,
                    &services,
                    &app_sep_visibility,
                    &hide,
                    &hide_others,
                    &show_all,
                    &app_sep_quit,
                    &app_quit,
                ],
            )?;

            // File + Edit + Window are required native desktop affordances.
            // In particular, WebKit text fields do not reliably receive the
            // standard Cmd+X/C/V/A/Z shortcuts unless the corresponding native
            // edit roles exist in the application menu.
            let cmd_new_session =
                MenuItem::with_id(app, "new-session", "New Session", true, Some("CmdOrCtrl+N"))?;
            let file_sep = PredefinedMenuItem::separator(app)?;
            let close_window = PredefinedMenuItem::close_window(app, None)?;
            let file_submenu = Submenu::with_items(
                app,
                "File",
                true,
                &[&cmd_new_session, &file_sep, &close_window],
            )?;

            let undo = PredefinedMenuItem::undo(app, None)?;
            let redo = PredefinedMenuItem::redo(app, None)?;
            let edit_sep_history = PredefinedMenuItem::separator(app)?;
            let cut = PredefinedMenuItem::cut(app, None)?;
            let copy = PredefinedMenuItem::copy(app, None)?;
            let paste = PredefinedMenuItem::paste(app, None)?;
            let edit_sep_selection = PredefinedMenuItem::separator(app)?;
            let select_all = PredefinedMenuItem::select_all(app, None)?;
            let edit_submenu = Submenu::with_items(
                app,
                "Edit",
                true,
                &[
                    &undo,
                    &redo,
                    &edit_sep_history,
                    &cut,
                    &copy,
                    &paste,
                    &edit_sep_selection,
                    &select_all,
                ],
            )?;

            let minimize = PredefinedMenuItem::minimize(app, None)?;
            let maximize = PredefinedMenuItem::maximize(app, None)?;
            let fullscreen = PredefinedMenuItem::fullscreen(app, None)?;
            let window_sep = PredefinedMenuItem::separator(app)?;
            let bring_all_to_front = PredefinedMenuItem::bring_all_to_front(app, None)?;
            let window_submenu = Submenu::with_items(
                app,
                "Window",
                true,
                &[
                    &minimize,
                    &maximize,
                    &fullscreen,
                    &window_sep,
                    &bring_all_to_front,
                ],
            )?;
            let cmd_files = MenuItem::with_id(
                app,
                "toggle-files",
                "Toggle Files Explorer",
                true,
                None::<&str>,
            )?;
            let cmd_repo =
                MenuItem::with_id(app, "toggle-repo", "Toggle Repo Panel", true, None::<&str>)?;
            let cmd_browser = MenuItem::with_id(
                app,
                "toggle-browser",
                "Toggle Browser Cockpit",
                true,
                None::<&str>,
            )?;
            let cmd_sessions = MenuItem::with_id(
                app,
                "toggle-sessions",
                "Toggle Sessions",
                true,
                None::<&str>,
            )?;
            let cmd_rooms =
                MenuItem::with_id(app, "toggle-rooms", "Toggle Rooms", true, None::<&str>)?;
            let cmd_council = MenuItem::with_id(
                app,
                "open-council",
                "Open Council Stage",
                true,
                None::<&str>,
            )?;
            let cmd_workspace = MenuItem::with_id(
                app,
                "workspace-toggle",
                "Toggle Workspace",
                true,
                None::<&str>,
            )?;
            let commands_submenu = Submenu::with_items(
                app,
                "Commands",
                true,
                &[
                    &cmd_workspace,
                    &cmd_files,
                    &cmd_repo,
                    &cmd_browser,
                    &cmd_sessions,
                    &cmd_rooms,
                    &cmd_council,
                ],
            )?;

            let main_menu = Menu::with_items(
                app,
                &[
                    &app_submenu,
                    &file_submenu,
                    &edit_submenu,
                    &commands_submenu,
                    &window_submenu,
                ],
            )?;
            app.set_menu(main_menu)?;

            // Cmd+Shift+Space (macOS "super+shift+space") summons or hides Ocean.
            // A duplicate/contested registration is logged, not fatal — the tray
            // stays the always-available fallback.
            let summon = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::Space);
            if let Err(e) = app.global_shortcut().register(summon) {
                eprintln!("[ocean-tauri] could not register Cmd+Shift+Space global shortcut: {e}");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Menubar-app pattern: closing the main window hides it to the tray
            // rather than quitting. Real quit is via tray "Quit" (or Cmd+Q).
            if window.label() != "main" {
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .on_menu_event(|app, event| {
            // Re-emit app-menu selections as `menu-command`, carrying the menu
            // item's id. The wasm host bridge (host::on_menu_command) routes
            // the id to CommandRegistry::run; unknown ids — and the predefined
            // About/Quit roles, which also act natively — resolve to a no-op
            // inside the registry lookup, so emitting them is harmless. (Tray-
            // menu selections are handled by the tray's own on_menu_event and
            // never reach here.)
            //
            // Readiness gate: until the wasm `menu-command` listener attaches,
            // Tauri drops events with no subscriber. Selections arriving
            // pre-attach are queued in `MenuBridge::pending` and replayed (in
            // arrival order) by `ui_ready` once the bundle signals readiness.
            // After that, emit immediately.
            let id = event.id.as_ref().to_string();
            let state = app.state::<AppState>();
            let mut guard = state.menu.lock();
            if guard.ready {
                drop(guard);
                let _ = app.emit("menu-command", &id);
            } else {
                guard.pending.push(id);
            }
        })
        .invoke_handler(tauri::generate_handler![
            pick_folder,
            watch_paths,
            unwatch_paths,
            repo_state,
            set_badge,
            open_file,
            daemon_status,
            daemon_start,
            daemon_stop,
            daemon_restart,
            ui_ready,
            ui_debug_resize,
            open_external_url,
            daemon_operator_request
        ])
        .run(tauri::generate_context!())
        .expect("error while running ocean-tauri");
}
