//! Sessions panel — grouped by project, worktrees, with collapsible sections.
//!
//! Sessions are grouped under their owning project: authoritatively via the
//! daemon's `owning_project` when it serves that field (once the enriched DTO
//! lands), or by exact-matching a session's workspace root to a project in the
//! catalogue (`daemon.projects`). Sessions with no matching project fall into
//! an "Other" bucket.
//!
//! Within a project, sessions whose workspace root sits under one of the
//! project's registered worktrees (daemon-enriched `worktrees` on
//! `ProjectInfo`, component-boundary prefix match) group under that worktree
//! sub-row; the rest stay in the project's flat list.
//!
//! Zero-turn drafts are filtered out unless they're the active session (the
//! lazy session creation approach no longer POSTs on "New Session" click, so
//! empty sessions no longer accumulate in the store; historical litter is
//! pruned from the display).

use std::collections::HashSet;
use leptos::prelude::*;
use leptos::ev::SubmitEvent;
use wasm_bindgen_futures::spawn_local;

use crate::daemon::{Daemon, fetch_fs_dirs, is_path_prefix, ProjectInfo, SessionSummary};

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

fn project_slug(name: &str) -> String {
    let mut slug = String::new();
    let mut just_dashed = false;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            just_dashed = false;
        } else if !slug.is_empty() && !just_dashed {
            slug.push('-');
            just_dashed = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "project-name".to_string()
    } else {
        slug
    }
}

fn project_root_from_parent(parent: &str, name: &str) -> String {
    let parent = parent.trim();
    let base = parent.trim_end_matches('/');
    let slug = project_slug(name);
    if base.is_empty() {
        if parent.starts_with('/') {
            format!("/{slug}")
        } else {
            slug
        }
    } else {
        format!("{base}/{slug}")
    }
}

fn project_create_root(text_mode: bool, parent: &str, root: &str, name: &str) -> String {
    if text_mode || parent.trim().is_empty() {
        root.trim().to_string()
    } else {
        project_root_from_parent(parent, name)
    }
}

/// One project section in the panel: a project header followed by its sessions,
/// optionally with worktree sub-groups when the daemon reports registered
/// worktrees for the project.
#[derive(Clone, Debug)]
pub(crate) struct ProjectSection {
    pub key: String,
    pub label: String,
    pub is_project: bool,
    /// Sessions not bucketed under any worktree (all sessions when `worktrees` is None).
    pub sessions: Vec<SessionSummary>,
    /// Sub-groups keyed by registered worktree path, set only when >=1 session matched.
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

    // Zero-session projects still render as empty groups so a freshly-created
    // project (no sessions yet) appears in the panel. Append a section for any
    // project that didn't collect a session above, keyed by the project id so
    // later sessions merge into it.
    for p in projects {
        if sections.iter().any(|sec| sec.key == p.id) {
            continue;
        }
        let label = if p.name.trim().is_empty() {
            p.workspace_root
                .rsplit('/')
                .find(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| p.workspace_root.clone())
        } else {
            p.name.clone()
        };
        sections.push(ProjectSection {
            key: p.id.clone(),
            label,
            is_project: true,
            sessions: Vec::new(),
            worktrees: None,
        });
    }

    // Sort sessions inside each group: newest first (ISO-8601 lexicographic cmp).
    for sec in &mut sections {
        sec.sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    }
    // Worktree bucketing: second pass inside each project section.
    // Sessions whose workspace root sits under a project worktree's path
    // (component-boundary prefix match) group under that sub-row;
    // remaining sessions stay in the project's main session list.
    // Zero worktrees → exactly current rendering (no sub-groups).
    for sec in &mut sections {
        if !sec.is_project {
            continue;
        }
        let proj = projects.iter().find(|p| p.id == sec.key);
        let wts = match proj {
            Some(p) if !p.worktrees.is_empty() => &p.worktrees,
            _ => continue,
        };
        let mut wt_groups: Vec<WorktreeGroup> = Vec::new();
        let mut unmatched: Vec<SessionSummary> = Vec::new();
        for s in &sec.sessions {
            let root = session_root(s);
            let matching = wts.iter().find(|wt| is_path_prefix(&wt.path, root));
            if let Some(wt) = matching {
                let branch = wt.branch.clone();
                match wt_groups.iter_mut().find(|g| g.root == wt.path) {
                    Some(g) => g.sessions.push(s.clone()),
                    None => wt_groups.push(WorktreeGroup {
                        root: wt.path.clone(),
                        branch,
                        sessions: vec![s.clone()],
                    }),
                }
            } else {
                unmatched.push(s.clone());
            }
        }
        if !wt_groups.is_empty() {
            for wt in &mut wt_groups {
                wt.sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            }
            wt_groups.sort_by(|a, b| {
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
            sec.worktrees = Some(wt_groups);
            sec.sessions = unmatched;
        }
    }

    // Sort sections: "Other" always last; session-derived project groups by
    // newest session (newest first); zero-session projects append after the
    // session-derived ones, alphabetical by label.
    sections.sort_by(|a, b| {
        if a.key == "__other__" && b.key != "__other__" {
            return std::cmp::Ordering::Greater;
        }
        if b.key == "__other__" && a.key != "__other__" {
            return std::cmp::Ordering::Less;
        }
        let a_ts = a.sessions.first().map(|s| s.updated_at.as_str()).unwrap_or("");
        let b_ts = b.sessions.first().map(|s| s.updated_at.as_str()).unwrap_or("");
        match (a_ts.is_empty(), b_ts.is_empty()) {
            (false, false) => b_ts.cmp(a_ts),
            (true, true) => a
                .label
                .to_ascii_lowercase()
                .cmp(&b.label.to_ascii_lowercase()),
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
        }
    });

    sections
}

/// The surface-origin icon for a session's `[TAG]` title prefix. The tag set
/// mirrors ocean-agent's `surface_flag` (the canonical client_type → flag
/// map): BRWSR/TUI/WEB/GUI/CLI/VOX/ACP/SLACK/CNVS/MOBL. Recognized surfaces
/// get a designed mark; anything else falls back to the small text badge so
/// future surfaces degrade legibly instead of invisibly.
pub(crate) fn origin_icon(tag: &str) -> Option<AnyView> {
    match tag {
        "tui" | "cli" => Some(view! { <crate::icons::Terminal /> }.into_any()),
        "web" => Some(view! { <crate::icons::Globe /> }.into_any()),
        "brwsr" => Some(view! { <crate::icons::Puzzle /> }.into_any()),
        "gui" | "desktop" | "app" => Some(view! { <crate::icons::Desktop /> }.into_any()),
        "acp" => Some(view! { <crate::icons::Code /> }.into_any()),
        "vox" => Some(view! { <crate::icons::Mic /> }.into_any()),
        "slack" => Some(view! { <crate::icons::Slack /> }.into_any()),
        "mobl" => Some(view! { <crate::icons::Smartphone /> }.into_any()),
        _ => None,
    }
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

/// Session freshness: true when the timestamp is under ten minutes old.
/// Drives the live dot on session rows.
pub(crate) fn is_recent(updated_at: &str) -> bool {
    let ts = js_sys::Date::parse(updated_at);
    !ts.is_nan() && (js_sys::Date::now() - ts) < 600_000.0
}

/// Split a `[TAG] rest` origin prefix off a session title. The daemon embeds
/// the client surface as a bracketed prefix (`[WEB] hi`, `[TUI] PM room`);
/// rendered raw it reads as slop. `[?]` (unknown client) strips without a
/// badge. Titles without a well-formed prefix pass through untouched.
pub(crate) fn split_origin(title: &str) -> (Option<String>, String) {
    let t = title.trim();
    if let Some(rest) = t.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            let tag = &rest[..end];
            let body = rest[end + 1..].trim();
            if !tag.is_empty() && tag.len() <= 8 && !body.is_empty() {
                let badge =
                    if tag == "?" { None } else { Some(tag.to_ascii_lowercase()) };
                return (badge, body.to_string());
            }
        }
    }
    (None, t.to_string())
}

// ---------------------------------------------------------------------------
// SessionsPanel component
// ---------------------------------------------------------------------------

/// Sessions modal — a centered overlay for chat, project creation, and resuming sessions.
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

    // Create-project form state (modal-only; posts via daemon.create_project).
    let create_name = RwSignal::new(String::new());
    let create_root = RwSignal::new(String::new());
    // Reveal-on-intent: the create form hides behind a quiet `+ New project`
    // row until the user asks for it.
    let show_create = RwSignal::new(false);
    // Breadcrumb directory browser state.
    let breadcrumb_parent = RwSignal::new(String::new());
    let breadcrumb_dirs = RwSignal::new(Vec::<crate::daemon::FsDirEntry>::new());
    let breadcrumb_loading = RwSignal::new(false);
    let breadcrumb_filter = RwSignal::new(String::new());
    let breadcrumb_text_mode = RwSignal::new(false);
    let breadcrumb_home = RwSignal::new(String::new());
    // True when the user clicked "Use this folder" — the current breadcrumb
    // directory is the project root directly (no parent+name slug derivation).
    let register_existing: RwSignal<bool> = RwSignal::new(false);
    let _popover_highlight = RwSignal::new(0usize);

    // Browse mode treats the chosen directory as a PARENT. The actual project
    // root is parent + normalized project-name slug, and the daemon creates
    // that folder on POST /v1/projects.  When `register_existing` is true the
    // browsed directory IS the project root (no parent+slug derivation).
    Effect::new(move |_| {
        if !breadcrumb_text_mode.get() {
            let parent = breadcrumb_parent.get();
            if !parent.trim().is_empty() {
                if register_existing.get_untracked() {
                    create_root.set(parent.clone());
                } else {
                    create_root.set(project_root_from_parent(&parent, &create_name.get()));
                }
            }
        }
    });

    // Initialize default project parent when panel opens.
    Effect::new(move |_| {
        if open.get() && create_root.get_untracked().is_empty() {
            let url = daemon.get_value().url.get_untracked();
            let parent = "~/dev".to_string();
            breadcrumb_parent.set(parent.clone());
            create_root.set(project_root_from_parent(&parent, &create_name.get_untracked()));
            spawn_local(async move {
                let u = url.clone();
                if let Some(resp) = fetch_fs_dirs(&u, &parent).await {
                    if resp.ok {
                        breadcrumb_home.set(resp.home.clone());
                        breadcrumb_dirs.set(resp.dirs);
                    } else {
                        let u2 = u.clone();
                        if let Some(resp2) = fetch_fs_dirs(&u2, "~").await {
                            if resp2.ok {
                                let fallback_parent = "~".to_string();
                                breadcrumb_home.set(resp2.home.clone());
                                breadcrumb_parent.set(fallback_parent.clone());
                                breadcrumb_dirs.set(resp2.dirs);
                                create_root.set(project_root_from_parent(
                                    &fallback_parent,
                                    &create_name.get_untracked(),
                                ));
                            }
                        }
                    }
                }
            });
        }
    });
    // The reveal row is the default state every time the modal opens — reset
    // show_create when the panel closes so reopening never shows a stale form.
    Effect::new(move |_| {
        if !open.get() {
            show_create.set(false);
            register_existing.set(false);
        }
    });

    let create_root_value = move || if register_existing.get() {
        create_root.get()
    } else {
        project_create_root(
            breadcrumb_text_mode.get(),
            &breadcrumb_parent.get(),
            &create_root.get(),
            &create_name.get(),
        )
    };
    let create_can_submit = move || {
        let name = create_name.get();
        let root = create_root_value();
        !name.trim().is_empty() && !root.trim().is_empty()
    };
    let start_chat = move |_| {
        daemon.get_value().begin_chat_session();
        open.set(false);
    };
    let create_project = move |ev: SubmitEvent| {
        ev.prevent_default();
        let name = create_name.get_untracked();
        let root = create_root_value();
        daemon.get_value().create_project(name, root);
    };

    // Create-project success closes the modal and clears the form; failure
    // keeps it open with the inline error. Watch the falling edge of the
    // daemon's pending signal so the close lands only after a real round-trip.
    let prev_pending: RwSignal<bool> = RwSignal::new(false);
    Effect::new(move |_| {
        let pending = daemon.get_value().project_create_pending.get();
        let was_pending = prev_pending.get_untracked();
        prev_pending.set(pending);
        if was_pending && !pending
            && daemon.get_value().project_create_error.get_untracked().is_none()
        {
            open.set(false);
            create_name.set(String::new());
            create_root.set(String::new());
            breadcrumb_parent.set(String::new());
            register_existing.set(false);
            breadcrumb_dirs.set(Vec::new());
            show_create.set(false);
        }
    });

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
            <div class="sessions-panel ocean-lit" role="dialog" aria-modal="true" aria-label="Sessions">
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
                    <button class="sessions-panel__new-btn" type="button" on:click=start_chat>
                        "New chat"
                    </button>

                    <button
                        class="sessions-create__reveal"
                        type="button"
                        on:click=move |_| show_create.set(true)
                    >
                        "+ New project"
                    </button>

                    <Show when=move || show_create.get()>
                    <form class="sessions-create" on:submit=create_project>
                        <div class="sessions-create__inputs">
                            <input
                                class="sessions-create__input"
                                type="text"
                                placeholder="Project name"
                                autocomplete="off"
                                prop:value=move || create_name.get()
                                on:input=move |ev| create_name.set(event_target_value(&ev))
                            />
                                                        {move || if breadcrumb_text_mode.get() {
                                view! {
                                    <div style="display:flex;gap:6px;align-items:center;flex:1 1 auto">
                                        <input
                                            class="sessions-create__input"
                                            type="text"
                                            placeholder="Project folder (absolute path)"
                                            autocomplete="off"
                                            spellcheck="false"
                                            prop:value=move || create_root.get()
                                            on:input=move |ev| create_root.set(event_target_value(&ev))
                                        />
                                        <button
                                            class="sessions-create__mode"
                                            type="button"
                                            title="Browse directories"
                                            on:click=move |_| breadcrumb_text_mode.set(false)
                                        >
                                            "browse"
                                        </button>
                                    </div>
                                }.into_any()
                            } else {
                                let path_str = create_root.get();
                                let has_tilde = path_str.starts_with('~');
                                let rest = if has_tilde { path_str[1..].to_string() } else { path_str.clone() };
                                let parts: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
                                let home = breadcrumb_home.get();
                                let mut display_segs: Vec<(String, String)> = Vec::new();
                                if has_tilde {
                                    display_segs.push(("~".to_string(), home.clone()));
                                    let mut prefix = home.clone();
                                    for seg in &parts {
                                        prefix = format!("{}/{}", prefix.trim_end_matches('/'), seg);
                                        display_segs.push((seg.to_string(), prefix.clone()));
                                    }
                                } else if !parts.is_empty() {
                                    let mut prefix = String::new();
                                    for seg in &parts {
                                        prefix = format!("{}/{}", prefix.trim_end_matches('/'), seg);
                                        display_segs.push((seg.to_string(), prefix.clone()));
                                    }
                                }
                                if display_segs.is_empty() {
                                    let h = "~".to_string();
                                    display_segs.push((h.clone(), home.clone()));
                                }
                                let d_url = daemon.get_value().url.get_untracked();

                                view! {
                                    <div class="sessions-create__breadcrumb">
                                        <div class="sessions-create__breadcrumb-row">
                                            {display_segs.iter().enumerate().map(|(idx, (label, prefix))| {
                                                let is_last = idx == display_segs.len() - 1;
                                                let lbl = label.clone();
                                                let pfx = prefix.clone();
                                                let u = d_url.clone();

                                                view! {
                                                    <button
                                                        class="sessions-create__breadcrumb-seg"
                                                        type="button"
                                                        on:click={
                                                            let uu = u.clone();
                                                            let p = pfx.clone();
                                                            move |_| {
                                                                breadcrumb_parent.set(p.clone());
                                                                breadcrumb_filter.set(String::new());
                                                                let uuu = uu.clone();
                                                                let p2 = p.clone();
                                                                spawn_local(async move {
                                                                    if let Some(resp) = fetch_fs_dirs(&uuu, &p2).await {
                                                                        if resp.ok {
                                                                            breadcrumb_home.set(resp.home);
                                                                            breadcrumb_dirs.set(resp.dirs);
                                                                        }
                                                                    }
                                                                });
                                                            }
                                                        }
                                                    >
                                                        {lbl}
                                                    </button>
                                                    {if !is_last {
                                                        view! { <span class="sessions-create__breadcrumb-sep">" / "</span> }.into_any()
                                                    } else {
                                                        ().into_any()
                                                    }}
                                                }.into_any()
                                            }).collect::<Vec<_>>()}
                                        </div>
                                        <button
                                            class="sessions-create__mode"
                                            type="button"
                                            title="Edit path as text"
                                            on:click=move |_| breadcrumb_text_mode.set(true)
                                        >
                                            "edit"
                                        </button>
                                        {move || {
                                            let popover_parent = breadcrumb_parent.get();
                                            if popover_parent.is_empty() {
                                                return ().into_any();
                                            }
                                            let dirs = breadcrumb_dirs.get();
                                            let filter = breadcrumb_filter.get();
                                            let loading = breadcrumb_loading.get();
                                            let filtered: Vec<crate::daemon::FsDirEntry> = {
                                                // Drop dotfiles and common build/dep noise so the
                                                // browser shows real project parents only.
                                                let clean: Vec<crate::daemon::FsDirEntry> = dirs
                                                    .iter()
                                                    .filter(|d| {
                                                        !(d.name.starts_with('.')
                                                            || matches!(
                                                                d.name.as_str(),
                                                                "__pycache__" | "node_modules" | "target"
                                                            ))
                                                    })
                                                    .cloned()
                                                    .collect();
                                                if filter.is_empty() {
                                                    clean
                                                } else {
                                                    clean
                                                        .iter()
                                                        .filter(|d| {
                                                            d.name.to_lowercase().contains(&filter.to_lowercase())
                                                        })
                                                        .cloned()
                                                        .collect()
                                                }
                                            };

                                            view! {
                                                <div class="sessions-create__popover">
                                                    <input
                                                        class="sessions-create__popover-filter"
                                                        type="text"
                                                        placeholder="Filter directories..."
                                                        autofocus=true
                                                        prop:value=move || breadcrumb_filter.get()
                                                        on:input=move |ev| breadcrumb_filter.set(event_target_value(&ev))
                                                        on:keydown=move |ev| {
                                                            if ev.key() == "Escape" {
                                                                breadcrumb_parent.set(String::new());
                                                            }
                                                        }
                                                    />
                                                    {move || {
                                                        let parent = breadcrumb_parent.get();
                                                        if parent.trim().is_empty() {
                                                            return ().into_any();
                                                        }
                                                        let basename = parent.rsplit('/').find(|s| !s.is_empty()).unwrap_or("project").to_string();
                                                        let bn = basename.clone();
                                                        let p = parent.clone();
                                                        view! {
                                                            <button
                                                                class="sessions-create__use-existing"
                                                                type="button"
                                                                on:click={
                                                                    let bnc = bn.clone();
                                                                    let pc = p.clone();
                                                                    move |_| {
                                                                        create_name.set(bnc.clone());
                                                                        create_root.set(pc.clone());
                                                                        register_existing.set(true);
                                                                    }
                                                                }
                                                            >
                                                                "Use folder: "
                                                                <span class="sessions-create__use-existing-path">{basename}</span>
                                                            </button>
                                                        }.into_any()
                                                    }}
                                                    <div class="sessions-create__popover-list">
                                                        {if loading {
                                                            view! { <div class="sessions-create__popover-status">"Loading..."</div> }.into_any()
                                                        } else if filtered.is_empty() {
                                                            view! {
                                                                <div class="sessions-create__popover-status">"No directories"</div>
                                                                <button
                                                                    class="sessions-create__popover-new"
                                                                    type="button"
                                                                    on:click={
                                                                        let pp = popover_parent.clone();
                                                                        move |_| {
                                                                            let new_path = project_root_from_parent(&pp, &create_name.get_untracked());
                                                                            create_root.set(new_path);
                                                                            breadcrumb_parent.set(String::new());
                                                                        }
                                                                    }
                                                                >
                                                                    "+ new folder: "
                                                                    <span class="sessions-create__popover-new-name">
                                                                        {move || {
                                                                            let n = create_name.get();
                                                                            if n.trim().is_empty() { "project-name".to_string() } else { n.trim().to_lowercase().replace(' ', "-") }
                                                                        }}
                                                                    </span>
                                                                </button>
                                                            }.into_any()
                                                        } else {
                                                            let items: Vec<_> = filtered.iter().map(|d| {
                                                                let name = d.name.clone();
                                                                let path = d.path.clone();
                                                                let repo = d.is_repo;
                                                                let branch = d.git_branch.clone();
                                                                let u = d_url.clone();
                                                                view! {
                                                                    <button
                                                                        class="sessions-create__popover-item"
                                                                        type="button"
                                                                        on:click={
                                                                            let path_owned = path.clone();
                                                                            let uu = u.clone();
                                                                            move |_| {
                                                                                breadcrumb_parent.set(path_owned.clone());
                                                                                breadcrumb_filter.set(String::new());
                                                                                let uuu = uu.clone();
                                                                                let p3 = path_owned.clone();
                                                                                spawn_local(async move {
                                                                                    if let Some(resp) = fetch_fs_dirs(&uuu, &p3).await {
                                                                                        if resp.ok {
                                                                                            breadcrumb_home.set(resp.home);
                                                                                            breadcrumb_dirs.set(resp.dirs);
                                                                                        }
                                                                                    }
                                                                                });
                                                                            }
                                                                        }
                                                                    >
                                                                        <span class="sessions-create__popover-item-name">{name.clone()}</span>
                                                                        {if repo && branch.is_some() {
                                                                            view! { <span class="sessions-create__popover-item-chip">{branch.as_deref().unwrap_or("")}</span> }.into_any()
                                                                        } else {
                                                                            ().into_any()
                                                                        }}
                                                                    </button>
                                                                }
                                                            }).collect();
                                                            view! {
                                                                {items.into_iter().collect::<Vec<_>>()}
                                                                <button
                                                                    class="sessions-create__popover-new"
                                                                    type="button"
                                                                    on:click={
                                                                        let pp = popover_parent.clone();
                                                                        move |_| {
                                                                            let new_path = project_root_from_parent(&pp, &create_name.get_untracked());
                                                                            create_root.set(new_path);
                                                                            breadcrumb_parent.set(String::new());
                                                                        }
                                                                    }
                                                                >
                                                                    "+ new folder: "
                                                                    <span class="sessions-create__popover-new-name">
                                                                        {move || {
                                                                            let n = create_name.get();
                                                                            if n.trim().is_empty() { "project-name".to_string() } else { n.trim().to_lowercase().replace(' ', "-") }
                                                                        }}
                                                                    </span>
                                                                </button>
                                                            }.into_any()
                                                        }}
                                                    </div>
                                                </div>
                                            }.into_any()
                                        }}
                                    </div>
                                }.into_any()
                            }}

                        </div>
                        <button
                            class="sessions-create__btn"
                            type="submit"
                            disabled=move || !create_can_submit()
                                || daemon.get_value().project_create_pending.get()
                        >
                            {move || if daemon.get_value().project_create_pending.get() {
                                "Creating…"
                            } else {
                                "Create project"
                            }}
                        </button>
                        <Show when=move || daemon.get_value().project_create_error.get().is_some()>
                            <div class="sessions-create__error">
                                {move || daemon.get_value().project_create_error.get().unwrap_or_default()}
                            </div>
                        </Show>
                    </form>
                    </Show>
                </div>

                <div class="sessions-panel__list">
                    <For
                        each=sections
                        key=|sec| {
                            // Content-sensitive key. `<For>` never re-runs `children` for a
                            // key it has already seen, so keying on `sec.key` alone froze
                            // every section at its first-paint count: the project sections
                            // render during the empty pre-fetch pass (count 0) and their keys
                            // persist, so their counts never updated when sessions arrived —
                            // only the newly-keyed "Other" bucket painted fresh. Folding the
                            // session count + newest-session signature into the key forces a
                            // re-render whenever a section's contents change. Collapse state is
                            // keyed on `sec.key` separately, so it survives.
                            let head = sec.sessions.first();
                            format!(
                                "{}|{}|{}|{}",
                                sec.key,
                                sec.sessions.len(),
                                head.map(|s| s.id.as_str()).unwrap_or(""),
                                head.map(|s| s.updated_at.as_str()).unwrap_or(""),
                            )
                        }
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
                            let new_key = skey.clone();
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
                                        // Project folder mark or the Other-bucket glyph.
                                        {if s_is_project {
                                            view! {
                                                <span class="project-logo"><crate::icons::Folder /></span>
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

                                    // ── Per-project new session (real projects only) ──
                                    <Show when=move || s_is_project>
                                        <button
                                            class="sessions-group__new-btn"
                                            type="button"
                                            on:click={
                                                let k = new_key.clone();
                                                move |_| {
                                                    daemon.get_value().begin_project_session(k.clone());
                                                    open.set(false);
                                                }
                                            }
                                        >
                                            "+ New session"
                                        </button>
                                    </Show>

                                    // ── Expanded body ───────────────────────
                                    <Show when=move || !is_collapsed(&show_key)>
                                        <div class="sessions-group__body">
                                            {if let Some(wts) = worktrees.clone() {
                                                let mut out: Vec<_> = wts.into_iter().map(|wt: WorktreeGroup| {
                                                    let rows = wt.sessions.clone();
                                                    let count = rows.len();
                                                    let root_label = wt.root.split('/').next_back()
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
                                                                    view! { <span class="worktree-group__branch"><crate::icons::GitBranch />{branch_text}</span> }.into_any()
                                                                } else {
                                                                    view! { <span class="worktree-group__branch worktree-group__branch--hidden"></span> }.into_any()
                                                                }}
                                                                <span class="worktree-group__count">
                                                                    {count}
                                                                </span>
                                                            </div>
                                                            {rows.into_iter().map(|s| session_row(s, daemon, open, current_id, false, None).into_any()).collect::<Vec<_>>()}
                                                        </div>
                                                    }.into_any()
                                                }).collect();
                                                // Remaining sessions (not matched to any worktree) shown flat below.
                                                out.extend(flattened.clone().into_iter()
                                                    .map(|s| session_row(s, daemon, open, current_id, true, Some(s_label.clone())).into_any()));
                                                out
                                            } else {
                                                // Flat list.
                                                flattened.clone().into_iter()
                                                    .map(|s| session_row(s, daemon, open, current_id, true, if s_is_project { Some(s_label.clone()) } else { None }).into_any())
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
                        "No sessions yet — start a chat or create a project."
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
    show_context: bool,
    group_label: Option<String>,
) -> impl IntoView {
    let session_id = session.id.clone();
    let (origin, clean_title) = split_origin(&session.title);
    let session_title = if clean_title.is_empty() {
        "(untitled)".to_string()
    } else {
        clean_title
    };
    let turn_label = format!(
        "{} turn{}",
        session.turn_count,
        if session.turn_count == 1 { "" } else { "s" }
    );
    let rel_time = fmt_relative_time(&session.updated_at);
    let fresh = is_recent(&session.updated_at);
    // Repo context: the cwd's last segment plus the live branch. Suppressed
    // inside worktree groups (header names root + branch) and when the tail
    // would just echo the enclosing project group's label.
    let repo_tail = if show_context && !session.cwd.is_empty() && session.cwd != "/" {
        session
            .cwd
            .rsplit('/')
            .find(|s| !s.is_empty())
            .map(str::to_string)
            .filter(|tail| {
                group_label
                    .as_deref()
                    .is_none_or(|label| !label.eq_ignore_ascii_case(tail))
            })
    } else {
        None
    };
    let branch = if show_context {
        session.git_branch.clone().filter(|b| !b.is_empty())
    } else {
        None
    };
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
            <div class="sessions-item__title">
                {fresh.then(|| view! {
                    <span class="sessions-item__dot" title="active in the last 10 minutes"></span>
                })}
                {origin.map(|o| match origin_icon(&o) {
                    Some(icon) => view! {
                        <span class="sessions-item__origin" title=format!("{o} session")>{icon}</span>
                    }.into_any(),
                    None => view! { <span class="sessions-item__badge">{o}</span> }.into_any(),
                })}
                <span class="sessions-item__name">{session_title}</span>
            </div>
            <div class="sessions-item__meta">
                {repo_tail.map(|p| view! { <span class="sessions-item__path">{p}</span> })}
                {branch.map(|b| view! {
                    <span class="sessions-item__branch"><crate::icons::GitBranch />{b}</span>
                })}
                <span class="sessions-item__time">{rel_time}</span>
                <span class="sessions-item__turns">{turn_label}</span>
            </div>
        </button>
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn split_origin_parses_web_prefix() {
        let (badge, title) = super::split_origin("[WEB] hi there");
        assert_eq!(badge.as_deref(), Some("web"));
        assert_eq!(title, "hi there");
    }

    #[test]
    fn split_origin_strips_unknown_client_without_badge() {
        let (badge, title) = super::split_origin("[?] hi");
        assert_eq!(badge, None);
        assert_eq!(title, "hi");
    }

    #[test]
    fn split_origin_passes_plain_titles_through() {
        let (badge, title) = super::split_origin("plain title");
        assert_eq!(badge, None);
        assert_eq!(title, "plain title");
        // Unclosed bracket / empty body stay raw.
        assert_eq!(super::split_origin("[WEB").1, "[WEB");
        assert_eq!(super::split_origin("[WEB]").1, "[WEB]");
    }

    use super::*;
    use crate::daemon::{OwningProjectRef, WorktreeInfo};

    fn project(id: &str, name: &str, workspace_root: &str) -> ProjectInfo {
        ProjectInfo {
            id: id.to_string(),
            name: name.to_string(),
            workspace_root: workspace_root.to_string(),
            git_branch: None,
            git_dirty: None,
            worktrees: Vec::new(),
        }
    }

    fn project_with_worktrees(
        id: &str,
        name: &str,
        workspace_root: &str,
        worktrees: &[(&str, Option<&str>)],
    ) -> ProjectInfo {
        ProjectInfo {
            worktrees: worktrees
                .iter()
                .map(|(path, branch)| WorktreeInfo {
                    path: path.to_string(),
                    branch: branch.map(str::to_string),
                })
                .collect(),
            ..project(id, name, workspace_root)
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

        // owning_project wins: the session groups under daemon-owner, NOT under
        // the catalogue project that shares its root. The orphaned catalogue
        // project still renders as its own empty section (Step 2.8).
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].key, "daemon-owner");
        assert_eq!(sections[0].label, "Daemon Owner");
        assert_eq!(sections[0].sessions[0].id, "owned-by-daemon");
        assert_eq!(sections[1].key, "catalogue-root");
        assert!(sections[1].sessions.is_empty());
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
    fn sessions_bucket_under_registered_worktrees_and_unmatched_stay_flat() {
        let projects = vec![project_with_worktrees(
            "owned",
            "Owned",
            "/repo-main",
            &[("/repo-feature", Some("feature/redesign"))],
        )];
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
            session(
                "feature-nested",
                "/repo-feature/crates/x",
                Some("/repo-feature/crates/x"),
                Some(owner("owned", "Owned")),
                None,
                1,
                "2026-07-05T12:02:00Z",
            ),
            // Component-boundary trap: /repo-featurex is NOT under /repo-feature.
            session(
                "boundary-trap",
                "/repo-featurex",
                Some("/repo-featurex"),
                Some(owner("owned", "Owned")),
                None,
                1,
                "2026-07-05T12:03:00Z",
            ),
        ];

        let sections = group_for_panel(&sessions, &projects, None);

        assert_eq!(sections.len(), 1);
        let sec = &sections[0];
        let worktrees = sec.worktrees.as_ref().expect("worktree sub-groups expected");
        assert_eq!(worktrees.len(), 1);
        let feature = &worktrees[0];
        assert_eq!(feature.root, "/repo-feature");
        assert_eq!(feature.branch.as_deref(), Some("feature/redesign"));
        let wt_ids: Vec<&str> = feature.sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(wt_ids, vec!["feature-nested", "feature-root"]);
        // Unmatched sessions stay in the project's flat list, newest first.
        let flat_ids: Vec<&str> = sec.sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(flat_ids, vec!["boundary-trap", "main-root"]);
    }

    #[test]
    fn project_without_worktrees_renders_flat_even_with_multiple_roots() {
        let projects = vec![project("owned", "Owned", "/repo-main")];
        let sessions = vec![
            session("a", "/repo-main", Some("/repo-main"), Some(owner("owned", "Owned")), None, 1, "2026-07-05T12:00:00Z"),
            session("b", "/repo-feature", Some("/repo-feature"), Some(owner("owned", "Owned")), None, 1, "2026-07-05T12:01:00Z"),
        ];

        let sections = group_for_panel(&sessions, &projects, None);

        assert_eq!(sections.len(), 1);
        assert!(sections[0].worktrees.is_none());
        assert_eq!(sections[0].sessions.len(), 2);
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
    #[test]
    fn zero_session_projects_render_as_empty_sections_after_populated() {
        let projects = vec![
            project("beta", "Beta", "/beta"),
            project("gamma", "Gamma", "/gamma"),
            project("blank", "", "/some/blank"),
        ];
        let sessions = vec![
            session("beta-sess", "/beta", Some("/beta"), None, None, 1, "2026-07-05T12:00:00Z"),
            session("other-sess", "/elsewhere", None, None, None, 1, "2026-07-05T12:05:00Z"),
        ];

        let sections = group_for_panel(&sessions, &projects, None);

        // Populated (beta) → zero-session projects alphabetical (blank, gamma) → Other last.
        let keys: Vec<&str> = sections.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, vec!["beta", "blank", "gamma", "__other__"]);

        // Zero-session project is an is_project section with empty sessions.
        let blank = sections.iter().find(|s| s.key == "blank").unwrap();
        assert!(blank.is_project);
        assert!(blank.sessions.is_empty());
        // Blank name falls back to the last `/` segment of workspace_root.
        assert_eq!(blank.label, "blank");

        // A project that already collected a session is not duplicated.
        assert_eq!(sections.iter().filter(|s| s.key == "beta").count(), 1);
    }

    #[test]
    fn project_root_from_parent_joins_parent_and_normalized_project_name() {
        assert_eq!(project_root_from_parent("~/dev", "Slop Check"), "~/dev/slop-check");
    }

    #[test]
    fn project_root_from_parent_trims_trailing_parent_slash() {
        assert_eq!(project_root_from_parent("~/dev/", "Slop Check"), "~/dev/slop-check");
    }

    #[test]
    fn project_root_from_parent_sanitizes_name_separators_into_single_folder() {
        for (name, expected_root) in [
            ("evil/../etc", "~/dev/evil-etc"),
            ("evil\\..\\etc", "~/dev/evil-etc"),
            ("!!!pwned!!!", "~/dev/pwned"),
            ("a...b", "~/dev/a-b"),
        ] {
            assert_eq!(project_root_from_parent("~/dev", name), expected_root, "name={name:?}");
        }
    }

    #[test]
    fn project_root_from_parent_blank_or_unusable_name_falls_back_to_project_name() {
        for name in ["", "   ", "!!!---///"] {
            assert_eq!(
                project_root_from_parent("~/dev", name),
                "~/dev/project-name",
                "name={name:?}"
            );
        }
    }
}
