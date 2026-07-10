//! Files panel — persistent explorer for the session cwd.
//! Owned by the FilesPanel workstream (feat/desktop-files-panel).
//! Native root pick via crate::host::pick_folder, live updates via
//! crate::host::{watch_paths, on_path_changed}. Browser host: read-only,
//! picker and watcher hidden.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::daemon::{fetch_fs_dirs, Daemon, FsDirEntry};

// ---------------------------------------------------------------------------
// Pure helpers — unit-testable without WASM
// ---------------------------------------------------------------------------

/// Sort entries: directories first (git repos sort above non-git dirs),
/// then alphabetically within each group.
pub(crate) fn sort_entries(entries: &mut [FsDirEntry]) {
    entries.sort_by(|a, b| {
        // Repos surface first; the daemon populates `is_repo` (`git` is a
        // legacy field it never sends).
        let a_repo = a.is_repo;
        let b_repo = b.is_repo;
        b_repo.cmp(&a_repo).then_with(|| a.name.cmp(&b.name))
    });
}

/// Given a PathEvent path, return Some(parent_dir) for a re-list target.
pub(crate) fn refresh_target(root: &str, event_path: &str, _kind: &str) -> Option<String> {
    let root = root.trim_end_matches('/');
    let event_path = event_path.trim_end_matches('/');
    if event_path == root {
        return None;
    }
    if let Some(slash) = event_path.rfind('/') {
        let parent = &event_path[..slash];
        if parent == root || parent.starts_with(root) {
            return Some(parent.to_string());
        }
    }
    None
}

/// Extract basename from a path.
pub(crate) fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

// ---------------------------------------------------------------------------
// Shared callback type
// ---------------------------------------------------------------------------

type DirCallback = Arc<dyn Fn(String) + Send + Sync>;

// ---------------------------------------------------------------------------
// FilesPanel component
// ---------------------------------------------------------------------------

#[component]
pub fn FilesPanel(daemon: Daemon) -> impl IntoView {
    let panel_root: RwSignal<Option<String>> = {
        let cwd = daemon.cwd.get_untracked();
        if cwd.is_empty() || cwd == "/" {
            RwSignal::new(None)
        } else {
            RwSignal::new(Some(cwd))
        }
    };
    let daemon_url = daemon.url;

    let dir_cache: RwSignal<HashMap<String, Vec<FsDirEntry>>> = RwSignal::new(HashMap::new());
    let loading_path: RwSignal<Option<String>> = RwSignal::new(None);
    let load_error: RwSignal<Option<String>> = RwSignal::new(None);
    let expanded: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let watcher_active: RwSignal<bool> = RwSignal::new(false);

    // ---- Shared closures (Arc for Send+Sync, clone before capturing) ----

    let load_dir: DirCallback = {
        Arc::new(move |path: String| {
            let url = daemon_url.get_untracked();
            loading_path.set(Some(path.clone()));
            load_error.set(None);
            spawn_local(async move {
                if let Some(resp) = fetch_fs_dirs(&url, &path).await {
                    // The daemon's success responses carry no `ok` field
                    // (serde default = false) — failure is signalled by
                    // `error`, so THAT is the success predicate.
                    if resp.error.is_none() {
                        let mut entries = resp.dirs;
                        sort_entries(&mut entries);
                        dir_cache.update(|cache| {
                            cache.insert(path, entries);
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

    // ---- choose_folder (Tauri-only action) ----
    let choose_folder = {
        let load_dir = Arc::clone(&load_dir);

        move || {
            let load_dir = Arc::clone(&load_dir);
            spawn_local(async move {
                if let Some(folder) = crate::host::pick_folder().await {
                    panel_root.set(Some(folder.clone()));
                    load_dir(folder.clone());
                    if crate::host::running_in_tauri() {
                        let paths: Vec<String> = vec![folder];
                        let _ = crate::host::watch_paths(&paths).await;
                        watcher_active.set(true);
                    }
                }
            });
        }
    };

    // ---- Watcher setup ----
    {
        let load_dir = Arc::clone(&load_dir);

        crate::host::on_path_changed(move |ev| {
            let Some(root) = panel_root.get() else { return };
            let target = refresh_target(&root, &ev.path, &ev.kind);
            match target {
                Some(dir) => load_dir(dir),
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
        });
    }

    // ---- Initial load + session-root follow ----
    // The daemon session cwd is the default panel root: list it on mount and
    // re-point the panel when the session root changes. Cache hits skip the
    // refetch so palette toggles don't re-list on every reveal.
    {
        let load_dir = Arc::clone(&load_dir);
        let cwd_sig = daemon.cwd;
        Effect::new(move |_| {
            let cwd = cwd_sig.get();
            if cwd.is_empty() || cwd == "/" {
                return;
            }
            if panel_root.get_untracked().as_deref() != Some(cwd.as_str()) {
                panel_root.set(Some(cwd.clone()));
            }
            if !dir_cache.with_untracked(|c| c.contains_key(&cwd)) {
                load_dir(cwd);
            }
        });
    }

    // ---- Cleanup ----
    let cleanup_watcher = {
        move || {
            if watcher_active.get() {
                if let Some(root) = panel_root.get_untracked() {
                    let paths: Vec<String> = vec![root];
                    spawn_local(async move {
                        let _ = crate::host::unwatch_paths(&paths).await;
                    });
                }
                watcher_active.set(false);
            }
        }
    };

    // Pre-clone for the view's render closure.
    let ld_header = Arc::clone(&load_dir);
    let ld_tree = Arc::clone(&load_dir);
    let ed_tree = Arc::clone(&expand_dir);
    let cd_tree = Arc::clone(&collapse_dir);

    view! {
        <div class="deck-files-panel">
            // ---- Header ----
            <div class="deck-files-header">
                {{
                    let ld = Arc::clone(&ld_header);
                    let cf = choose_folder;
                    move || match panel_root.get() {
                        Some(root) => view! {
                            <div class="deck-files-breadcrumb">
                                <span class="deck-files-root-label" title=root.clone()>
                                    {basename(&root)}
                                </span>
                                <span class="deck-files-root-path">{root.clone()}</span>
                            </div>
                            <div class="deck-files-actions">
                                <button
                                    class="deck-files-btn deck-files-btn--refresh"
                                    type="button"
                                    on:click={
                                        let ld = Arc::clone(&ld);
                                        move |_| {
                                            if let Some(r) = panel_root.get() {
                                                ld(r);
                                            }
                                        }
                                    }
                                    title="Refresh"
                                >
                                    "↻"
                                </button>
                            </div>
                        }.into_any(),
                        None => {
                            if crate::host::running_in_tauri() {
                                view! {
                                    <div class="deck-files-empty">
                                        <p class="deck-files-empty__text">
                                            "No folder selected. Choose a project root to explore."
                                        </p>
                                        <button
                                            class="deck-files-btn deck-files-btn--primary"
                                            type="button"
                                            on:click={
                                                let cf = cf.clone();
                                                move |_| cf()
                                            }
                                        >
                                            "Choose folder…"
                                        </button>
                                    </div>
                                }.into_any()
                            } else {
                                view! {
                                    <div class="deck-files-empty">
                                        <p class="deck-files-empty__text">
                                            "No active session or the session has no working directory."
                                        </p>
                                    </div>
                                }.into_any()
                            }
                        }
                    }
                }}
            </div>

            // ---- Loading ----
            {move || loading_path.get().map(|path| {
                view! {
                    <div class="deck-files-loading">
                        {format!("Loading {}…", basename(&path))}
                    </div>
                }.into_any()
            })}

            // ---- Error ----
            {move || load_error.get().map(|err| {
                view! {
                    <div class="deck-files-error">{err}</div>
                }.into_any()
            })}

            // ---- File tree ----
            <div class="deck-files-tree">
                {{
                    let ld = Arc::clone(&ld_tree);
                    let ed = Arc::clone(&ed_tree);
                    let cd = Arc::clone(&cd_tree);
                    move || match panel_root.get() {
                        Some(ref root_path) => {
                            let entries = dir_cache.with(|c| c.get(root_path).cloned());
                            match entries {
                                Some(entries) => view! {
                                    <ul class="deck-files-list">
                                        {entries.into_iter().map(|entry| {
                                            dir_row(
                                                entry,
                                                1,
                                                dir_cache,
                                                expanded,
                                                Arc::clone(&ld),
                                                Arc::clone(&ed),
                                                Arc::clone(&cd),
                                            )
                                        }).collect::<Vec<_>>()}
                                    </ul>
                                }.into_any(),
                                None => view! { <div></div> }.into_any(),
                            }
                        }
                        None => view! { <div></div> }.into_any(),
                    }
                }}
            </div>

            <span style="display:none">{move || { let _ = &cleanup_watcher; }}</span>
        </div>
    }
}

// ---------------------------------------------------------------------------
// dir_row — renders a single directory entry with collapsible children
// ---------------------------------------------------------------------------

fn dir_row(
    entry: FsDirEntry,
    depth: usize,
    dir_cache: RwSignal<HashMap<String, Vec<FsDirEntry>>>,
    expanded: RwSignal<HashSet<String>>,
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
            view! { <span class="deck-files-branch">{b}</span> }.into_any()
        } else {
            view! { <span></span> }.into_any()
        }
    } else {
        view! { <span></span> }.into_any()
    };

    view! {
        <li class="deck-files-node deck-files-node--dir">
            <button
                class="deck-files-row deck-files-row--dir"
                type="button"
                style=format!("padding-left: {}px", depth * 16 + 8)
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
                <span class="deck-files-arrow" class:is-open=is_expanded_arrow>
                    <crate::icons::ChevronDown />
                </span>
                <span class="deck-files-icon">
                    {if has_git {
                        view! { <crate::icons::GitBranch /> }.into_any()
                    } else {
                        view! { <crate::icons::Folder /> }.into_any()
                    }}
                </span>
                <span class="deck-files-name">{entry_name}</span>
                {branch_view}
            </button>
            {move || if is_expanded() {
                let p = entry_path.clone();
                let entries = dir_cache.with(|c| c.get(&p).cloned()).unwrap_or_default();
                view! {
                    <ul class="deck-files-list">
                        {entries.into_iter().map(|child| {
                            dir_row(
                                child,
                                depth + 1,
                                dir_cache,
                                expanded,
                                Arc::clone(&load_dir),
                                Arc::clone(&expand_dir),
                                Arc::clone(&collapse_dir),
                            )
                        }).collect::<Vec<_>>()}
                    </ul>
                }.into_any()
            } else {
                view! { <div></div> }.into_any()
            }}
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

    fn entry(name: &str, git: bool, is_repo: bool) -> FsDirEntry {
        FsDirEntry {
            name: name.to_string(),
            path: format!("/root/{}", name),
            git,
            is_repo,
            git_branch: if git { Some("main".into()) } else { None },
        }
    }

    #[test]
    fn sort_puts_git_repos_first() {
        let mut entries = vec![
            entry("zzz", false, false),
            entry("aaa", false, false),
            entry("bbb", true, true),
            entry("ccc", true, true),
        ];
        sort_entries(&mut entries);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["bbb", "ccc", "aaa", "zzz"]);
    }

    #[test]
    fn sort_stable_for_same_type() {
        let mut entries = vec![
            entry("delta", false, false),
            entry("alpha", false, false),
            entry("charlie", true, true),
            entry("bravo", true, true),
        ];
        sort_entries(&mut entries);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["bravo", "charlie", "alpha", "delta"]);
    }

    #[test]
    fn refresh_target_root_event_returns_none() {
        let root = "/home/user/project";
        assert_eq!(refresh_target(root, root, "modified"), None);
    }

    #[test]
    fn refresh_target_file_in_subdir() {
        let root = "/home/user/project";
        assert_eq!(
            refresh_target(root, "/home/user/project/src/main.rs", "modified"),
            Some("/home/user/project/src".into())
        );
    }

    #[test]
    fn refresh_target_dir_event() {
        let root = "/home/user/project";
        assert_eq!(
            refresh_target(root, "/home/user/project/src/components", "created"),
            Some("/home/user/project/src".into())
        );
    }

    #[test]
    fn refresh_target_nested() {
        let root = "/root";
        assert_eq!(
            refresh_target(root, "/root/a/b/c/d/file.txt", "modified"),
            Some("/root/a/b/c/d".into())
        );
    }

    #[test]
    fn basename_simple() {
        assert_eq!(basename("/foo/bar/baz.txt"), "baz.txt");
        assert_eq!(basename("just_a_file"), "just_a_file");
        assert_eq!(basename("/root/"), "");
    }
}
