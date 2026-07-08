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

use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::daemon::{
    fetch_fs_dirs_with_files, fetch_fs_file, Daemon, FsDirEntry, FsDirsResponse, FsFileEntry,
    FsFileResponse,
};
// Reuse the deck files panel's shared helpers rather than forking the
// sorting / refresh / basename logic — single source of truth for the
// session-cwd tree contract.
use crate::deck::files::{basename, refresh_target, sort_entries};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Which kind of content a workspace tab shows.
#[derive(Clone, Debug, PartialEq)]
pub enum TabKind {
    /// Persistent empty-state tab — prompts the user to pick a file.
    Files,
    /// A preview of one file, keyed by its absolute path. Closable.
    Preview(String),
    /// Persistent stub tab — renders the slot a sibling agent fills.
    Browser,
}

/// One tab in the workspace strip. `id` is stable (`files` / `browser` /
/// `preview:<path>`) so the strip can key on it.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceTab {
    pub id: String,
    pub title: String,
    pub kind: TabKind,
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
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
}

/// Render a byte size as whole KiB (round up so a 1-byte file reads "1 KiB",
/// never "0 KiB"; 0 bytes reads "0 KiB").
pub(crate) fn format_kib(size: u64) -> String {
    format!("{} KiB", (size + 1023) / 1024)
}

/// Ensure a tab for `kind` exists in `tabs` and return its index, making it
/// the active tab. `Files` and `Browser` are singletons (focus existing);
/// `Preview(path)` is keyed by path and inserted before the persistent
/// `Browser` tab so the strip keeps its Files · previews · Browser order.
pub(crate) fn open_or_focus(tabs: &mut Vec<WorkspaceTab>, kind: &TabKind) -> usize {
    match kind {
        TabKind::Files => focus_or_push(tabs, kind, "files", "Files"),
        TabKind::Browser => focus_or_push(tabs, kind, "browser", "Browser"),
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
/// persistent `Files`/`Browser` tab is a no-op. Returns the new active index:
/// closing the active tab moves focus to its left neighbour (clamped); closing
/// a tab to the right of active is focus-neutral.
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
        if i == 0 { 0 } else { i - 1 }
    } else {
        active
    };
    new_active.min(tabs.len() - 1)
}

// ---------------------------------------------------------------------------
// Shared callback types
// ---------------------------------------------------------------------------

type DirCallback = Arc<dyn Fn(String) + Send + Sync>;
type FileCallback = Arc<dyn Fn(String) + Send + Sync>;

// ---------------------------------------------------------------------------
// WorkspacePane component
// ---------------------------------------------------------------------------

/// The permanent right-side desktop pane. `open` is the shared collapse state
/// owned by the app shell (header toggle + ⌘K command flip the same signal);
/// the pane renders `is-collapsed` from it and the shell adjusts its layout
/// gutter to match.
#[component]
pub fn WorkspacePane(daemon: Daemon, open: RwSignal<bool>) -> impl IntoView {
    // Root = session cwd, the same source the deck files panel reads. An
    // empty / "/" cwd leaves the tree empty until a session lands.
    let tree_root: RwSignal<Option<String>> = {
        let cwd = daemon.cwd.get_untracked();
        if cwd.is_empty() || cwd == "/" {
            RwSignal::new(None)
        } else {
            RwSignal::new(Some(cwd))
        }
    };
    let daemon_url = daemon.url;

    // Tabs: Files and Browser are persistent; Preview tabs come and go.
    let tabs: RwSignal<Vec<WorkspaceTab>> = RwSignal::new(vec![
        WorkspaceTab { id: "files".into(), title: "Files".into(), kind: TabKind::Files },
        WorkspaceTab { id: "browser".into(), title: "Browser".into(), kind: TabKind::Browser },
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

    // ---- Tree load / expand / collapse (mirror deck::files::FilesPanel) ----

    let load_dir: DirCallback = {
        let daemon_url = daemon_url;
        let dir_cache = dir_cache;
        let loading_path = loading_path;
        let load_error = load_error;
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
        let dir_cache = dir_cache;
        let expanded = expanded;
        let loading_path = loading_path;
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
        let expanded = expanded;
        Arc::new(move |path: String| {
            expanded.update(|e| {
                e.remove(&path);
            });
        })
    };

    // ---- File content fetch (shared by open + watcher refetch) ----

    let fetch_file_content: FileCallback = {
        let daemon_url = daemon_url;
        let preview_cache = preview_cache;
        let preview_loading = preview_loading;
        let preview_error = preview_error;
        Arc::new(move |path: String| {
            let url = daemon_url.get_untracked();
            preview_loading.set(Some(path.clone()));
            preview_error.set(None);
            spawn_local(async move {
                if let Some(resp) = fetch_fs_file(&url, &path).await {
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
        let tabs = tabs;
        let active_tab = active_tab;
        let preview_cache = preview_cache;
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
        let tree_root = tree_root;
        let load_dir = Arc::clone(&load_dir);
        let dir_cache = dir_cache;
        let expanded = expanded;
        let preview_cache = preview_cache;
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
        Effect::new(move |_| {
            let cwd = cwd_sig.get();
            if cwd.is_empty() || cwd == "/" {
                return;
            }
            if tree_root.get_untracked().as_deref() != Some(cwd.as_str()) {
                tree_root.set(Some(cwd.clone()));
            }
            if !dir_cache.with_untracked(|c| c.contains_key(&cwd)) {
                load_dir(cwd);
            }
        });
    }

    // Pre-clone for the view's render closures.
    let ld_tree = Arc::clone(&load_dir);
    let ed_tree = Arc::clone(&expand_dir);
    let cd_tree = Arc::clone(&collapse_dir);
    let of_tree = Arc::clone(&open_file);
    // Hoisted out of `view!`: a turbofish (`collect::<Vec<_>>()`) inside an
    // RSX attribute position parses `<Vec<_>>` as a tag opener.
    let tab_entries = move || tabs.get().into_iter().enumerate().collect::<Vec<_>>();
    // Dedicated clone for the tab-content closure below (FnMut, re-runs):
    // the Browser tab renders the sibling module's live screencast component.
    let daemon_browser = daemon.clone();

    view! {
        <aside
            class="workspace"
            class:is-collapsed=move || !open.get()
            role="complementary"
            aria-label="Workspace"
        >
            // ---- Tab strip ----
            <div class="workspace-tabs">
                <For
                    each=tab_entries
                    key=|(_, t)| t.id.clone()
                    children=move |(index, tab)| {
                        let tab_id = tab.id.clone();
                        let tab_title = tab.title.clone();
                        let closable = matches!(tab.kind, TabKind::Preview(_));
                        view! {
                            <button
                                class="workspace-tab"
                                class:is-active=move || active_tab.get() == index
                                type="button"
                                title=tab_title.clone()
                                on:click=move |_| active_tab.set(index)
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
                                        >"✕"</span>
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
                        TabKind::Files => view! {
                            <div class="workspace-preview">
                                <div class="workspace-empty">
                                    "Open file — select a file from the tree"
                                </div>
                            </div>
                        }
                        .into_any(),
                        TabKind::Browser => view! {
                            <div class="workspace-browser">
                                // Live agent-browser view: CDP screencast of the
                                // daemon-owned Chrome + input forwarding
                                // (workspace_browser.rs).
                                <crate::workspace_browser::WorkspaceBrowser daemon=daemon_browser.clone() />
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
                <aside class="workspace-tree" aria-label="Files">
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
                                                    )
                                                })
                                                .collect::<Vec<_>>()}
                                            {files
                                                .into_iter()
                                                .filter(|f| name_matches(&f.name, &filter))
                                                .map(|f| file_row(f, 1, Arc::clone(&of_tree)))
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
                                )
                            })
                            .collect::<Vec<_>>()}
                        {files
                            .into_iter()
                            .filter(|f| name_matches(&f.name, &filter))
                            .map(|f| file_row(f, depth + 1, Arc::clone(&open_file)))
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

fn file_row(file: FsFileEntry, depth: usize, open_file: FileCallback) -> impl IntoView {
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
}
