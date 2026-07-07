//! Native backend for the ocean-surface Tauri shell.
//!
//! Hosts the same Leptos WASM bundle (`../../dist`) the browser PWA loads and
//! exposes the native affordances GPUI used to provide: a system folder picker
//! (replaces `rfd`) and recursive path watchers that debounce filesystem events
//! back to the webview as `path-changed` (replaces `ocean-gui/shell/watcher.rs`).

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_dialog::DialogExt;

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
#[derive(Default)]
struct AppState {
    watchers: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
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

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            pick_folder,
            watch_paths,
            unwatch_paths
        ])
        .run(tauri::generate_context!())
        .expect("error while running ocean-tauri");
}
