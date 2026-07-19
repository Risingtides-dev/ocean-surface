//! Workspace pane — the permanent RIGHT-side desktop shell pane (Tauri only).
//!
//! Codex-desktop-style: a tab strip (Files empty-state · Preview tabs · Browser
//! slot) over a body that splits into an active-tab content region and a
//! docked, always-visible file tree. The tree lists the session cwd (same
//! `daemon.cwd` source the deck files panel uses), lazy-expands folders via
//! `GET /v1/fs/dirs?path=..&files=1`, and opens a file's contents in a Preview
//! tab via `GET /v1/fs/file?path=..`.
//!
//! Mounting is integrator-owned (app.rs) and gated on
//! [`crate::host::running_in_tauri`]; on the browser PWA and Chrome extension
//! this component never mounts, so the transcript-first layout is untouched.
//! The Browser tab is a STUB only — it renders `<div id="workspace-browser-slot">`
//! and a sibling agent (BrowserPaneTab) fills that slot; this module builds no
//! browser UI of its own.
//!
//! Pure helpers (`name_matches`, `sort_files`, `open_or_focus`, `close_tab`,
//! `format_kib`) are unit-testable without WASM.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use leptos::ev;
use leptos::portal::Portal;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

use crate::daemon::{
    fetch_fs_dirs_with_files, fetch_fs_file, Daemon, FsDirEntry, FsDirsResponse, FsFileEntry,
    FsFileResponse,
};
// Reuse the deck files panel's shared helpers rather than forking the
// sorting / refresh / basename logic — single source of truth for the
// session-cwd tree contract.
use crate::deck::files::{basename, browsable_root, is_secret_file, refresh_target, sort_entries};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Callback: open a file path with the OS default application.
/// Takes (path, pointer_x, pointer_y) — the root is baked in at the call site.
type OpenExternallyCb = Arc<dyn Fn(String, f64, f64) + Send + Sync>;

/// Which kind of content a workspace tab shows.
#[derive(Clone, Debug, PartialEq)]
pub enum TabKind {
    /// Persistent empty-state tab — prompts the user to pick a file.
    Files,
    /// A preview of one file, keyed by its absolute path. Closable.
    Preview(String),
    /// Persistent stub tab — renders the slot a sibling agent fills.
    Browser,
    /// Persistent repo-state tab — mounts the deck's RepoPanel. Never closable;
    /// sits last in the strip (after Browser).
    Repo,
}

/// One tab in the workspace strip. `id` is stable (`files` / `browser` / `repo`
/// / `preview:<path>`) so the strip can key on it.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceTab {
    pub id: String,
    pub title: String,
    pub kind: TabKind,
}

/// A one-shot intent to focus a specific workspace surface. Set on the pane's
/// `focus_intent` prop by the command layer (app.rs) when a toggle-* command
/// fires on Tauri — where Files/Browser/Repo live in THIS pane, not the deck.
/// The pane's Effect opens or focuses the matching persistent tab, makes it
/// active, then resets the signal to `None` so a repeat of the same intent
/// re-fires.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceFocus {
    Files,
    Browser,
    Repo,
    /// File-preview intent from a file-tree click or host deep-link. The
    /// workspace rail opens the deck, selects the Files tab, and the
    /// FilesPanel picks up the preview from the daemon's intent signal.
    Preview {
        path: String,
        generation: u64,
    },
}

// ---------------------------------------------------------------------------
// Pure helpers — unit-testable without WASM
// ---------------------------------------------------------------------------

/// Case-insensitive substring filter on a name. An empty/whitespace filter
/// passes everything (the idle state shows the full tree).
pub(crate) fn name_matches(name: &str, filter: &str) -> bool {
    let f = filter.trim();
    if f.is_empty() {
        return true;
    }
    name.to_lowercase().contains(&f.to_lowercase())
}

/// Sort file entries alphabetically by name (case-insensitive). Directories
/// keep the deck's repo-first ordering via [`sort_entries`]; files are a flat
/// alphabetical run beneath them.
pub(crate) fn sort_files(files: &mut [FsFileEntry]) {
    files.sort_by_key(|a| a.name.to_lowercase());
}

/// Render a byte size as whole KiB (round up so a 1-byte file reads "1 KiB",
/// never "0 KiB"; 0 bytes reads "0 KiB").
pub(crate) fn format_kib(size: u64) -> String {
    format!("{} KiB", size.div_ceil(1024))
}

/// Ensure a tab for `kind` exists in `tabs` and return its index, making it
/// the active tab. `Files`, `Browser`, and `Repo` are singletons (focus
/// existing); `Preview(path)` is keyed by path and inserted before the
/// persistent `Browser` tab so the strip keeps its Files · previews · Browser
/// · Repo order (Repo is always last — it is pushed onto the tail when missing
/// and never displaced by a new preview).
pub(crate) fn open_or_focus(tabs: &mut Vec<WorkspaceTab>, kind: &TabKind) -> usize {
    match kind {
        TabKind::Files => focus_or_push(tabs, kind, "files", "Files"),
        TabKind::Browser => focus_or_push(tabs, kind, "browser", "Browser"),
        TabKind::Repo => focus_or_push(tabs, kind, "repo", "Repo"),
        TabKind::Preview(path) => {
            if let Some(i) = tabs
                .iter()
                .position(|t| matches!(&t.kind, TabKind::Preview(p) if p == path))
            {
                return i;
            }
            let tab = WorkspaceTab {
                id: format!("preview:{}", path),
                title: basename(path).to_string(),
                kind: kind.clone(),
            };
            // Keep Browser last: insert just before it when present.
            let insert_at = tabs
                .iter()
                .position(|t| matches!(t.kind, TabKind::Browser))
                .unwrap_or(tabs.len());
            tabs.insert(insert_at, tab);
            insert_at
        }
    }
}

fn focus_or_push(tabs: &mut Vec<WorkspaceTab>, kind: &TabKind, id: &str, title: &str) -> usize {
    if let Some(i) = tabs.iter().position(|t| t.id == id) {
        return i;
    }
    tabs.push(WorkspaceTab {
        id: id.into(),
        title: title.into(),
        kind: kind.clone(),
    });
    tabs.len() - 1
}

/// Close the tab with `id`. Only `Preview` tabs are closable — closing a
/// persistent `Files`/`Browser`/`Repo` tab is a no-op. Returns the new active
/// index: closing the active tab moves focus to its left neighbour (clamped);
/// closing a tab to the right of active is focus-neutral.
pub(crate) fn close_tab(tabs: &mut Vec<WorkspaceTab>, active: usize, id: &str) -> usize {
    let Some(i) = tabs.iter().position(|t| t.id == id) else {
        return active;
    };
    if !matches!(tabs[i].kind, TabKind::Preview(_)) {
        return active;
    }
    tabs.remove(i);
    if tabs.is_empty() {
        return 0;
    }
    let new_active = if active > i {
        active - 1
    } else if active == i {
        // Removed the focused tab → land on the left neighbour (clamped).
        if i == 0 {
            0
        } else {
            i - 1
        }
    } else {
        active
    };
    new_active.min(tabs.len() - 1)
}

// ---------------------------------------------------------------------------
// Preview lifecycle — pure helpers (unit-testable without WASM)
// ---------------------------------------------------------------------------

/// Remove every closable `Preview` tab, returning the active index to use next.
/// Persistent `Files`/`Browser`/`Repo` tabs are preserved in order. If `active`
/// pointed at a persistent tab, focus follows it to its new index; if it pointed
/// at a removed `Preview` (or is out of range), focus lands on the first tab
/// (Files) — always a safe persistent surface.
pub(crate) fn clear_preview_tabs(tabs: &mut Vec<WorkspaceTab>, active: usize) -> usize {
    let active_persistent_id = tabs
        .get(active)
        .filter(|t| !matches!(t.kind, TabKind::Preview(_)))
        .map(|t| t.id.clone());
    tabs.retain(|t| !matches!(t.kind, TabKind::Preview(_)));
    match active_persistent_id {
        Some(id) => tabs.iter().position(|t| t.id == id).unwrap_or(0),
        None => 0,
    }
}

/// Whether a preview-generation retire is warranted: any change to session
/// identity or browsable root retires the generation (and wipes preview state);
/// a stable session at a stable root does not, so same-session tree refreshes
/// never spuriously clear open previews.
pub(crate) fn preview_gen_should_retire(
    prev_session: &Option<String>,
    next_session: &Option<String>,
    prev_root: &Option<String>,
    next_root: &Option<String>,
) -> bool {
    prev_session != next_session || prev_root != next_root
}

/// Admission check for an async file-read completion: a result is applied only
/// when the generation it was issued under still matches the live generation.
/// A read from a retired generation (session/root changed mid-flight) is
/// discarded so it cannot repopulate the prior session's preview state.
pub(crate) fn read_is_current(issued_generation: u64, live_generation: u64) -> bool {
    issued_generation == live_generation
}

// ---------------------------------------------------------------------------
// Shared callback types
// ---------------------------------------------------------------------------

type DirCallback = Arc<dyn Fn(String) + Send + Sync>;
type FileCallback = Arc<dyn Fn(String) + Send + Sync>;

// ---------------------------------------------------------------------------
// Pane geometry — pure width math (unit-tested) + persisted-layout constants
// ---------------------------------------------------------------------------

/// Desired pane width when none is persisted; MUST equal the pre-hydration
/// `--workspace-w` fallback in workspace.css (`:root`).
const DEFAULT_WORKSPACE_W: f64 = 520.0;
/// Hard floor on the pane — wins over every other constraint (a degenerate
/// tiny window accepts a pane wider than its viewport rather than vanishing).
const MIN_PANE_W: f64 = 320.0;
/// In split view the content column is guaranteed at least this much, so the
/// pane's max is the tighter of (65% of viewport) and (viewport − this floor).
const COLUMN_MIN_W: f64 = 480.0;
/// Split-view breakpoint; MUST equal the `@media (min-width: 900px)` rule in
/// workspace.css. At or above this the pane SPLITS; below it OVERLAYS.
const SPLIT_BP: f64 = 900.0;
/// Pane may take at most this fraction of the viewport in split view.
const MAX_PANE_FACTOR: f64 = 0.65;
/// In overlay mode a sliver of the underlying context stays visible.
const OVERLAY_EDGE_W: f64 = 48.0;
/// Below this pane width a Preview/Browser tab auto-collapses the tree.
const NARROW_PANE_W: f64 = 560.0;

/// Pure auto-collapse rule for the docked tree: only wide content tabs
/// (Preview/Browser) collapse it, and only in a narrow pane. Never Files —
/// the tree is that tab's body — and never Repo (a compact list that
/// coexists with the tree).
fn tree_auto_collapses(kind: &TabKind, pane_w: f64) -> bool {
    matches!(kind, TabKind::Preview(_) | TabKind::Browser) && pane_w < NARROW_PANE_W
}

const LS_WIDTH: &str = "ocean.workspace.w";
const LS_TREE: &str = "ocean.workspace.tree";
const LS_TREE_USER: &str = "ocean.workspace.tree.user-set";

/// Effective pane width for a desired width at a given viewport.
///
/// Split mode (`vw >= SPLIT_BP`): the pane may take up to 65% of the viewport
/// but must always leave [`COLUMN_MIN_W`] for the content column. Overlay mode
/// (`vw < SPLIT_BP`): the pane may cover all but an [`OVERLAY_EDGE_W`] context
/// sliver. [`MIN_PANE_W`] wins over everything (degenerate tiny windows
/// accepted).
pub(crate) fn effective_width(desired: f64, vw: f64) -> f64 {
    let max = if vw >= SPLIT_BP {
        (vw * MAX_PANE_FACTOR).min(vw - COLUMN_MIN_W)
    } else {
        vw - OVERLAY_EDGE_W
    };
    desired.clamp(MIN_PANE_W, max.max(MIN_PANE_W))
}

// ---------------------------------------------------------------------------
// WorkspacePane component
// ---------------------------------------------------------------------------

/// Map a WorkspaceFocus intent to its canonical TabKind.
/// Shared by the production Effect and its unit tests.
fn workspace_focus_tab_kind(focus: &WorkspaceFocus) -> TabKind {
    match focus {
        WorkspaceFocus::Files => TabKind::Files,
        WorkspaceFocus::Browser => TabKind::Browser,
        WorkspaceFocus::Repo => TabKind::Repo,
        WorkspaceFocus::Preview { path, .. } => TabKind::Preview(path.clone()),
    }
}

/// The permanent right-side desktop pane. `open` is the shared collapse state
/// owned by the app shell (header toggle + ⌘K command flip the same signal);
/// the pane renders `is-collapsed` from it and the shell adjusts its layout
/// gutter to match. `focus_intent` is a one-shot set by the command layer
/// (app.rs) to surface a specific tab when a toggle-* command fires on Tauri —
/// the Effect below opens/focuses the matching tab and resets it to `None`.
#[component]
pub fn WorkspacePane(
    daemon: Daemon,
    open: RwSignal<bool>,
    focus_intent: RwSignal<Option<WorkspaceFocus>>,
) -> impl IntoView {
    // Root = session cwd, the same source the deck files panel reads. A
    // non-browsable cwd (unset, "/", or the projectless-chat /tmp pin)
    // leaves the tree empty until a real project session lands.
    let tree_root: RwSignal<Option<String>> =
        RwSignal::new(browsable_root(&daemon.cwd.get_untracked()).map(str::to_string));
    let daemon_url = daemon.url;

    // Tabs: Files, Browser, and Repo are persistent; Preview tabs come and go.
    // Repo sits LAST (after Browser) and is never closable.
    let tabs: RwSignal<Vec<WorkspaceTab>> = RwSignal::new(vec![
        WorkspaceTab {
            id: "files".into(),
            title: "Files".into(),
            kind: TabKind::Files,
        },
        WorkspaceTab {
            id: "browser".into(),
            title: "Browser".into(),
            kind: TabKind::Browser,
        },
        WorkspaceTab {
            id: "repo".into(),
            title: "Repo".into(),
            kind: TabKind::Repo,
        },
    ]);
    let active_tab: RwSignal<usize> = RwSignal::new(0);

    // Tree state — mirrors the deck files panel exactly.
    let dir_cache: RwSignal<HashMap<String, FsDirsResponse>> = RwSignal::new(HashMap::new());
    let loading_path: RwSignal<Option<String>> = RwSignal::new(None);
    let load_error: RwSignal<Option<String>> = RwSignal::new(None);
    let expanded: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let tree_filter: RwSignal<String> = RwSignal::new(String::new());

    // Preview state — one content cache per path, plus path-scoped load/error.
    let preview_cache: RwSignal<HashMap<String, FsFileResponse>> = RwSignal::new(HashMap::new());
    let preview_loading: RwSignal<Option<String>> = RwSignal::new(None);
    let preview_error: RwSignal<Option<(String, String)>> = RwSignal::new(None);

    // Preview lifecycle guard. `preview_generation` binds every async file read
    // to the {session, browsable-root} it was issued under; `preview_session`
    // tracks the session identity the current previews belong to. A change to
    // either session identity or root bumps the generation, wipes preview
    // surface state, and drops Preview tabs — so a late read or a docked tab can
    // never carry the prior session's file into a new one (TASK-27).
    let preview_generation: RwSignal<u64> = RwSignal::new(0);
    let preview_session: RwSignal<Option<String>> =
        RwSignal::new(daemon.session_id.get_untracked());

    // Context menu (B0: Open Externally). (path, pointer_x, pointer_y).
    // Root is resolved from tree_root at render time.
    let context_menu: RwSignal<Option<(String, f64, f64)>> = RwSignal::new(None);
    let context_menu_item_ref = NodeRef::<leptos::html::Button>::new();

    // Auto-focus the first menuitem when the portal appears so Esc/Enter
    // work without an extra click.
    {
        let cm = context_menu;
        let item_ref = context_menu_item_ref;
        Effect::new(move |_| {
            if cm.get().is_some() {
                if let Some(el) = item_ref.get() {
                    let _ = el.focus();
                }
            }
        });
    }

    // ---- Tree load / expand / collapse (mirror deck::files::FilesPanel) ----

    let load_dir: DirCallback = {
        Arc::new(move |path: String| {
            let url = daemon_url.get_untracked();
            loading_path.set(Some(path.clone()));
            load_error.set(None);
            spawn_local(async move {
                if let Some(resp) = fetch_fs_dirs_with_files(&url, &path).await {
                    // Daemon success responses carry no `ok` field (serde
                    // default = false) — failure is signalled by `error`, so
                    // THAT is the success predicate (same as the files panel).
                    if resp.error.is_none() {
                        dir_cache.update(|cache| {
                            cache.insert(path, resp);
                        });
                    } else {
                        load_error.set(resp.error);
                    }
                } else {
                    load_error.set(Some("Failed to reach daemon".into()));
                }
                loading_path.set(None);
            });
        })
    };

    let expand_dir: DirCallback = {
        let load_dir = Arc::clone(&load_dir);
        Arc::new(move |path: String| {
            let loaded = dir_cache.with(|c| c.contains_key(&path));
            expanded.update(|e| {
                e.insert(path.clone());
            });
            if !loaded {
                loading_path.set(Some(path.clone()));
                load_dir(path);
            }
        })
    };

    let collapse_dir: DirCallback = {
        Arc::new(move |path: String| {
            expanded.update(|e| {
                e.remove(&path);
            });
        })
    };

    // ---- File content fetch (shared by open + watcher refetch) ----

    let fetch_file_content: FileCallback = {
        Arc::new(move |path: String| {
            let url = daemon_url.get_untracked();
            // Capture the generation this read is issued under; its completion
            // is admitted only if the session/root has not changed meanwhile.
            let issued_generation = preview_generation.get_untracked();
            preview_loading.set(Some(path.clone()));
            preview_error.set(None);
            spawn_local(async move {
                let fetched = fetch_fs_file(&url, &path).await;
                // Discard a completion from a retired generation so a stale read
                // cannot repopulate the prior session's preview state.
                if !read_is_current(issued_generation, preview_generation.get_untracked()) {
                    return;
                }
                if let Some(resp) = fetched {
                    if resp.error.is_none() {
                        preview_cache.update(|c| {
                            c.insert(path.clone(), resp);
                        });
                    } else {
                        preview_error.set(Some((
                            path.clone(),
                            resp.error.unwrap_or_else(|| "read failed".into()),
                        )));
                    }
                } else {
                    preview_error.set(Some((path.clone(), "Failed to reach daemon".into())));
                }
                preview_loading.set(None);
            });
        })
    };

    // ---- File open: focus/open a Preview tab, then fetch on cache miss ----

    let open_file: FileCallback = {
        let fetch_file_content = Arc::clone(&fetch_file_content);
        Arc::new(move |path: String| {
            // 1. Open/focus the Preview tab for this path.
            let mut t = tabs.get();
            let idx = open_or_focus(&mut t, &TabKind::Preview(path.clone()));
            tabs.set(t);
            active_tab.set(idx);
            // 2. Fetch content only on a cache miss.
            if preview_cache.with_untracked(|c| c.contains_key(&path)) {
                return;
            }
            fetch_file_content(path);
        })
    };

    // ---- Watcher: tree refresh + live preview refetch ----

    {
        let load_dir = Arc::clone(&load_dir);

        let fetch_file_content = Arc::clone(&fetch_file_content);
        crate::host::on_path_changed(move |ev| {
            // Tree: re-list the parent of the changed path (the deck pattern).
            if let Some(root) = tree_root.get() {
                match refresh_target(&root, &ev.path, &ev.kind) {
                    Some(dir) => load_dir(dir),
                    // refresh_target returns None when the event IS the root
                    // (or outside it) — drop cached subtrees and reload root.
                    None => {
                        dir_cache.update(|c| {
                            c.retain(|p, _| !p.starts_with(&root));
                        });
                        expanded.update(|e| {
                            e.retain(|p| !p.starts_with(&root));
                        });
                        load_dir(root);
                    }
                }
            }
            // Preview: if an open file changed on disk, refetch its content.
            if preview_cache.with_untracked(|c| c.contains_key(&ev.path)) {
                fetch_file_content(ev.path.clone());
            }
        });
    }

    // ---- Initial root load + session-cwd follow (mirror deck) ----

    {
        let load_dir = Arc::clone(&load_dir);
        let cwd_sig = daemon.cwd;
        let session_sig = daemon.session_id;
        Effect::new(move |_| {
            // Subscribe to BOTH session identity and cwd: a change to either
            // retires the current preview generation. (Reads are ordered so the
            // effect re-runs whenever either signal changes.)
            let session = session_sig.get();
            let cwd = cwd_sig.get();
            let new_root = browsable_root(&cwd).map(str::to_string);

            // Session identity or browsable-root change → wipe preview surface
            // state and retire in-flight reads, so no docked Preview tab or late
            // completion carries the prior session's file forward (TASK-27).
            if preview_gen_should_retire(
                &preview_session.get_untracked(),
                &session,
                &tree_root.get_untracked(),
                &new_root,
            ) {
                preview_generation.update(|g| *g += 1);
                preview_session.set(session);
                preview_cache.set(HashMap::new());
                preview_loading.set(None);
                preview_error.set(None);
                context_menu.set(None);
                let mut t = tabs.get_untracked();
                let new_active = clear_preview_tabs(&mut t, active_tab.get_untracked());
                tabs.set(t);
                active_tab.set(new_active);
            }

            let Some(root) = new_root else {
                // Session reset ("New chat" pins cwd to /tmp) or no session:
                // clear the tree and any lingering load error (e.g. "access
                // denied: /tmp is outside home directory") so the pane shows
                // its clean empty state instead of stale state (QA-007).
                if tree_root.get_untracked().is_some() {
                    tree_root.set(None);
                }
                load_error.set(None);
                return;
            };
            if tree_root.get_untracked().as_deref() != Some(root.as_str()) {
                tree_root.set(Some(root.clone()));
                load_error.set(None);
            }
            if !dir_cache.with_untracked(|c| c.contains_key(&root)) {
                load_dir(root);
            }
        });
    }

    // ---- Command-layer focus intent (one-shot) ----
    // app.rs sets focus_intent when a toggle-* command fires on Tauri (where
    // the Files/Browser/Repo surfaces live in THIS pane, not the deck).
    // Routes through workspace_focus_tab_kind (shared helper) so the
    // WorkspaceFocus → TabKind mapping is unit-testable.
    Effect::new({
        let open_file = Arc::clone(&open_file);
        move |_| {
            let Some(f) = focus_intent.get() else {
                return;
            };
            let kind = workspace_focus_tab_kind(&f);
            match kind {
                TabKind::Preview(path) => {
                    // Route through open_file for cache-miss fetch.
                    open_file(path);
                }
                _ => {
                    let mut t = tabs.get();
                    let idx = open_or_focus(&mut t, &kind);
                    tabs.set(t);
                    active_tab.set(idx);
                }
            }
            focus_intent.set(None);
        }
    });

    // Pre-clone for the view's render closures.
    let ld_tree = Arc::clone(&load_dir);
    let ed_tree = Arc::clone(&expand_dir);
    let cd_tree = Arc::clone(&collapse_dir);
    let of_tree = Arc::clone(&open_file);
    let ctx_menu_cb: OpenExternallyCb = {
        let cm = context_menu;
        Arc::new(move |path, x, y| {
            cm.set(Some((path, x, y)));
        })
    };
    // Hoisted out of `view!`: a turbofish (`collect::<Vec<_>>()`) inside an
    // RSX attribute position parses `<Vec<_>>` as a tag opener.
    let tab_entries = move || tabs.get().into_iter().enumerate().collect::<Vec<_>>();
    // Dedicated clones for the tab-content closures below (FnMut, re-runs):
    // the Browser tab renders the sibling module's live screencast component,
    // and the Repo tab mounts the deck's RepoPanel (same component the deck
    // uses — the workspace is its home on Tauri).
    let daemon_browser = daemon.clone();
    let daemon_repo = daemon.clone();

    // ── ergonomics: drag-resize + collapsible tree ────────────────────────────
    // The DESIRED width persists across sessions; the EFFECTIVE pane width
    // (`pane_w`, derived) clamps it to the live viewport so the shell gutter
    // never outruns the content column. The live `--workspace-w` custom property
    // has exactly ONE writer — the effect below — so handlers only ever mutate
    // `desired_w` / `viewport_w`.

    fn ls_get_f64(key: &str) -> Option<f64> {
        web_sys::window()?
            .local_storage()
            .ok()??
            .get_item(key)
            .ok()?
            .and_then(|s| s.parse::<f64>().ok())
    }
    fn ls_get_bool(key: &str) -> bool {
        web_sys::window()
            .and_then(|w| w.local_storage().ok())
            .and_then(|r| r)
            .and_then(|r| r.get_item(key).ok())
            .flatten()
            .map(|s| s == "1")
            .unwrap_or(false)
    }
    fn ls_set(key: &str, val: &str) {
        let _ = web_sys::window()
            .and_then(|w| w.local_storage().ok())
            .and_then(|r| r)
            .map(|r| r.set_item(key, val));
    }
    fn read_viewport_w() -> f64 {
        web_sys::window()
            .and_then(|w| w.inner_width().ok())
            .and_then(|n| n.as_f64())
            .unwrap_or(1280.0)
    }
    fn set_workspace_w_var(px: f64) {
        let css = format!("{px}px");
        if let Some(el) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
        {
            if let Ok(el) = el.dyn_into::<web_sys::HtmlElement>() {
                let _ = el.style().set_property("--workspace-w", &css);
            }
        }
    }
    /// Toggle the `is-resizing` class on <html>. During a drag the pointer moves
    /// the pane instantly; animating the shell gutter behind it reads as
    /// rubber-band churn, so workspace.css suppresses layout transitions while the
    /// class is present. `pointercancel` mirrors `pointerup` because pointer
    /// capture can be revoked by the OS mid-gesture.
    fn set_resizing(on: bool) {
        if let Some(el) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
        {
            if let Ok(el) = el.dyn_into::<web_sys::HtmlElement>() {
                let _ = el.class_list().toggle_with_force("is-resizing", on);
            }
        }
    }
    // tiny helper because leptos::ev::Pointer* target access is verbose
    fn ev_target_element(ev: &web_sys::PointerEvent) -> Option<web_sys::Element> {
        let t = ev.target()?;
        t.dyn_into::<web_sys::Element>().ok()
    }

    let desired_w: RwSignal<f64> =
        RwSignal::new(ls_get_f64(LS_WIDTH).unwrap_or(DEFAULT_WORKSPACE_W));
    let viewport_w: RwSignal<f64> = RwSignal::new(read_viewport_w());
    let user_set_tree: RwSignal<bool> = RwSignal::new(ls_get_bool(LS_TREE_USER));
    let tree_collapsed_user: RwSignal<bool> = RwSignal::new(ls_get_bool(LS_TREE));
    // Effective pane width: the desired width clamped to the live viewport. This
    // is the single value every consumer reads (the CSS-var effect below, the
    // tree auto-collapse heuristic) — nothing recomputes the clamp itself.
    let pane_w = Signal::derive(move || effective_width(desired_w.get(), viewport_w.get()));
    // Files tab = the docked tree IS the body (the content region renders
    // nothing there), so while it is active the tree can never collapse away —
    // a collapsed body would leave a dead pane. Auto-collapse only applies to
    // wide content tabs (Preview/Browser) in a narrow pane; a user collapse
    // applies to any content tab but is likewise overridden on Files.
    let files_active = Signal::derive(move || {
        let active = active_tab.get();
        tabs.with(|t| {
            t.get(active)
                .map(|tab| matches!(tab.kind, TabKind::Files))
                .unwrap_or(false)
        })
    });
    let tree_collapsed = {
        Signal::derive(move || {
            if files_active.get() {
                false
            } else if user_set_tree.get() {
                tree_collapsed_user.get()
            } else {
                let active = active_tab.get();
                let tabs_vec = tabs.get();
                tabs_vec
                    .get(active)
                    .map(|t| tree_auto_collapses(&t.kind, pane_w.get()))
                    .unwrap_or(false)
            }
        })
    };
    let dragging: RwSignal<bool> = RwSignal::new(false);

    // Single writer of the live `--workspace-w` custom property: the derived pane
    // width drives it, so handlers only mutate `desired_w` / `viewport_w`. Runs
    // once at setup (the initial sync — the CSS :root fallback holds until then)
    // and again on every drag tick or window resize.
    Effect::new(move |_| {
        set_workspace_w_var(pane_w.get());
    });

    // Keep the viewport in sync so the derived width re-clamps when the OS resizes
    // the window (e.g. snapping a maximized window back to a tile). Bound +
    // on_cleanup so the listener lives with the component scope and is torn down
    // on unmount (matching app.rs's pointer-light listener).
    let _resize_listener = window_event_listener(ev::resize, move |_| {
        viewport_w.set(read_viewport_w());
    });
    on_cleanup(move || _resize_listener.remove());

    let on_resize_move = move |ev: web_sys::PointerEvent| {
        if !dragging.get() {
            return;
        }
        let x = ev.client_x() as f64;
        let vw = viewport_w.get();
        // WYSIWYG: store the EFFECTIVE width at this pointer position as the
        // desired width, so the handle can never outrun the clamp — a drag into
        // the no-go zone parks at the boundary instead of over/undershooting.
        desired_w.set(effective_width(vw - x, vw));
    };
    let on_resize_up = move |ev: web_sys::PointerEvent| {
        dragging.set(false);
        set_resizing(false);
        if let Some(el) = ev_target_element(&ev) {
            let _ = el.release_pointer_capture(ev.pointer_id());
        }
        // Persist only on release (unchanged from the original behavior).
        ls_set(LS_WIDTH, &desired_w.get_untracked().to_string());
    };
    // Pointer capture can be cancelled by the OS mid-gesture; mirror pointerup so
    // the is-resizing class and drag lock never get stuck on.
    let on_resize_cancel = move |ev: web_sys::PointerEvent| {
        dragging.set(false);
        set_resizing(false);
        if let Some(el) = ev_target_element(&ev) {
            let _ = el.release_pointer_capture(ev.pointer_id());
        }
        ls_set(LS_WIDTH, &desired_w.get_untracked().to_string());
    };
    let on_resize_down = move |ev: web_sys::PointerEvent| {
        dragging.set(true);
        set_resizing(true);
        if let Some(el) = ev_target_element(&ev) {
            let _ = el.set_pointer_capture(ev.pointer_id());
        }
    };
    let reset_width = move |_| {
        desired_w.set(DEFAULT_WORKSPACE_W);
        ls_set(LS_WIDTH, &DEFAULT_WORKSPACE_W.to_string());
    };
    let toggle_tree = move |_| {
        let now = !tree_collapsed.get();
        user_set_tree.set(true);
        tree_collapsed_user.set(now);
        ls_set(LS_TREE_USER, "1");
        ls_set(LS_TREE, if now { "1" } else { "0" });
    };
    view! {
        <aside
            class="workspace"
            class:is-collapsed=move || !open.get()
            class:files-active=move || files_active.get()
            role="complementary"
            aria-label="Workspace"
        >
            <div
                class="workspace-resize"
                role="separator"
                aria-orientation="vertical"
                aria-label="Resize workspace"
                title="Drag to resize — double-click to reset"
                on:pointerdown=on_resize_down
                on:pointermove=on_resize_move
                on:pointerup=on_resize_up
                on:pointercancel=on_resize_cancel
                on:dblclick=reset_width
            ></div>
            // ---- Tab strip ----
            <div class="workspace-tabs">
                <For
                    each=tab_entries
                    key=|(_, t)| t.id.clone()
                    children=move |(index, tab)| {
                        let tab_id = tab.id.clone();
                        let tab_title = tab.title.clone();
                        let closable = matches!(tab.kind, TabKind::Preview(_));
                        let preview_path = match &tab.kind {
                            TabKind::Preview(p) => Some(p.clone()),
                            _ => None,
                        };
                        view! {
                            <button
                                class="workspace-tab"
                                class:is-active=move || active_tab.get() == index
                                type="button"
                                title=tab_title.clone()
                                on:click=move |_| active_tab.set(index)
                                on:contextmenu={
                                    let p = preview_path.clone();
                                    move |ev| {
                                        if let Some(ref path) = p {
                                            ev.prevent_default();
                                            context_menu.set(Some((
                                                path.clone(),
                                                ev.client_x() as f64,
                                                ev.client_y() as f64,
                                            )));
                                        }
                                    }
                                }
                            >
                                <span class="workspace-tab__title">{tab_title.clone()}</span>
                                {closable.then(|| {
                                    let id_for_close = tab_id.clone();
                                    view! {
                                        <span
                                            class="workspace-tab__close"
                                            title="Close"
                                            on:click=move |ev| {
                                                ev.stop_propagation();
                                                let mut t = tabs.get();
                                                let new_active =
                                                    close_tab(&mut t, active_tab.get(), &id_for_close);
                                                tabs.set(t);
                                                active_tab.set(new_active);
                                            }
                                        ><crate::icons::Close /></span>
                                    }
                                })}
                            </button>
                        }
                    }
                />
            </div>

            // ---- Body: active-tab content + docked tree ----
            <div class="workspace-body">
                // Active-tab content region.
                {move || {
                    let active = active_tab.get();
                    let tabs_vec = tabs.get();
                    let Some(tab) = tabs_vec.get(active) else {
                        return view! { <div class="workspace-empty"></div> }.into_any();
                    };
                    match &tab.kind {
                        TabKind::Files => {
                            // No content node — the docked tree IS this tab's
                            // body; `.workspace.files-active` (workspace.css)
                            // flexes the tree to fill the pane.
                            ().into_any()
                        }
                        TabKind::Browser => view! {
                            <div class="workspace-browser">
                                // Live agent-browser view: CDP screencast of the
                                // daemon-owned Chrome + input forwarding
                                // (workspace_browser.rs).
                                <crate::workspace_browser::WorkspaceBrowser daemon=daemon_browser.clone() />
                            </div>
                        }
                        .into_any(),
                        TabKind::Repo => view! {
                            <div class="workspace-repo">
                                // Repo state (branch, dirty/staged, commits) —
                                // the same RepoPanel the deck mounts; the
                                // workspace is its home on Tauri.
                                <crate::deck::repo::RepoPanel daemon=daemon_repo.clone() />
                            </div>
                        }
                        .into_any(),
                        TabKind::Preview(path) => {
                            let path = path.clone();
                            // One clone per reactive closure below — each is
                            // FnMut and re-runs, so none may consume `path`.
                            let path_loading = path.clone();
                            let path_error = path.clone();
                            view! {
                                <div class="workspace-preview">
                                    {move || {
                                        preview_loading
                                            .get()
                                            .filter(|p| *p == path_loading)
                                            .map(|p| {
                                                view! {
                                                    <div class="workspace-loading">
                                                        {format!("Loading {}…", basename(&p))}
                                                    </div>
                                                }
                                            })
                                    }}
                                    {move || {
                                        preview_error
                                            .get()
                                            .filter(|(p, _)| *p == path_error)
                                            .map(|(_, msg)| {
                                                view! {
                                                    <div class="workspace-error">{msg}</div>
                                                }
                                            })
                                    }}
                                    {move || match preview_cache.with(|c| c.get(&path).cloned()) {
                                        None => view! {
                                            <div class="workspace-empty">
                                                "Open file — select a file from the tree"
                                            </div>
                                        }
                                        .into_any(),
                                        Some(r) if r.binary => view! {
                                            <div class="workspace-empty">
                                                {format!("binary file ({})", format_kib(r.size))}
                                            </div>
                                        }
                                        .into_any(),
                                        Some(r) => view! {
                                            <div class="workspace-preview__scroll">
                                                {r.truncated.then(|| {
                                                    view! {
                                                        <div class="workspace-preview__banner">
                                                            {format!(
                                                                "truncated — file is {}",
                                                                format_kib(r.size),
                                                            )}
                                                        </div>
                                                    }
                                                })}
                                                <pre class="workspace-preview__code">
                                                    <code>{r.content}</code>
                                                </pre>
                                            </div>
                                        }
                                        .into_any(),
                                    }}
                                </div>
                            }
                            .into_any()
                        }
                    }
                }}

                // ---- Docked file tree (right edge inside the pane) ----
                <aside class="workspace-tree" class:is-collapsed=move || tree_collapsed.get() aria-label="Files">
                    <button
                        class="workspace-tree-toggle"
                        class:is-collapsed=move || tree_collapsed.get()
                        type="button"
                        title=move || if tree_collapsed.get() { "Show file tree" } else { "Hide file tree" }
                        on:click=toggle_tree
                    >
                        <crate::icons::ChevronDown />
                    </button>
                    <div class="workspace-tree-filter">
                        <input
                            class="workspace-tree-filter__input"
                            type="search"
                            placeholder="Filter files…"
                            prop:value=move || tree_filter.get()
                            on:input=move |ev| tree_filter.set(event_target_value(&ev))
                        />
                    </div>
                    <div class="workspace-tree-body">
                        {move || {
                            let Some(root) = tree_root.get() else {
                                return view! {
                                    <div class="workspace-empty">
                                        "No folder — start a session to explore."
                                    </div>
                                }
                                .into_any();
                            };
                            // Loading / error banners for the root listing.
                            let root_loading = loading_path.get().map(|_| {
                                view! { <div class="workspace-loading">"Loading…"</div> }
                            });
                            let root_error = load_error.get().map(|err| {
                                view! { <div class="workspace-error">{err}</div> }
                            });
                            let entries = dir_cache.with(|c| c.get(&root).cloned());
                            let body = match entries {
                                None => view! { <div></div> }.into_any(),
                                Some(resp) => {
                                    let mut dirs = resp.dirs.clone();
                                    sort_entries(&mut dirs);
                                    let mut files = resp.files.clone();
                                    sort_files(&mut files);
                                    let filter = tree_filter.get();
                                    view! {
                                        <ul class="workspace-tree-list">
                                            {dirs
                                                .into_iter()
                                                .filter(|d| name_matches(&d.name, &filter))
                                                .map(|d| {
                                                    dir_row(
                                                        d,
                                                        1,
                                                        dir_cache,
                                                        expanded,
                                                        tree_filter,
                                                        Arc::clone(&of_tree),
                                                        Arc::clone(&ld_tree),
                                                        Arc::clone(&ed_tree),
                                                        Arc::clone(&cd_tree),
                                                        Some(ctx_menu_cb.clone()),
                                                    )
                                                })
                                                .collect::<Vec<_>>()}
                                            {files
                                                .into_iter()
                                                .filter(|f| !is_secret_file(&f.name))
                                                .filter(|f| name_matches(&f.name, &filter))
                                                .map(|f| file_row(f, 1, Arc::clone(&of_tree), Some(ctx_menu_cb.clone())))
                                                .collect::<Vec<_>>()}
                                        </ul>
                                    }
                                    .into_any()
                                }
                            };
                            view! {
                                {root_loading}
                                {root_error}
                                {body}
                            }
                            .into_any()
                        }}
                    </div>
                </aside>
            </div>
        </aside>

        // ── Context menu (B0: Open Externally) ──────────────────────────
        // Portal to document.body so it escapes the tree's scroll clipping
        // and z-index stacking.
        {move || {
            let Some((path, x, y)) = context_menu.get() else {
                return ().into_any();
            };
            // Clamp to viewport so the menu never bleeds off-screen.
            let vw = web_sys::window()
                .and_then(|w| w.inner_width().ok().map(|w| w.as_f64().unwrap_or(1024.0)))
                .unwrap_or(1024.0);
            let vh = web_sys::window()
                .and_then(|w| w.inner_height().ok().map(|h| h.as_f64().unwrap_or(768.0)))
                .unwrap_or(768.0);
            let clamped_x = x.max(4.0).min(vw - 188.0);
            let clamped_y = y.max(4.0).min(vh - 40.0);
            let root = tree_root.get();
            let body = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.body())
                .unwrap();
            let item_ref = context_menu_item_ref;
            view! {
                <Portal mount=body>
                    // Invisible backdrop captures outside clicks + Esc.
                    <div
                        class="workspace-context-overlay"
                        on:click=move |_| context_menu.set(None)
                        on:contextmenu=move |ev| {
                            ev.prevent_default();
                            context_menu.set(None);
                        }
                        on:keydown=move |ev| {
                            if ev.key() == "Escape" {
                                ev.prevent_default();
                                context_menu.set(None);
                            }
                        }
                    >
                        <div
                            class="workspace-context-menu"
                            style=format!("left: {clamped_x}px; top: {clamped_y}px")
                            role="menu"
                            aria-label="File actions"
                        >
                            <button
                                node_ref=item_ref
                                class="workspace-context-menu__item"
                                type="button"
                                role="menuitem"
                                on:click={
                                    let context_root = root.clone();
                                    let context_path = path.clone();
                                    move |_| {
                                        context_menu.set(None);
                                        let r = context_root.clone();
                                        let p = context_path.clone();
                                        spawn_local(async move {
                                            if let Some(ref rr) = r {
                                                let _ = crate::host::open_externally(rr, &p).await;
                                            }
                                        });
                                    }
                                }
                                on:keydown=move |ev| {
                                    if ev.key() == "Escape" {
                                        ev.prevent_default();
                                        context_menu.set(None);
                                    }
                                }
                            >
                                Open Externally
                            </button>
                        </div>
                    </div>
                </Portal>
            }
            .into_any()
        }}

    }
}

// ---------------------------------------------------------------------------
// dir_row — a collapsible folder with lazy-loaded, filtered children
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn dir_row(
    entry: FsDirEntry,
    depth: usize,
    dir_cache: RwSignal<HashMap<String, FsDirsResponse>>,
    expanded: RwSignal<HashSet<String>>,
    tree_filter: RwSignal<String>,
    open_file: FileCallback,
    load_dir: DirCallback,
    expand_dir: DirCallback,
    collapse_dir: DirCallback,
    open_externally: Option<OpenExternallyCb>,
) -> impl IntoView {
    let entry_path = entry.path.clone();
    let entry_name = entry.name.clone();
    let has_git = entry.is_repo;
    let branch = entry.git_branch;

    let is_expanded = {
        let p = entry_path.clone();
        move || expanded.with(|e| e.contains(&p))
    };
    let is_expanded_arrow = is_expanded.clone();

    let branch_view: AnyView = if has_git {
        if let Some(b) = branch {
            view! { <span class="workspace-tree__branch">{b}</span> }.into_any()
        } else {
            view! { <span></span> }.into_any()
        }
    } else {
        view! { <span></span> }.into_any()
    };

    view! {
        <li class="workspace-tree-node workspace-tree-node--dir">
            <button
                class="workspace-tree-row workspace-tree-row--dir"
                type="button"
                style=format!("padding-left: {}px", depth * 14 + 8)
                on:click={
                    let p = entry_path.clone();
                    let ed = Arc::clone(&expand_dir);
                    let cd = Arc::clone(&collapse_dir);
                    move |_| {
                        if expanded.with(|e| e.contains(&p)) {
                            cd(p.clone());
                        } else {
                            ed(p.clone());
                        }
                    }
                }
            >
                <span class="workspace-tree__arrow" class:is-open=is_expanded_arrow>
                    <crate::icons::ChevronDown />
                </span>
                <span class="workspace-tree__icon">
                    {if has_git {
                        view! { <crate::icons::GitBranch /> }.into_any()
                    } else {
                        view! { <crate::icons::Folder /> }.into_any()
                    }}
                </span>
                <span class="workspace-tree__name">{entry_name}</span>
                {branch_view}
            </button>
            {move || {
                let p = entry_path.clone();
                if !is_expanded() {
                    return view! { <div></div> }.into_any();
                }
                let resp = dir_cache
                    .with(|c| c.get(&p).cloned())
                    .unwrap_or_default();
                let mut dirs = resp.dirs.clone();
                sort_entries(&mut dirs);
                let mut files = resp.files.clone();
                sort_files(&mut files);
                let filter = tree_filter.get();
                view! {
                    <ul class="workspace-tree-list">
                        {dirs
                            .into_iter()
                            .filter(|d| name_matches(&d.name, &filter))
                            .map(|d| {
                                dir_row(
                                    d,
                                    depth + 1,
                                    dir_cache,
                                    expanded,
                                    tree_filter,
                                    Arc::clone(&open_file),
                                    Arc::clone(&load_dir),
                                    Arc::clone(&expand_dir),
                                    Arc::clone(&collapse_dir),
                                    open_externally.clone(),
                                )
                            })
                            .collect::<Vec<_>>()}
                        {files
                            .into_iter()
                            .filter(|f| !is_secret_file(&f.name))
                            .filter(|f| name_matches(&f.name, &filter))
                            .map(|f| file_row(f, depth + 1, Arc::clone(&open_file), open_externally.clone()))
                            .collect::<Vec<_>>()}
                    </ul>
                }
                .into_any()
            }}
        </li>
    }
    .into_any()
}

// ---------------------------------------------------------------------------
// file_row — a single file; click opens/focuses a Preview tab
// ---------------------------------------------------------------------------

fn file_row(
    file: FsFileEntry,
    depth: usize,
    open_file: FileCallback,
    open_externally: Option<OpenExternallyCb>,
) -> impl IntoView {
    let path = file.path.clone();
    let name = file.name.clone();
    view! {
        <li class="workspace-tree-node workspace-tree-node--file">
            <button
                class="workspace-tree-row workspace-tree-row--file"
                type="button"
                style=format!("padding-left: {}px", depth * 14 + 8)
                title=path.clone()
                on:click={
                    let p = path.clone();
                    let of = Arc::clone(&open_file);
                    move |_| of(p.clone())
                }
                on:contextmenu={
                    let p = path.clone();
                    let oe = open_externally.clone();
                    move |ev| {
                        ev.prevent_default();
                        if let Some(ref cb) = oe {
                            cb(p.clone(), ev.client_x() as f64, ev.client_y() as f64);
                        }
                    }
                }
            >
                <span class="workspace-tree__icon"><crate::icons::Code /></span>
                <span class="workspace-tree__name">{name}</span>
            </button>
        </li>
    }
    .into_any()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn file_entry(name: &str) -> FsFileEntry {
        FsFileEntry {
            name: name.to_string(),
            path: format!("/root/{}", name),
            size: 0,
        }
    }

    fn tab(id: &str, kind: TabKind) -> WorkspaceTab {
        WorkspaceTab {
            id: id.into(),
            title: id.into(),
            kind,
        }
    }

    // -- workspace_focus_tab_kind (shared helper, production + tests) ---------

    #[test]
    fn focus_tab_kind_files() {
        assert_eq!(
            super::workspace_focus_tab_kind(&WorkspaceFocus::Files),
            TabKind::Files
        );
    }

    #[test]
    fn focus_tab_kind_browser() {
        assert_eq!(
            super::workspace_focus_tab_kind(&WorkspaceFocus::Browser),
            TabKind::Browser
        );
    }

    #[test]
    fn focus_tab_kind_repo() {
        assert_eq!(
            super::workspace_focus_tab_kind(&WorkspaceFocus::Repo),
            TabKind::Repo
        );
    }

    #[test]
    fn focus_tab_kind_preview_maps_path() {
        let kind = super::workspace_focus_tab_kind(&WorkspaceFocus::Preview {
            path: "/home/project/src/main.rs".into(),
            generation: 5,
        });
        assert_eq!(kind, TabKind::Preview("/home/project/src/main.rs".into()));
    }

    // The existing open_or_focus tests (below) prove that
    // TabKind::Preview(path) → open_or_focus → correct insertion/focus.
    // Together these two sets prove the full chain:
    // WorkspaceFocus::Preview{path} → TabKind::Preview(path) → open_or_focus

    #[test]
    fn name_matches_empty_filter_passes_all() {
        assert!(name_matches("main.rs", ""));
        assert!(name_matches("main.rs", "   "));
        assert!(name_matches("anything", ""));
    }

    #[test]
    fn name_matches_case_insensitive_substring() {
        assert!(name_matches("Main.RS", "main"));
        assert!(name_matches("Cargo.toml", "CARGO"));
        assert!(name_matches("src/lib.rs", "lib"));
        assert!(!name_matches("main.rs", "py"));
    }

    #[test]
    fn sort_files_alphabetical_case_insensitive() {
        let mut files = vec![
            file_entry("Zebra.rs"),
            file_entry("alpha.rs"),
            file_entry("Beta.toml"),
        ];
        sort_files(&mut files);
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["alpha.rs", "Beta.toml", "Zebra.rs"]);
    }

    #[test]
    fn format_kib_rounds_up_with_min_one_for_nonzero() {
        assert_eq!(format_kib(0), "0 KiB");
        assert_eq!(format_kib(1), "1 KiB");
        assert_eq!(format_kib(1023), "1 KiB");
        assert_eq!(format_kib(1024), "1 KiB");
        assert_eq!(format_kib(1025), "2 KiB");
        assert_eq!(format_kib(1500), "2 KiB");
        assert_eq!(format_kib(2048), "2 KiB");
    }

    // ---- open_or_focus ----

    #[test]
    fn open_or_focus_files_singleton_focuses_existing() {
        let mut tabs = vec![
            tab("files", TabKind::Files),
            tab("browser", TabKind::Browser),
        ];
        let i = open_or_focus(&mut tabs, &TabKind::Files);
        assert_eq!(i, 0);
        assert_eq!(tabs.len(), 2); // no duplicate
    }

    #[test]
    fn open_or_focus_preview_inserts_before_browser() {
        let mut tabs = vec![
            tab("files", TabKind::Files),
            tab("browser", TabKind::Browser),
        ];
        let i = open_or_focus(&mut tabs, &TabKind::Preview("/root/a.rs".into()));
        assert_eq!(i, 1); // sits between Files(0) and Browser(now 2)
        assert_eq!(tabs.len(), 3);
        assert!(matches!(&tabs[1].kind, TabKind::Preview(_)));
        assert!(matches!(&tabs[2].kind, TabKind::Browser));
        assert_eq!(tabs[1].title, "a.rs");
    }

    #[test]
    fn open_or_focus_preview_focuses_existing_by_path() {
        let mut tabs = vec![
            tab("files", TabKind::Files),
            WorkspaceTab {
                id: "preview:/root/a.rs".into(),
                title: "a.rs".into(),
                kind: TabKind::Preview("/root/a.rs".into()),
            },
            tab("browser", TabKind::Browser),
        ];
        let i = open_or_focus(&mut tabs, &TabKind::Preview("/root/a.rs".into()));
        assert_eq!(i, 1);
        assert_eq!(tabs.len(), 3); // no duplicate
    }

    #[test]
    fn open_or_focus_preview_distinct_paths_stack_in_order() {
        let mut tabs = vec![
            tab("files", TabKind::Files),
            tab("browser", TabKind::Browser),
        ];
        let _ = open_or_focus(&mut tabs, &TabKind::Preview("/root/a.rs".into()));
        let _ = open_or_focus(&mut tabs, &TabKind::Preview("/root/b.rs".into()));
        assert_eq!(tabs.len(), 4);
        assert!(matches!(&tabs[1].kind, TabKind::Preview(p) if p == "/root/a.rs"));
        assert!(matches!(&tabs[2].kind, TabKind::Preview(p) if p == "/root/b.rs"));
        assert!(matches!(&tabs[3].kind, TabKind::Browser));
    }

    // ---- close_tab ----

    #[test]
    fn close_tab_persistent_is_noop() {
        let mut tabs = vec![
            tab("files", TabKind::Files),
            tab("browser", TabKind::Browser),
        ];
        assert_eq!(close_tab(&mut tabs, 0, "files"), 0);
        assert_eq!(close_tab(&mut tabs, 0, "browser"), 0);
        assert_eq!(tabs.len(), 2);
    }

    #[test]
    fn close_tab_active_moves_to_left_neighbour() {
        let mut tabs = vec![
            tab("files", TabKind::Files),
            WorkspaceTab {
                id: "preview:/a".into(),
                title: "a".into(),
                kind: TabKind::Preview("/a".into()),
            },
            WorkspaceTab {
                id: "preview:/b".into(),
                title: "b".into(),
                kind: TabKind::Preview("/b".into()),
            },
            tab("browser", TabKind::Browser),
        ];
        // active = 2 (preview:/b); closing it lands on index 1 (preview:/a).
        let new_active = close_tab(&mut tabs, 2, "preview:/b");
        assert_eq!(new_active, 1);
        assert_eq!(tabs.len(), 3);
    }

    #[test]
    fn close_tab_leftmost_preview_clamps_to_files() {
        let mut tabs = vec![
            tab("files", TabKind::Files),
            WorkspaceTab {
                id: "preview:/a".into(),
                title: "a".into(),
                kind: TabKind::Preview("/a".into()),
            },
            tab("browser", TabKind::Browser),
        ];
        // active = 1 (the only preview); closing it lands on Files(0).
        let new_active = close_tab(&mut tabs, 1, "preview:/a");
        assert_eq!(new_active, 0);
        assert_eq!(tabs.len(), 2);
    }

    #[test]
    fn close_tab_to_the_right_of_active_is_focus_neutral() {
        let mut tabs = vec![
            tab("files", TabKind::Files),
            WorkspaceTab {
                id: "preview:/a".into(),
                title: "a".into(),
                kind: TabKind::Preview("/a".into()),
            },
            WorkspaceTab {
                id: "preview:/b".into(),
                title: "b".into(),
                kind: TabKind::Preview("/b".into()),
            },
            tab("browser", TabKind::Browser),
        ];
        // active = 1; closing index 2 (to its right) keeps active at 1.
        let new_active = close_tab(&mut tabs, 1, "preview:/b");
        assert_eq!(new_active, 1);
        assert_eq!(tabs.len(), 3);
    }

    #[test]
    fn close_tab_unknown_id_is_noop() {
        let mut tabs = vec![tab("files", TabKind::Files)];
        assert_eq!(close_tab(&mut tabs, 0, "nope"), 0);
        assert_eq!(tabs.len(), 1);
    }

    #[test]
    fn close_tab_to_left_of_active_shifts_active_left() {
        let mut tabs = vec![
            tab("files", TabKind::Files),
            WorkspaceTab {
                id: "preview:/a".into(),
                title: "a".into(),
                kind: TabKind::Preview("/a".into()),
            },
            WorkspaceTab {
                id: "preview:/b".into(),
                title: "b".into(),
                kind: TabKind::Preview("/b".into()),
            },
            tab("browser", TabKind::Browser),
        ];
        // active = 2 (preview:/b); closing index 1 (to its left) shifts active to 1.
        let new_active = close_tab(&mut tabs, 2, "preview:/a");
        assert_eq!(new_active, 1);
        assert_eq!(tabs.len(), 3);
    }

    // ---- Repo tab (persistent, last position) + focus mapping ----

    #[test]
    fn open_or_focus_repo_present_focuses_existing() {
        let mut tabs = vec![
            tab("files", TabKind::Files),
            tab("browser", TabKind::Browser),
            tab("repo", TabKind::Repo),
        ];
        let i = open_or_focus(&mut tabs, &TabKind::Repo);
        assert_eq!(i, 2);
        assert_eq!(tabs.len(), 3); // no duplicate
    }

    #[test]
    fn open_or_focus_preview_inserts_before_browser_keeps_repo_last() {
        let mut tabs = vec![
            tab("files", TabKind::Files),
            tab("browser", TabKind::Browser),
            tab("repo", TabKind::Repo),
        ];
        let i = open_or_focus(&mut tabs, &TabKind::Preview("/root/a.rs".into()));
        // Files(0) · preview(1) · Browser(2) · Repo(3) — Repo stays last.
        assert_eq!(i, 1);
        assert_eq!(tabs.len(), 4);
        assert!(matches!(&tabs[1].kind, TabKind::Preview(_)));
        assert!(matches!(&tabs[2].kind, TabKind::Browser));
        assert!(matches!(&tabs[3].kind, TabKind::Repo));
    }

    #[test]
    fn open_or_focus_focus_each_persistent_kind_no_duplicates() {
        // Mirrors the focus_intent Effect's WorkspaceFocus -> TabKind mapping:
        // focusing each persistent surface lands on its tab without dupes.
        let mut tabs = vec![
            tab("files", TabKind::Files),
            tab("browser", TabKind::Browser),
            tab("repo", TabKind::Repo),
        ];
        assert_eq!(open_or_focus(&mut tabs, &TabKind::Files), 0);
        assert_eq!(open_or_focus(&mut tabs, &TabKind::Browser), 1);
        assert_eq!(open_or_focus(&mut tabs, &TabKind::Repo), 2);
        assert_eq!(tabs.len(), 3);
        assert_eq!(tabs[0].id, "files");
        assert_eq!(tabs[1].id, "browser");
        assert_eq!(tabs[2].id, "repo");
    }

    #[test]
    fn close_tab_repo_is_noop() {
        let mut tabs = vec![
            tab("files", TabKind::Files),
            tab("browser", TabKind::Browser),
            tab("repo", TabKind::Repo),
        ];
        // Repo is persistent: closing it from any active index is a no-op.
        assert_eq!(close_tab(&mut tabs, 2, "repo"), 2);
        assert_eq!(close_tab(&mut tabs, 0, "repo"), 0);
        assert_eq!(tabs.len(), 3);
        // The close button never renders for it (only Preview is closable),
        // but close_tab must defend regardless.
        assert!(matches!(&tabs[2].kind, TabKind::Repo));
    }

    // ---- effective_width (pure clamp math) ----

    #[test]
    fn effective_width_split_column_floor() {
        // vw=1000, desired=650: split mode, the content column keeps 480, so
        // the pane parks at vw − 480 = 520 (under the 65% = 650 cap anyway).
        assert_eq!(effective_width(650.0, 1000.0), 520.0);
    }

    #[test]
    fn effective_width_split_65pct_cap() {
        // vw=2000, desired=1400: 65% of 2000 = 1300 caps below the
        // vw − 480 = 1520 column floor, so the pane stops at 1300.
        assert_eq!(effective_width(1400.0, 2000.0), 1300.0);
    }

    #[test]
    fn effective_width_min_floor_wins() {
        // vw=1200, desired=100: MIN_PANE_W (320) wins over the tiny desired.
        assert_eq!(effective_width(100.0, 1200.0), 320.0);
    }

    #[test]
    fn effective_width_overlay_keeps_context_sliver() {
        // vw=800 (< 900): overlay mode covers all but a 48px sliver → 752.
        assert_eq!(effective_width(900.0, 800.0), 752.0);
    }

    #[test]
    fn effective_width_degenerate_window_clamps_to_min() {
        // vw=350 (< 900): overlay max would be 302, but MIN_PANE_W (320) wins
        // — a degenerate window accepts a pane wider than its viewport.
        assert_eq!(effective_width(500.0, 350.0), 320.0);
    }

    #[test]
    fn effective_width_boundary_at_split_bp_is_split() {
        // vw=900 exactly is split (>=), so the pane max is the column floor
        // vw − 480 = 420, not the overlay 852. A large desired parks at 420.
        assert_eq!(effective_width(900.0, 900.0), 420.0);
    }

    // ---- tree_auto_collapses ----

    #[test]
    fn tree_never_auto_collapses_on_files_or_repo() {
        // Files: the tree IS the tab body — collapsing it would leave a dead
        // pane. Repo: compact list, coexists with the tree at any width.
        assert!(!tree_auto_collapses(&TabKind::Files, 320.0));
        assert!(!tree_auto_collapses(&TabKind::Repo, 320.0));
    }

    #[test]
    fn tree_auto_collapses_wide_tabs_only_below_narrow() {
        let preview = TabKind::Preview("/root/a.rs".into());
        assert!(tree_auto_collapses(&preview, NARROW_PANE_W - 1.0));
        assert!(tree_auto_collapses(&TabKind::Browser, NARROW_PANE_W - 1.0));
        // At and above the threshold the tree stays docked.
        assert!(!tree_auto_collapses(&preview, NARROW_PANE_W));
        assert!(!tree_auto_collapses(
            &TabKind::Browser,
            NARROW_PANE_W + 40.0
        ));
    }

    // ---- Preview lifecycle (TASK-27): session/root-bound clearing ----

    fn preview_tab(path: &str) -> WorkspaceTab {
        WorkspaceTab {
            id: format!("preview:{}", path),
            title: basename(path).to_string(),
            kind: TabKind::Preview(path.into()),
        }
    }

    #[test]
    fn clear_preview_tabs_removes_previews_keeps_persistent_in_order() {
        // Files · a.rs · b.rs · Browser · Repo → previews gone, order intact.
        let mut tabs = vec![
            tab("files", TabKind::Files),
            preview_tab("/root/a.rs"),
            preview_tab("/root/b.rs"),
            tab("browser", TabKind::Browser),
            tab("repo", TabKind::Repo),
        ];
        let _ = clear_preview_tabs(&mut tabs, 0);
        let ids: Vec<&str> = tabs.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["files", "browser", "repo"]);
    }

    #[test]
    fn clear_preview_tabs_active_on_preview_lands_on_files() {
        // A session switch while a Preview tab is focused must land on the
        // safe first persistent tab (Files), never a stale/out-of-range index.
        let mut tabs = vec![
            tab("files", TabKind::Files),
            preview_tab("/root/a.rs"),
            tab("browser", TabKind::Browser),
            tab("repo", TabKind::Repo),
        ];
        let new_active = clear_preview_tabs(&mut tabs, 1);
        assert_eq!(new_active, 0);
        assert!(matches!(tabs[0].kind, TabKind::Files));
    }

    #[test]
    fn clear_preview_tabs_active_on_persistent_follows_it() {
        // Focus on Browser (index 3) survives the wipe: Browser is now index 1.
        let mut tabs = vec![
            tab("files", TabKind::Files),
            preview_tab("/root/a.rs"),
            preview_tab("/root/b.rs"),
            tab("browser", TabKind::Browser),
            tab("repo", TabKind::Repo),
        ];
        let new_active = clear_preview_tabs(&mut tabs, 3);
        assert_eq!(new_active, 1);
        assert_eq!(tabs[new_active].id, "browser");
    }

    #[test]
    fn read_is_current_admits_only_matching_generation() {
        // A completion issued under generation 3 is admitted only while the
        // live generation is still 3; any later generation retires it.
        assert!(read_is_current(3, 3));
        assert!(!read_is_current(3, 4));
        assert!(!read_is_current(0, 1));
    }

    #[test]
    fn preview_gen_retires_on_session_change() {
        // Same root, different session → retire (clear previews).
        let root = Some("/home/project".to_string());
        assert!(preview_gen_should_retire(
            &Some("sess-a".into()),
            &Some("sess-b".into()),
            &root,
            &root,
        ));
    }

    #[test]
    fn preview_gen_retires_on_root_change() {
        // Same session, different browsable root → retire.
        let session = Some("sess-a".to_string());
        assert!(preview_gen_should_retire(
            &session,
            &session,
            &Some("/home/project-one".into()),
            &Some("/home/project-two".into()),
        ));
        // A drop to a non-browsable root (None, e.g. /tmp reset) also retires.
        assert!(preview_gen_should_retire(
            &session,
            &session,
            &Some("/home/project-one".into()),
            &None,
        ));
    }

    #[test]
    fn preview_gen_stable_session_and_root_does_not_retire() {
        // Same session, same root (a mere tree refresh) must NOT clear previews.
        let session = Some("sess-a".to_string());
        let root = Some("/home/project".to_string());
        assert!(!preview_gen_should_retire(&session, &session, &root, &root));
    }
}
