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
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_deep_link::DeepLinkExt;

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
    app.dialog()
        .file()
        .pick_folder(move |folder| {
            let _ = tx.send(folder);
        });
    let folder = rx
        .await
        .map_err(|_| "folder picker channel closed".to_string())?;
    Ok(folder.map(|f| f.to_string()))
}

/// Watch the given paths recursively, emitting `path-changed` events to the
/// webview, coalesced to a 200ms quiet window so editor saves don't fan out
/// into a burst. Replaces `ocean-gui/src/shell/watcher.rs`.
#[tauri::command]
async fn watch_paths(
    paths: Vec<String>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let mut watchers = state.watchers.lock();
    for raw in paths {
        let canonical = Path::new(&raw)
            .canonicalize()
            .map_err(|e| format!("{raw}: {e}"))?;
        let key = canonical.to_string_lossy().into_owned();

        // Re-watching a path replaces the prior watcher rather than leaking it.
        if let Some(stale) = watchers.remove(&key) {
            stale.abort();
        }

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
            .watch(&canonical, RecursiveMode::Recursive)
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

        watchers.insert(key, handle);
    }
    Ok(())
}

/// Stop watching the given paths. Paths are canonicalized best-effort — a path
/// that no longer exists is matched by its raw form.
#[tauri::command]
async fn unwatch_paths(paths: Vec<String>, state: State<'_, AppState>) -> Result<(), String> {
    let mut watchers = state.watchers.lock();
    for raw in paths {
        let key = match Path::new(&raw).canonicalize() {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(_) => raw,
        };
        if let Some(handle) = watchers.remove(&key) {
            handle.abort();
        }
    }
    Ok(())
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

/// Open a file with the OS default application.
///
/// * `root` — session workspace root (canonical).
/// * `path` — absolute path of the file to open.
///
/// Safety: `path` must be under `root` component-wise. Only regular files
/// are opened (directories pass through to the OS but are not the intended
/// use).
#[tauri::command]
fn open_file(root: String, path: String) -> Result<(), String> {
    let root = Path::new(&root)
        .canonicalize()
        .map_err(|e| format!("canonicalize root: {e}"))?;
    let target = Path::new(&path)
        .canonicalize()
        .map_err(|e| format!("canonicalize path: {e}"))?;
    // Component-wise prefix check: target must start with root.
    if !target.starts_with(&root) {
        return Err("path escapes workspace root".into());
    }
    if !target.is_file() {
        return Err("path is not a regular file".into());
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
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum DaemonState {
    /// Port reachable. `pid` is present iff the shell owns the child.
    Running,
    /// No child and the port is not reachable (never started / just stopped).
    Stopped,
    /// The shell owns a live child but the port is not up yet (just spawned).
    Starting,
    /// Port not reachable after previously being up, with no owned child —
    /// an external daemon that went away.
    Unreachable,
}

impl Default for DaemonState {
    fn default() -> Self {
        DaemonState::Stopped
    }
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
/// explicit arg → `OCEAN_DAEMON_BIN` env → `ocean-daemon` on PATH.
fn resolve_daemon_bin(explicit: Option<&str>) -> String {
    resolve_daemon_bin_with(explicit, std::env::var("OCEAN_DAEMON_BIN").ok().as_deref())
}

/// Pure core of [`resolve_daemon_bin`] — takes the env value as a parameter
/// so the resolution order is unit-testable without mutating process env.
fn resolve_daemon_bin_with(explicit: Option<&str>, env_bin: Option<&str>) -> String {
    if let Some(bin) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        return bin.to_string();
    }
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
    bin: Option<String>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<DaemonStatusDto, String> {
    state
        .daemon
        .start(&resolve_daemon_bin(bin.as_deref()))?;
    let snap = state.daemon.snapshot();
    let _ = app.emit("daemon-status", snap.clone());
    Ok(snap)
}

/// Stop the daemon the shell owns (never an external one). Emits a final
/// `daemon-status` event.
#[tauri::command]
async fn daemon_stop(state: State<'_, AppState>, app: AppHandle) -> Result<DaemonStatusDto, String> {
    state.daemon.stop();
    let snap = state.daemon.snapshot();
    let _ = app.emit("daemon-status", snap.clone());
    Ok(snap)
}

/// Restart the shell-owned daemon (stop + start). Emits a `daemon-status`
/// event.
#[tauri::command]
async fn daemon_restart(
    bin: Option<String>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<DaemonStatusDto, String> {
    state
        .daemon
        .restart(&resolve_daemon_bin(bin.as_deref()))?;
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
        Ok((a, b)) => (a as usize, b as usize),
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
        let id_short = format!("{}", &oid.to_string()[..7.min(oid.to_string().len())]);
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
    use std::fs;

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
        let result = repo_state(dir.path().to_string_lossy().into_owned())
            .expect("should not err");
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

    #[test]
    fn resolve_daemon_bin_explicit_wins_over_env_and_default() {
        // Explicit arg always wins, regardless of env.
        assert_eq!(
            resolve_daemon_bin_with(Some("/x/ocean-daemon"), Some("/env/od")),
            "/x/ocean-daemon"
        );
        // Whitespace-only explicit falls through.
        assert_eq!(
            resolve_daemon_bin_with(Some("   "), Some("/env/od")),
            "/env/od"
        );
    }

    #[test]
    fn resolve_daemon_bin_env_wins_over_path_default() {
        assert_eq!(resolve_daemon_bin_with(None, Some("/env/od")), "/env/od");
        // Whitespace-only env falls through to the PATH default.
        assert_eq!(resolve_daemon_bin_with(None, Some("  ")), "ocean-daemon");
    }

    #[test]
    fn resolve_daemon_bin_path_default_when_nothing_set() {
        assert_eq!(resolve_daemon_bin_with(None, None), "ocean-daemon");
    }

    #[test]
    fn decide_state_running_when_port_reachable() {
        // Reachable is Running regardless of child ownership (pid distinguishes
        // our-child vs external-daemon at the DTO layer, not here).
        assert_eq!(decide_daemon_state(true, true, false), DaemonState::Running);
        assert_eq!(decide_daemon_state(true, false, false), DaemonState::Running);
        assert_eq!(decide_daemon_state(true, true, true), DaemonState::Running);
    }

    #[test]
    fn decide_state_starting_when_child_alive_but_port_not_up() {
        assert_eq!(decide_daemon_state(false, true, false), DaemonState::Starting);
        assert_eq!(decide_daemon_state(false, true, true), DaemonState::Starting);
    }

    #[test]
    fn decide_state_stopped_vs_unreachable_uses_reachable_ever() {
        // Never reached + nothing serving + no child → Stopped.
        assert_eq!(decide_daemon_state(false, false, false), DaemonState::Stopped);
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
        let bin = resolve_daemon_bin(None);
        let _ = match action {
            DaemonTrayAction::Start => daemon.start(&bin),
            DaemonTrayAction::Restart => daemon.restart(&bin),
        };
        let snap = daemon.snapshot();
        let _ = app.emit("daemon-status", snap);
    });
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
        .plugin(tauri_plugin_fs::init())
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
            let about = PredefinedMenuItem::about(app, None, None)?;
            let app_sep = PredefinedMenuItem::separator(app)?;
            let app_quit = PredefinedMenuItem::quit(app, None::<&str>)?;
            let app_submenu =
                Submenu::with_items(app, "Ocean", true, &[&about, &app_sep, &app_quit])?;

            let cmd_new_session =
                MenuItem::with_id(app, "new-session", "New Session", true, None::<&str>)?;
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
            let cmd_sessions =
                MenuItem::with_id(app, "toggle-sessions", "Toggle Sessions", true, None::<&str>)?;
            let cmd_rooms =
                MenuItem::with_id(app, "toggle-rooms", "Toggle Rooms", true, None::<&str>)?;
            let cmd_council =
                MenuItem::with_id(app, "open-council", "Open Council Stage", true, None::<&str>)?;
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
                    &cmd_new_session,
                    &cmd_files,
                    &cmd_repo,
                    &cmd_browser,
                    &cmd_sessions,
                    &cmd_rooms,
                    &cmd_council,
                ],
            )?;

            let main_menu = Menu::with_items(app, &[&app_submenu, &commands_submenu])?;
            app.set_menu(main_menu)?;

            // Cmd+Shift+Space (macOS "super+shift+space") summons or hides Ocean.
            // A duplicate/contested registration is logged, not fatal — the tray
            // stays the always-available fallback.
            let summon = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::Space);
            if let Err(e) = app.global_shortcut().register(summon) {
                eprintln!(
                    "[ocean-tauri] could not register Cmd+Shift+Space global shortcut: {e}"
                );
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
            ui_ready
        ])
        .run(tauri::generate_context!())
        .expect("error while running ocean-tauri");
}
