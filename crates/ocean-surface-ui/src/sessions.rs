//! Sessions panel — grouped by project, worktrees, with collapsible sections.
//!
//! Sessions are grouped under their owning project: authoritatively via the
//! daemon's `owning_project` when it serves that field (once the enriched DTO
//! lands), or by exact-matching a session's workspace root to a project in the
//! catalogue (`daemon.projects`). Sessions with no matching project fall into
//! an "Other" bucket.
//!
//! Within a project, sessions that span multiple workspace roots are split into
//! worktree sub-groups (keyed by root, not branch — branch is just a display
//! chip to handle detached HEAD / duplicate branch names safely). A project
//! with a single root renders flat.
//!
//! Zero-turn drafts are filtered out unless they're the active session (the
//! lazy session creation approach no longer POSTs on "New Session" click, so
//! empty sessions no longer accumulate in the store; historical litter is
//! pruned from the display).

use std::collections::HashSet;
use leptos::prelude::*;
use js_sys;

use crate::daemon::{Daemon, ProjectInfo, SessionSummary};

// ---------------------------------------------------------------------------
// Pure helpers — unit-testable without WASM
// ---------------------------------------------------------------------------

/// The workspace root for a session: its own `workspace_root` if present,
/// otherwise the reported `cwd` (which the daemon fills from the stored root).
pub(crate) fn session_root(s: &SessionSummary) -> &str {
    s.workspace_root
        .as_deref()
        .filter(|r| !r.is_empty())
        .unwrap_or(s.cwd.as_str())
}

/// One project section in the panel: a project header followed by its sessions,
/// optionally split into worktree sub-groups when the project spans multiple
/// workspace roots.
#[derive(Clone, Debug)]
pub(crate) struct ProjectSection {
    pub key: String,
    pub label: String,
    pub is_project: bool,
    /// Flat session list. Populated only when `worktrees` is None.
    pub sessions: Vec<SessionSummary>,
    /// Sub-groups by workspace root, set only when >1 distinct root exists.
    pub worktrees: Option<Vec<WorktreeGroup>>,
}

/// A worktree section inside a project: sessions sharing one workspace root.
#[derive(Clone, Debug)]
pub(crate) struct WorktreeGroup {
    pub root: String,
    pub branch: Option<String>,
    pub sessions: Vec<SessionSummary>,
}

/// Group + filter sessions for the panel display: project-first, newest
/// sessions first, "Other" last. Zero-turn drafts filtered unless they are
/// the active session. Pure — no WASM, testable with `cargo test`.
pub(crate) fn group_for_panel(
    sessions: &[SessionSummary],
    projects: &[ProjectInfo],
    active_id: Option<&str>,
) -> Vec<ProjectSection> {
    let active_id = active_id.map(|s| s.to_string());

    // Prune: skip 0-turn drafts except the active session itself.
    let filtered: Vec<&SessionSummary> = sessions
        .iter()
        .filter(|s| s.turn_count > 0 || active_id.as_deref() == Some(s.id.as_str()))
        .collect();

    // Collect into sections by project membership.
    let mut sections: Vec<ProjectSection> = Vec::new();
    for s in &filtered {
        let (key, label, is_proj) = if let Some(op) = &s.owning_project {
            let label = if op.name.trim().is_empty() {
                op.id.clone()
            } else {
                op.name.clone()
            };
            (op.id.clone(), label, true)
        } else if let Some(p) = projects.iter().find(|p| {
            !p.workspace_root.is_empty() && p.workspace_root == session_root(s)
        }) {
            let label = if p.name.trim().is_empty() {
                p.workspace_root.clone()
            } else {
                p.name.clone()
            };
            (p.id.clone(), label, true)
        } else {
            ("__other__".to_string(), "Other".to_string(), false)
        };

        match sections.iter_mut().find(|sec: &&mut ProjectSection| sec.key == key) {
            Some(sec) => sec.sessions.push((*s).clone()),
            None => sections.push(ProjectSection {
                key,
                label,
                is_project: is_proj,
                sessions: vec![(*s).clone()],
                worktrees: None,
            }),
        }
    }

    // Sort sessions inside each group: newest first (ISO-8601 lexicographic cmp).
    for sec in &mut sections {
        sec.sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    }

    // Split into worktree sub-groups where a project has >1 distinct root.
    for sec in &mut sections {
        if !sec.is_project {
            continue;
        }
        let mut roots: Vec<WorktreeGroup> = Vec::new();
        for s in &sec.sessions {
            let root = session_root(s).to_string();
            let branch = s.git_branch.clone();
            match roots.iter_mut().find(|wt: &&mut WorktreeGroup| wt.root == root) {
                Some(wt) => wt.sessions.push(s.clone()),
                None => roots.push(WorktreeGroup {
                    root,
                    branch,
                    sessions: vec![s.clone()],
                }),
            }
        }
        if roots.len() > 1 {
            for wt in &mut roots {
                wt.sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            }
            roots.sort_by(|a, b| {
                let a_has = a.branch.is_some();
                let b_has = b.branch.is_some();
                match (a_has, b_has) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => b
                        .sessions
                        .first()
                        .map(|s| s.updated_at.as_str())
                        .unwrap_or("")
                        .cmp(a.sessions.first().map(|s| s.updated_at.as_str()).unwrap_or("")),
                }
            });
            sec.worktrees = Some(roots);
        }
    }

    // Sort sections: project groups by newest session; "Other" always last.
    sections.sort_by(|a, b| match (a.key == "__other__", b.key == "__other__") {
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        _ => b
            .sessions
            .first()
            .map(|s| s.updated_at.as_str())
            .unwrap_or("")
            .cmp(a.sessions.first().map(|s| s.updated_at.as_str()).unwrap_or("")),
    });

    sections
}

/// First letter, uppercased, for a project monogram badge.
pub(crate) fn monogram(name: &str) -> String {
    name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into())
}

/// Relative time from an ISO-8601 timestamp: "2h ago", "5d ago", etc.
/// Uses `js_sys::Date` for the browser's local time. Falls back to the raw
/// date text when parsing fails.
pub(crate) fn fmt_relative_time(updated_at: &str) -> String {
    let ts = js_sys::Date::parse(updated_at);
    if ts.is_nan() {
        return updated_at[..updated_at.len().min(10)].to_string();
    }
    let diff_s = (js_sys::Date::now() - ts) / 1000.0;
    if diff_s < 60.0 {
        "just now".into()
    } else if diff_s < 3600.0 {
        format!("{}m ago", (diff_s / 60.0) as u32)
    } else if diff_s < 86400.0 {
        format!("{}h ago", (diff_s / 3600.0) as u32)
    } else if diff_s < 604800.0 {
        format!("{}d ago", (diff_s / 86400.0) as u32)
    } else {
        updated_at[..10].to_string()
    }
}

// ---------------------------------------------------------------------------
// SessionsPanel component
// ---------------------------------------------------------------------------

/// Sessions panel that slides in from the right when open.
#[component]
pub fn SessionsPanel(daemon: Daemon, open: RwSignal<bool>) -> impl IntoView {
    let session_list = daemon.session_list;
    let projects = daemon.projects;
    let current_id = daemon.session_id;
    let daemon = StoredValue::new(daemon);

    // Fetch sessions whenever the panel opens.
    Effect::new(move |_| {
        if open.get() {
            daemon.get_value().fetch_sessions();
        }
    });

    // Derived: grouped sections (recomputes on session_list or projects change).
    let sections = move || {
        group_for_panel(
            &session_list.get(),
            &projects.get(),
            current_id.get().as_deref(),
        )
    };

    // Collapse state: default to one open section (active project, else first
    // section) as soon as panel data materializes. User toggles win afterward.
    let collapsed: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let collapse_primed: RwSignal<bool> = RwSignal::new(false);
    Effect::new(move |_| {
        if !open.get() || collapse_primed.get_untracked() {
            return;
        }

        let secs = group_for_panel(
            &session_list.get(),
            &projects.get(),
            current_id.get().as_deref(),
        );
        if secs.is_empty() {
            return;
        }

        let active_id = current_id.get_untracked();
        let mut expanded_keys: Vec<String> = secs
            .iter()
            .filter(|sec| {
                active_id
                    .as_deref()
                    .is_some_and(|id| sec.sessions.iter().any(|s| s.id == id))
            })
            .map(|sec| sec.key.clone())
            .collect();

        if expanded_keys.is_empty() {
            expanded_keys.push(secs[0].key.clone());
        }

        collapsed.set(
            secs.iter()
                .filter(|sec| !expanded_keys.contains(&sec.key))
                .map(|sec| sec.key.clone())
                .collect(),
        );
        collapse_primed.set(true);
    });

    let is_open = move || open.get();
    let is_collapsed = move |key: &str| collapsed.get().contains(key);
    let toggle = move |key: String| {
        collapsed.update(|set| {
            if set.contains(&key) {
                set.remove(&key);
            } else {
                set.insert(key);
            }
        });
    };

    let is_empty = move || sections().is_empty();

    view! {
        <div
            class="sessions-overlay"
            class:sessions-overlay--open=is_open
            on:click=move |ev| {
                let target = event_target::<web_sys::HtmlElement>(&ev);
                if target.class_list().contains("sessions-overlay") {
                    open.set(false);
                }
            }
        >
            <div class="sessions-panel">
                <div class="sessions-panel__head">
                    <h2 class="sessions-panel__title">"Sessions"</h2>
                    <button
                        class="sessions-panel__close"
                        type="button"
                        aria-label="close sessions panel"
                        on:click=move |_| open.set(false)
                    >
                        "✕"
                    </button>
                </div>

                <div class="sessions-panel__actions">
                    <button
                        class="sessions-panel__new-btn"
                        type="button"
                        on:click=move |_| {
                            daemon.get_value().new_session();
                            open.set(false);
                        }
                    >
                        "+ New Session"
                    </button>
                </div>

                <div class="sessions-panel__list">
                    <For
                        each=sections
                        key=|sec| sec.key.clone()
                        children=move |sec: ProjectSection| {
                            let skey = sec.key.clone();
                            let s_is_project = sec.is_project;
                            let s_label = sec.label.clone();
                            let s_count = sec.sessions.len();
                            let worktrees = sec.worktrees.clone();
                            let flattened = sec.sessions.clone();
                            let glyph_key = skey.clone();
                            let show_key = skey.clone();
                            let click_key = skey.clone();
                            let glyph = move || if is_collapsed(&glyph_key) { "▸" } else { "▾" };

                            view! {
                                <div class="sessions-group">
                                    // ── Group header ────────────────────────
                                    <button
                                        class="sessions-group__head"
                                        class:sessions-group__head--other=!s_is_project
                                        type="button"
                                        on:click={
                                            let k = click_key.clone();
                                            move |_| toggle(k.clone())
                                        }
                                    >
                                        // Monogram badge or fallback glyph.
                                        {if s_is_project {
                                            view! {
                                                <span class="project-logo">{monogram(&s_label)}</span>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <span class="project-logo project-logo--other">"⋯"</span>
                                            }.into_any()
                                        }}

                                        <span class="sessions-group__label">{s_label.clone()}</span>
                                        <span class="sessions-group__glyph">{glyph}</span>
                                        <span class="sessions-group__count">{s_count}</span>
                                    </button>

                                    // ── Expanded body ───────────────────────
                                    <Show when=move || !is_collapsed(&show_key)>
                                        <div class="sessions-group__body">
                                            {if let Some(wts) = worktrees.clone() {
                                                // Worktree split: render sub-headers.
                                                wts.into_iter().map(|wt: WorktreeGroup| {
                                                    let rows = wt.sessions.clone();
                                                    let count = rows.len();
                                                    let root_label = wt.root.split('/').last()
                                                        .filter(|s| !s.is_empty())
                                                        .unwrap_or("worktree")
                                                        .to_string();
                                                    let branch_label = wt.branch.clone().filter(|b| !b.is_empty());
                                                    let has_branch = branch_label.is_some();
                                                    let branch_text = branch_label.unwrap_or_default();
                                                    view! {
                                                        <div class="worktree-group">
                                                            <div class="worktree-group__head">
                                                                <span class="worktree-group__root">{root_label}</span>
                                                                {if has_branch {
                                                                    view! { <span class="worktree-group__branch">{branch_text}</span> }.into_any()
                                                                } else {
                                                                    view! { <span class="worktree-group__branch worktree-group__branch--hidden"></span> }.into_any()
                                                                }}
                                                                <span class="worktree-group__count">
                                                                    {count}
                                                                </span>
                                                            </div>
                                                            {rows.into_iter().map(|s| session_row(s, daemon, open, current_id, false).into_any()).collect::<Vec<_>>()}
                                                        </div>
                                                    }.into_any()
                                                }).collect::<Vec<_>>()
                                            } else {
                                                // Flat list.
                                                flattened.clone().into_iter()
                                                    .map(|s| session_row(s, daemon, open, current_id, !s_is_project).into_any())
                                                    .collect::<Vec<_>>()
                                            }}
                                        </div>
                                    </Show>
                                </div>
                            }
                        }
                    />
                </div>

                <Show when=is_empty>
                    <div class="sessions-panel__empty">
                        "No sessions yet. Send a message to start one."
                    </div>
                </Show>
            </div>
        </div>
    }
}

/// One session row rendered in the panel. Extracted to avoid closure-in-view
/// borrow issues and keep the nested For stable.
fn session_row(
    session: SessionSummary,
    daemon: StoredValue<Daemon>,
    panel_open: RwSignal<bool>,
    current_id: RwSignal<Option<String>>,
    show_path: bool,
) -> impl IntoView {
    let session_id = session.id.clone();
    let session_title = if session.title.trim().is_empty() {
        "(untitled)".to_string()
    } else {
        session.title.clone()
    };
    let session_path = session.cwd.clone();
    let show_path = show_path && !session_path.trim().is_empty();
    let turn_label = format!(
        "{} turn{}",
        session.turn_count,
        if session.turn_count == 1 { "" } else { "s" }
    );
    let rel_time = fmt_relative_time(&session.updated_at);
    let row_id = session_id.clone();

    view! {
        <button
            class="sessions-item"
            class:sessions-item--active=move || current_id.get().as_deref() == Some(row_id.as_str())
            type="button"
            on:click={
                let id = session_id.clone();
                let title = session.title.clone();
                move |_| {
                    daemon.get_value().switch_session(id.clone(), title.clone());
                    panel_open.set(false);
                }
            }
        >
            <div class="sessions-item__title">{session_title}</div>
            <div class="sessions-item__meta">
                <span class="sessions-item__time">{rel_time}</span>
                <span class="sessions-item__turns">{turn_label}</span>
            </div>
            {if show_path {
                view! { <div class="sessions-item__path">{session_path}</div> }.into_any()
            } else {
                view! { <div class="sessions-item__path sessions-item__path--hidden"></div> }.into_any()
            }}
        </button>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::OwningProjectRef;

    fn project(id: &str, name: &str, workspace_root: &str) -> ProjectInfo {
        ProjectInfo {
            id: id.to_string(),
            name: name.to_string(),
            workspace_root: workspace_root.to_string(),
        }
    }

    fn owner(id: &str, name: &str) -> OwningProjectRef {
        OwningProjectRef {
            id: id.to_string(),
            name: name.to_string(),
        }
    }

    fn session(
        id: &str,
        cwd: &str,
        workspace_root: Option<&str>,
        owning_project: Option<OwningProjectRef>,
        git_branch: Option<&str>,
        turn_count: u32,
        updated_at: &str,
    ) -> SessionSummary {
        SessionSummary {
            id: id.to_string(),
            title: id.to_string(),
            cwd: cwd.to_string(),
            workspace_root: workspace_root.map(str::to_string),
            owning_project,
            git_branch: git_branch.map(str::to_string),
            turn_count,
            updated_at: updated_at.to_string(),
        }
    }

    #[test]
    fn zero_turn_sessions_are_pruned_unless_active() {
        let projects = vec![project("repo", "Repo", "/repo")];
        let sessions = vec![
            session("active-zero", "/repo", Some("/repo"), None, None, 0, "2026-07-05T12:02:00Z"),
            session("stale-zero", "/repo", Some("/repo"), None, None, 0, "2026-07-05T12:01:00Z"),
            session("with-turn", "/repo", Some("/repo"), None, None, 1, "2026-07-05T12:00:00Z"),
        ];

        let sections = group_for_panel(&sessions, &projects, Some("active-zero"));

        assert_eq!(sections.len(), 1);
        let ids: Vec<&str> = sections[0].sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["active-zero", "with-turn"]);
    }

    #[test]
    fn owning_project_wins_over_matching_catalogue_root() {
        let projects = vec![project("catalogue-root", "Catalogue Root", "/repo")];
        let sessions = vec![session(
            "owned-by-daemon",
            "/repo",
            Some("/repo"),
            Some(owner("daemon-owner", "Daemon Owner")),
            None,
            1,
            "2026-07-05T12:00:00Z",
        )];

        let sections = group_for_panel(&sessions, &projects, None);

        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].key, "daemon-owner");
        assert_eq!(sections[0].label, "Daemon Owner");
        assert_eq!(sections[0].sessions[0].id, "owned-by-daemon");
    }

    #[test]
    fn exact_workspace_or_cwd_root_matches_project_and_other_stays_last() {
        let projects = vec![project("matched", "Matched", "/repo")];
        let sessions = vec![
            session("workspace-match", "/ignored", Some("/repo"), None, None, 1, "2026-07-05T12:01:00Z"),
            session("cwd-match", "/repo", None, None, None, 1, "2026-07-05T12:00:00Z"),
            session("unmatched-newest", "/elsewhere", None, None, None, 1, "2026-07-05T12:02:00Z"),
        ];

        let sections = group_for_panel(&sessions, &projects, None);

        let keys: Vec<&str> = sections.iter().map(|section| section.key.as_str()).collect();
        assert_eq!(keys, vec!["matched", "__other__"]);
        let matched_ids: Vec<&str> =
            sections[0].sessions.iter().map(|session| session.id.as_str()).collect();
        assert_eq!(matched_ids, vec!["workspace-match", "cwd-match"]);
        assert_eq!(sections[1].sessions[0].id, "unmatched-newest");
    }

    #[test]
    fn project_with_multiple_roots_splits_into_worktrees_and_preserves_branch_labels() {
        let sessions = vec![
            session(
                "main-root",
                "/repo-main",
                Some("/repo-main"),
                Some(owner("owned", "Owned")),
                Some("main"),
                1,
                "2026-07-05T12:00:00Z",
            ),
            session(
                "feature-root",
                "/repo-feature",
                Some("/repo-feature"),
                Some(owner("owned", "Owned")),
                Some("feature/redesign"),
                1,
                "2026-07-05T12:01:00Z",
            ),
        ];

        let sections = group_for_panel(&sessions, &[], None);

        assert_eq!(sections.len(), 1);
        let worktrees = sections[0].worktrees.as_ref().expect("project should split by root");
        assert_eq!(worktrees.len(), 2);
        let main = worktrees.iter().find(|worktree| worktree.root == "/repo-main").unwrap();
        assert_eq!(main.branch.as_deref(), Some("main"));
        assert_eq!(main.sessions[0].id, "main-root");
        let feature =
            worktrees.iter().find(|worktree| worktree.root == "/repo-feature").unwrap();
        assert_eq!(feature.branch.as_deref(), Some("feature/redesign"));
        assert_eq!(feature.sessions[0].id, "feature-root");
    }

    #[test]
    fn project_sections_and_rows_sort_newest_first_with_other_last() {
        let projects = vec![project("alpha", "Alpha", "/alpha"), project("beta", "Beta", "/beta")];
        let sessions = vec![
            session("alpha-old", "/alpha", Some("/alpha"), None, None, 1, "2026-07-05T12:00:00Z"),
            session("alpha-new", "/alpha", Some("/alpha"), None, None, 1, "2026-07-05T12:02:00Z"),
            session("beta-newest-project", "/beta", Some("/beta"), None, None, 1, "2026-07-05T12:03:00Z"),
            session("other-newest-overall", "/other", None, None, None, 1, "2026-07-05T12:04:00Z"),
        ];

        let sections = group_for_panel(&sessions, &projects, None);

        let keys: Vec<&str> = sections.iter().map(|section| section.key.as_str()).collect();
        assert_eq!(keys, vec!["beta", "alpha", "__other__"]);
        let alpha_rows: Vec<&str> =
            sections[1].sessions.iter().map(|session| session.id.as_str()).collect();
        assert_eq!(alpha_rows, vec!["alpha-new", "alpha-old"]);
    }
}
