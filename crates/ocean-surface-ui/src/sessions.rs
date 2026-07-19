//! Sessions panel — grouped by project, worktrees, with collapsible sections.
//!
//! Sessions are grouped under their owning project: authoritatively via the
//! daemon's `owning_project` when it serves that field (exact-root matches
//! today), else by matching the session's workspace root against the project
//! catalogue (`daemon.projects`) — exact root first, then component-boundary
//! prefix against each project's root and registered worktree paths (longest
//! root wins). Sessions with no matching project fall into an "Other" bucket.
//!
//! Within a project, sessions whose workspace root sits under one of the
//! project's registered worktrees (daemon-enriched `worktrees` on
//! `ProjectInfo`, component-boundary prefix match) group under that worktree
//! sub-row; the rest stay in the project's flat list.
//!
//! Every session returned by the daemon is shown, including zero-turn drafts,
//! matching the TUI's persisted-session discovery.

use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use std::collections::HashSet;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

use crate::daemon::{is_path_prefix, Daemon, ProjectInfo, SessionRunState, SessionSummary};
use futures_util::future::{abortable, AbortHandle};

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

/// Resolve the catalogue project that owns `root` when the daemon didn't
/// attach `owning_project` (worktree/subdir sessions, older daemons): exact
/// `workspace_root` match first, then component-boundary prefix against each
/// project's `workspace_root` and registered worktree paths. The LONGEST
/// matching root wins so nested project roots never steal each other's
/// sessions. Pure — unit-testable without WASM.
pub(crate) fn project_for_root<'a>(
    projects: &'a [ProjectInfo],
    root: &str,
) -> Option<&'a ProjectInfo> {
    if root.is_empty() {
        return None;
    }
    if let Some(p) = projects
        .iter()
        .find(|p| !p.workspace_root.is_empty() && p.workspace_root == root)
    {
        return Some(p);
    }
    projects
        .iter()
        .filter_map(|p| {
            let roots = std::iter::once(p.workspace_root.as_str())
                .chain(p.worktrees.iter().map(|wt| wt.path.as_str()));
            roots
                .filter(|r| !r.is_empty() && is_path_prefix(r, root))
                .map(str::len)
                .max()
                .map(|len| (len, p))
        })
        .max_by_key(|(len, _)| *len)
        .map(|(_, p)| p)
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

/// Derive a project name from a directory path: the last non-empty
/// segment after stripping a trailing slash. Falls back to "project"
/// when the path is blank or every segment is empty.
fn derive_project_name(path: &str) -> String {
    path.trim()
        .trim_end_matches('/')
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("project")
        .to_string()
}

/// One project section in the panel: a project header followed by its sessions,
/// optionally with worktree sub-groups when the daemon reports registered
/// worktrees for the project.
#[derive(Clone, Debug)]
pub(crate) struct ProjectSection {
    pub key: String,
    pub label: String,
    pub is_project: bool,
    /// True when the matched project has git state (branch or dirty flag).
    pub is_git: bool,
    /// Sessions not bucketed under any worktree or the main group
    /// (all sessions when `worktrees` is None).
    pub sessions: Vec<SessionSummary>,
    /// Sub-groups keyed by registered worktree path, set only when >=1 session matched.
    pub worktrees: Option<Vec<WorktreeGroup>>,
    /// Main-root nested group carved from unmatched leftovers after worktree
    /// matching — sessions whose root lives under the project's workspace_root.
    /// Only set when the project has registered worktrees and >=1 session qualifies.
    pub main_group: Option<WorktreeGroup>,
    /// Explicit cwd for this section's lazy new-session action. Catalogue root
    /// wins; otherwise a real session root is captured before bucketing. `None`
    /// suppresses the action rather than inheriting ambient cwd or using `/tmp`.
    pub new_session_cwd: Option<String>,
}

/// A worktree section inside a project: sessions sharing one workspace root.
#[derive(Clone, Debug)]
pub(crate) struct WorktreeGroup {
    pub root: String,
    pub branch: Option<String>,
    pub sessions: Vec<SessionSummary>,
}

/// Resolve the explicit cwd for a project section's new-session action.
/// Catalogue authority wins; daemon-owned orphan sections fall back to their
/// newest concrete session root. Empty projections return `None`, which keeps
/// the action absent rather than inheriting a prior session's cwd.
fn project_section_session_cwd(
    section: &ProjectSection,
    projects: &[ProjectInfo],
) -> Option<String> {
    projects
        .iter()
        .find(|project| project.id == section.key)
        .map(|project| project.workspace_root.trim())
        .filter(|root| !root.is_empty())
        .map(str::to_string)
        .or_else(|| {
            section
                .sessions
                .iter()
                .map(session_root)
                .map(str::trim)
                .find(|root| !root.is_empty())
                .map(str::to_string)
        })
}

/// Group sessions for the panel display: project-first, newest sessions
/// first, "Other" last. Every daemon-catalogued session is retained so the
/// desktop panel stays in parity with the TUI's persisted-session discovery.
/// Pure — no WASM, testable with `cargo test`.
pub(crate) fn group_for_panel(
    sessions: &[SessionSummary],
    projects: &[ProjectInfo],
    _active_id: Option<&str>,
) -> Vec<ProjectSection> {
    // The TUI includes persisted zero-turn drafts. Do not silently hide them
    // here: a session returned by the daemon belongs in the desktop catalogue.

    // Collect into sections by project membership.
    let mut sections: Vec<ProjectSection> = Vec::new();
    for s in sessions {
        let (key, label, is_proj) = if let Some(op) = &s.owning_project {
            let label = if op.name.trim().is_empty() {
                op.id.clone()
            } else {
                op.name.clone()
            };
            (op.id.clone(), label, true)
        } else if let Some(p) = project_for_root(projects, session_root(s)) {
            let label = if p.name.trim().is_empty() {
                p.workspace_root.clone()
            } else {
                p.name.clone()
            };
            (p.id.clone(), label, true)
        } else {
            ("__other__".to_string(), "Other".to_string(), false)
        };

        match sections
            .iter_mut()
            .find(|sec: &&mut ProjectSection| sec.key == key)
        {
            Some(sec) => sec.sessions.push(s.clone()),
            None => {
                let is_git = if is_proj {
                    projects
                        .iter()
                        .find(|p| p.id == key)
                        .is_some_and(|p| p.git_branch.is_some() || p.git_dirty.is_some())
                } else {
                    false
                };
                sections.push(ProjectSection {
                    key,
                    label,
                    is_project: is_proj,
                    is_git,
                    sessions: vec![s.clone()],
                    worktrees: None,
                    main_group: None,
                    new_session_cwd: None,
                })
            }
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
            is_git: p.git_branch.is_some() || p.git_dirty.is_some(),
            sessions: Vec::new(),
            worktrees: None,
            main_group: None,
            new_session_cwd: None,
        });
    }

    // Sort sessions inside each group: newest first (ISO-8601 lexicographic cmp),
    // then freeze the section's explicit new-session cwd before worktree/main
    // bucketing moves those sessions into nested groups.
    for sec in &mut sections {
        sec.sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        sec.new_session_cwd = project_section_session_cwd(sec, projects);
    }
    // Worktree + main-group bucketing: second pass inside each project section.
    // Sessions whose workspace root sits under a project worktree's path
    // (component-boundary prefix match) group under that sub-row — worktree
    // match always wins first so sessions never double-bucket. After matching,
    // unmatched sessions matching the project's workspace_root form the main
    // group; the rest remain flat strays. Zero worktrees → today's flat
    // rendering (no sub-groups, no main node).
    for sec in &mut sections {
        if !sec.is_project {
            continue;
        }
        let proj = projects.iter().find(|p| p.id == sec.key);
        let has_wts = proj.is_some_and(|p| !p.worktrees.is_empty());
        if !has_wts {
            continue;
        }
        let proj = proj.unwrap(); // safe: has_wts ensures Some
        let wts = &proj.worktrees;
        // Materialize every registered worktree up front. Empty worktrees are
        // still real project context and must remain visible in the drawer.
        let mut wt_groups: Vec<WorktreeGroup> = wts
            .iter()
            .map(|wt| WorktreeGroup {
                root: wt.path.clone(),
                branch: wt.branch.clone(),
                sessions: Vec::new(),
            })
            .collect();
        let mut unmatched: Vec<SessionSummary> = Vec::new();
        for s in &sec.sessions {
            let root = session_root(s);
            let matching = wts.iter().find(|wt| is_path_prefix(&wt.path, root));
            if let Some(wt) = matching {
                // Every registered worktree was seeded above, so a match always
                // resolves to an existing group.
                if let Some(group) = wt_groups.iter_mut().find(|g| g.root == wt.path) {
                    group.sessions.push(s.clone());
                }
            } else {
                unmatched.push(s.clone());
            }
        }
        // Carve main group from unmatched leftovers: sessions whose root lives
        // under the project's workspace_root form the main node; the rest are
        // true strays. Only applies when the project has registered worktrees.
        let workspace_root = proj.workspace_root.as_str();
        let mut main_sessions: Vec<SessionSummary> = Vec::new();
        let mut strays: Vec<SessionSummary> = Vec::new();
        for s in &unmatched {
            if is_path_prefix(workspace_root, session_root(s)) {
                main_sessions.push(s.clone());
            } else {
                strays.push(s.clone());
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
                        .cmp(
                            a.sessions
                                .first()
                                .map(|s| s.updated_at.as_str())
                                .unwrap_or(""),
                        ),
                }
            });
            sec.worktrees = Some(wt_groups);
        }
        if !main_sessions.is_empty() {
            main_sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            sec.main_group = Some(WorktreeGroup {
                root: proj.workspace_root.clone(),
                branch: proj.git_branch.clone(),
                sessions: main_sessions,
            });
        }
        sec.sessions = strays;
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
        let a_ts = section_newest_ts(a);
        let b_ts = section_newest_ts(b);
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

// --- section helpers (used by sort, For key, and collapse priming) ---
/// Newest session timestamp across flat, worktree, and main-group sessions.
/// Returns "" when the section is empty; ISO-8601 strings compare lexicographically.
pub(crate) fn section_newest_ts(sec: &ProjectSection) -> &str {
    let from_flat = sec.sessions.first().map(|s| s.updated_at.as_str());
    let from_main = sec
        .main_group
        .as_ref()
        .and_then(|mg| mg.sessions.first().map(|s| s.updated_at.as_str()));
    let from_wts = sec.worktrees.as_ref().and_then(|wts| {
        wts.iter()
            .filter_map(|wt| wt.sessions.first().map(|s| s.updated_at.as_str()))
            .max()
    });
    [from_flat, from_main, from_wts]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or("")
}

/// Whether any session in the section (including nested groups) has this id.
pub(crate) fn section_contains_session(sec: &ProjectSection, session_id: &str) -> bool {
    sec.sessions.iter().any(|s| s.id == session_id)
        || sec
            .main_group
            .as_ref()
            .is_some_and(|mg| mg.sessions.iter().any(|s| s.id == session_id))
        || sec.worktrees.as_ref().is_some_and(|wts| {
            wts.iter()
                .any(|wt| wt.sessions.iter().any(|s| s.id == session_id))
        })
}

/// Total session count across flat, main-group, and worktree sessions.
pub(crate) fn section_total_sessions(sec: &ProjectSection) -> usize {
    let nested: usize = sec
        .main_group
        .as_ref()
        .map(|mg| mg.sessions.len())
        .unwrap_or(0)
        + sec
            .worktrees
            .as_ref()
            .map(|wts| wts.iter().map(|wt| wt.sessions.len()).sum())
            .unwrap_or(0);
    sec.sessions.len() + nested
}

/// Deterministic bucket-layout signature so a flat→main/worktree recategorization
/// changes the `<For>` key even when total count and newest timestamp are
/// identical. Encodes the shape of sessions within a project section: number of
/// flat sessions, main-group sessions (if any), and per-worktree session counts.
pub(crate) fn section_layout_signature(sec: &ProjectSection) -> String {
    let flat = sec.sessions.len();
    let main_count = sec
        .main_group
        .as_ref()
        .map(|mg| mg.sessions.len())
        .unwrap_or(0);
    let wt_sig: Vec<String> = sec
        .worktrees
        .as_ref()
        .map(|wts| {
            wts.iter()
                .map(|wt| format!("{}:{}", wt.root, wt.sessions.len()))
                .collect()
        })
        .unwrap_or_default();
    let wt_segments = if wt_sig.is_empty() {
        String::new()
    } else {
        format!("|W_{}", wt_sig.join("_"))
    };
    let main_segment = if sec.main_group.is_some() {
        format!("|M{}", main_count)
    } else {
        String::new()
    };
    format!("F{}{}{}", flat, main_segment, wt_segments)
}

/// Content-sensitive identity for a rendered project section. Leptos `<For>`
/// retains the child view for an unchanged key, so both the user-visible label
/// and explicit new-session cwd participate: daemon-side project rename/root
/// changes must replace captured actions even when contents stay unchanged.
pub(crate) fn section_render_key(sec: &ProjectSection) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        sec.key,
        sec.label,
        section_total_sessions(sec),
        section_newest_ts(sec),
        section_layout_signature(sec),
        sec.new_session_cwd.as_deref().unwrap_or("<none>"),
    )
}

// ---------------------------------------------------------------------------
// Collapse priming decider (pure) — one open section per panel generation
// ---------------------------------------------------------------------------
// Extracted so the "which section is expanded by default" decision can be
// unit-tested without the Leptos runtime. Session and project-catalogue fetches
// settle independently, so an early grouping may not yet contain the active
// session's final project section. Latching the default on the first nonempty
// snapshot (the old behavior) could lock an unrelated newest section open while
// the active session's section stayed collapsed after catalogue regrouping.

/// Whether priming has settled for the current panel-open generation. Priming
/// only ever *locks* the no-active fallback (`Settled(None)`) so a later poll
/// cannot make the default jump to a different newest section. When an active
/// session exists, priming stays `Pending` and keeps re-evaluating so the
/// default follows the active session onto its final section as the catalogue
/// settles.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PrimedFor {
    /// Not settled this generation — the default is still eligible to move.
    Pending,
    /// Settled for this active session id (`None` = the no-active fallback).
    Settled(Option<String>),
}

/// Priming state carried across regroupings within one panel-open generation.
/// A fresh `default()` marks a new generation (panel close→reopen resets it).
#[derive(Clone, Debug, PartialEq, Eq)]
struct CollapsePrimingState {
    primed: PrimedFor,
    /// Set once the user toggles any section; user state then wins over all
    /// further priming for the rest of this generation.
    user_toggled: bool,
}

impl Default for CollapsePrimingState {
    fn default() -> Self {
        Self {
            primed: PrimedFor::Pending,
            user_toggled: false,
        }
    }
}

/// Outcome of one priming pass: the collapsed set to apply (or `None` to leave
/// collapse state untouched) plus the state to carry into the next pass.
struct CollapsePlan {
    collapsed: Option<HashSet<String>>,
    next: CollapsePrimingState,
}

impl CollapsePlan {
    fn idle(state: &CollapsePrimingState) -> Self {
        Self {
            collapsed: None,
            next: state.clone(),
        }
    }
}

/// Decide the default collapse set for a priming pass. Pure — no WASM.
///
/// - The user owning collapse state (after a toggle) or an already-settled
///   no-active fallback for the same active id yields an idle plan.
/// - With an active session located in the layout, its section(s) expand and
///   priming stays `Pending` so a later catalogue regroup can move the default.
/// - With no active session at all, the newest section expands and priming
///   settles so subsequent polls cannot make the default jump.
/// - With an active id present but not yet in the layout, the newest section
///   expands provisionally and priming stays `Pending` until the active session
///   materializes.
fn plan_collapse_priming(
    state: &CollapsePrimingState,
    sections: &[ProjectSection],
    active_id: Option<&str>,
) -> CollapsePlan {
    // User owns collapse state for the rest of this generation.
    if state.user_toggled {
        return CollapsePlan::idle(state);
    }
    // Already settled for this exact active id — locked until it changes or the
    // generation resets.
    if let PrimedFor::Settled(settled) = &state.primed {
        if settled.as_deref() == active_id {
            return CollapsePlan::idle(state);
        }
    }
    // No layout yet — stay eligible, touch nothing.
    if sections.is_empty() {
        return CollapsePlan::idle(state);
    }

    let active_keys: Vec<&str> = active_id
        .map(|id| {
            sections
                .iter()
                .filter(|sec| section_contains_session(sec, id))
                .map(|sec| sec.key.as_str())
                .collect()
        })
        .unwrap_or_default();

    // Expand the active session's section(s); otherwise fall back to the first
    // (newest) section. Only the no-active fallback locks — an active session
    // keeps priming eligible so the default follows catalogue regrouping.
    let expanded: Vec<&str> = if !active_keys.is_empty() {
        active_keys
    } else {
        vec![sections[0].key.as_str()]
    };
    let settle = active_id.is_none();

    let collapsed: HashSet<String> = sections
        .iter()
        .filter(|sec| !expanded.contains(&sec.key.as_str()))
        .map(|sec| sec.key.clone())
        .collect();

    let primed = if settle {
        PrimedFor::Settled(active_id.map(str::to_string))
    } else {
        PrimedFor::Pending
    };

    CollapsePlan {
        collapsed: Some(collapsed),
        next: CollapsePrimingState {
            primed,
            user_toggled: false,
        },
    }
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
                let badge = if tag == "?" {
                    None
                } else {
                    Some(tag.to_ascii_lowercase())
                };
                return (badge, body.to_string());
            }
        }
    }
    (None, t.to_string())
}

// ---------------------------------------------------------------------------
// Poll lifecycle deciders (Stream A1, v6) — production helpers
// ---------------------------------------------------------------------------
// Called from the poll rail at every guard site. Extracted so lifecycle
// scenarios can be tested without a browser.

/// Guard for the write site called before every `session_list.set()`.
/// Enforces captured generation match so a stale task cannot write after
/// close/reopen. Also checks the panel is still open (defensive: a settle
/// after close should never land).
fn poll_guard_write(current_gen: u64, my_gen: u64, panel_open: bool) -> bool {
    current_gen == my_gen && panel_open
}

/// Guard for the tick site: skip if in_flight or gen changed.
fn poll_should_skip(current_gen: u64, my_gen: u64, in_flight: bool) -> bool {
    in_flight || current_gen != my_gen
}

/// ONE shared release guard called after every outcome (Ok fresh, Ok error,
/// Err aborted). Only clears in_flight if this task still owns it — gen match
/// AND still in_flight. All three outcomes in the production poll rail call
/// this once, after the match.
fn poll_release_in_flight(current_gen: u64, my_gen: u64, in_flight: bool) -> bool {
    current_gen == my_gen && in_flight
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

    // ── Abortable poll rail (Stream A1, v6) ──────────────────────────
    // Every fetch is wrapped in abortable(); the AbortHandle is stored so
    // stop() can cancel in-flight HTTP. Gen counter prevents stale writes;
    // in_flight prevents concurrent ticks. Shared stop_polling aborts the
    // fetch + clears the interval + bumps gen + clears in_flight, and is
    // called on close, reopen, and on_cleanup (destruction).
    //
    // IntervalHandle is leptos::helpers::IntervalHandle — a Copy i32 wrapper
    // with clear(). Created via set_interval_with_handle(fn, Duration).
    // No raw Closure/i32/forget — leptos owns the JS-side lifecycle.
    let poll_gen: RwSignal<u64> = RwSignal::new(0);
    let poll_in_flight: RwSignal<bool> = RwSignal::new(false);
    let poll_fetch_abort: RwSignal<Option<AbortHandle>> = RwSignal::new(None);
    let poll_interval: RwSignal<Option<IntervalHandle>> = RwSignal::new(None);

    // Shared stop: abort in-flight fetch + clear interval + bump gen + clear
    // in_flight. Called on close, reopen (before starting a new cycle), and
    // on_cleanup (component destruction).
    let stop_polling = {
        move || {
            // Abort any in-flight fetch so it can't write after close.
            if let Some(h) = poll_fetch_abort.get_untracked() {
                h.abort(); // AbortHandle::abort is idempotent
                poll_fetch_abort.set(None);
            }
            // Clear the interval handle — no more ticks.
            if let Some(handle) = poll_interval.get_untracked() {
                handle.clear();
                poll_interval.set(None);
            }
            // Bump generation: all in-flight writes from the old cycle are
            // now stale and will be rejected by the gen guard.
            poll_gen.update(|g| *g += 1);
            // Clear in_flight so reopen doesn't see a poisoned flag.
            poll_in_flight.set(false);
        }
    };

    // The closure captures only Copy signals — it is Copy itself.
    // Just assign; no clone needed.
    let stop_for_effect = stop_polling;

    // Initial fetch + open/close lifecycle.
    Effect::new(move |_| {
        let d = daemon.get_value();
        if open.get() {
            // Stop any prior cycle (singleton).
            stop_for_effect();

            // Seed project catalogue.
            d.fetch_projects();

            // Fresh generation: any prior in-flight writes become stale.
            poll_gen.update(|g| *g += 1);
            let my_gen = poll_gen.get_untracked();

            // ── Immediate abortable fetch ──
            // Only the fetch itself is wrapped in abortable(). The outer
            // spawn_local matches all outcomes, then calls ONE unified
            // poll_release_in_flight guard after the match.
            poll_in_flight.set(true);
            let fetch_ticket = d.claim_session_fetch_ticket();
            let d_immediate = d.clone();
            let d_commit = d.clone();
            let (immediate_fut, immediate_handle) =
                abortable(async move { d_immediate.fetch_all_sessions().await });
            poll_fetch_abort.set(Some(immediate_handle));
            spawn_local(async move {
                let outcome = immediate_fut.await;
                match outcome {
                    Ok(Ok(fresh)) => {
                        if poll_guard_write(poll_gen.get_untracked(), my_gen, open.get_untracked())
                        {
                            let _ = d_commit.store_sessions_if_current(fetch_ticket, fresh);
                        }
                    }
                    Ok(Err(e)) => log::warn!("sessions poll error: {e}"),
                    Err(_aborted) => { /* no-op: fetch never ran */ }
                }
                // ONE captured-gen ownership guard after every outcome.
                // Passes the actual in_flight value, not a literal.
                if poll_release_in_flight(
                    poll_gen.get_untracked(),
                    my_gen,
                    poll_in_flight.get_untracked(),
                ) {
                    poll_in_flight.set(false);
                }
            });

            // ── 2-second Interval tick rail ──
            // Uses leptos::helpers::set_interval_with_handle. IntervalHandle
            // is a Copy i32 wrapper — store directly in RwSignal. No raw
            // Closure/i32/forget.
            let d_tick = d.clone();
            match set_interval_with_handle(
                move || {
                    // Skip if handle was cleared (stop_polling ran).
                    if poll_interval.get_untracked().is_none() {
                        return;
                    }
                    // Skip if a prior tick is still running or gen changed.
                    if poll_should_skip(
                        poll_gen.get_untracked(),
                        my_gen,
                        poll_in_flight.get_untracked(),
                    ) {
                        return;
                    }
                    poll_in_flight.set(true);
                    let fetch_ticket = d_tick.claim_session_fetch_ticket();
                    let d_fetch = d_tick.clone();
                    let d_commit = d_tick.clone();
                    let (tick_fut, tick_handle) =
                        abortable(async move { d_fetch.fetch_all_sessions().await });
                    poll_fetch_abort.set(Some(tick_handle));
                    spawn_local(async move {
                        let outcome = tick_fut.await;
                        match outcome {
                            Ok(Ok(fresh)) => {
                                if poll_guard_write(
                                    poll_gen.get_untracked(),
                                    my_gen,
                                    open.get_untracked(),
                                ) {
                                    let _ = d_commit.store_sessions_if_current(fetch_ticket, fresh);
                                }
                            }
                            Ok(Err(e)) => log::warn!("sessions poll error: {e}"),
                            Err(_aborted) => { /* no-op */ }
                        }
                        if poll_release_in_flight(
                            poll_gen.get_untracked(),
                            my_gen,
                            poll_in_flight.get_untracked(),
                        ) {
                            poll_in_flight.set(false);
                        }
                    });
                },
                std::time::Duration::from_secs(2),
            ) {
                Ok(handle) => poll_interval.set(Some(handle)),
                Err(e) => log::error!("sessions poll interval creation failed: {:?}", e),
            }
        } else {
            stop_for_effect();
        }
    });

    // on_cleanup: stop everything when the component unmounts.
    on_cleanup(stop_polling);

    // Derived: grouped sections (recomputes on session_list or projects change).
    let sections = move || {
        group_for_panel(
            &session_list.get(),
            &projects.get(),
            current_id.get().as_deref(),
        )
    };

    // Collapse state: default to one open section (active project, else newest
    // section) as soon as panel data materializes. Priming is generation-aware
    // per panel open — it stays eligible to re-evaluate the default until the
    // active session is located (or the no-active fallback settles), and a
    // panel close→reopen resets the generation. The first user toggle wins for
    // the rest of that generation. See `plan_collapse_priming` for the decider.
    let collapsed: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let priming_state: RwSignal<CollapsePrimingState> =
        RwSignal::new(CollapsePrimingState::default());
    let priming_prev_open: RwSignal<bool> = RwSignal::new(false);
    Effect::new(move |_| {
        let is_open = open.get();
        // Detect a close→reopen transition and start a fresh priming generation.
        let was_open = priming_prev_open.get_untracked();
        priming_prev_open.set(is_open);
        if is_open && !was_open {
            priming_state.set(CollapsePrimingState::default());
        }
        if !is_open {
            return;
        }

        let active_id = current_id.get();
        let secs = group_for_panel(&session_list.get(), &projects.get(), active_id.as_deref());
        let plan =
            plan_collapse_priming(&priming_state.get_untracked(), &secs, active_id.as_deref());
        if let Some(next_collapsed) = plan.collapsed {
            collapsed.set(next_collapsed);
        }
        priming_state.set(plan.next);
    });

    let is_open = move || open.get();
    let is_collapsed = move |key: &str| collapsed.get().contains(key);
    let toggle = move |key: String| {
        // First user toggle hands collapse ownership to the user for the rest of
        // this panel-open generation, so later regroupings never re-prime.
        priming_state.update(|st| st.user_toggled = true);
        collapsed.update(|set| {
            if set.contains(&key) {
                set.remove(&key);
            } else {
                set.insert(key);
            }
        });
    };

    let is_empty = move || sections().is_empty();

    // Project-action mode: exactly one form is visible at a time.
    #[derive(Clone, Copy, PartialEq)]
    enum ProjectAction {
        None,
        NewProject,
        ExistingProject,
    }

    let action_mode: RwSignal<ProjectAction> = RwSignal::new(ProjectAction::None);
    // New-project form.
    let new_name: RwSignal<String> = RwSignal::new(String::new());
    let new_parent: RwSignal<String> = RwSignal::new(String::new());
    // Existing-project form.
    let existing_path: RwSignal<String> = RwSignal::new(String::new());

    Effect::new(move |_| {
        if !open.get() {
            if daemon.get_value().project_create_pending.get_untracked() {
                open.set(true);
                return;
            }
            daemon.get_value().project_create_error.set(None);
            action_mode.set(ProjectAction::None);
        }
    });

    let start_chat = move |_| {
        daemon.get_value().begin_chat_session();
        open.set(false);
    };

    let cancel_project = move || {
        if daemon.get_value().project_create_pending.get_untracked() {
            return;
        }
        daemon.get_value().project_create_error.set(None);
        action_mode.set(ProjectAction::None);
        // Return focus to the reveal button once <Show> remounts it.
        if let Some(win) = web_sys::window() {
            let cb = wasm_bindgen::closure::Closure::once_into_js(move || {
                if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                    if let Some(el) = doc
                        .query_selector("button.sessions-create__reveal")
                        .ok()
                        .flatten()
                    {
                        let _ = el.unchecked_into::<web_sys::HtmlElement>().focus();
                    }
                }
            });
            let _ =
                win.set_timeout_with_callback_and_timeout_and_arguments_0(cb.unchecked_ref(), 0);
        }
    };

    // ── Focus management for project-action forms ──────────────────────
    let existing_input_ref: NodeRef<leptos::html::Input> = NodeRef::new();
    let new_input_ref: NodeRef<leptos::html::Input> = NodeRef::new();

    // Auto-focus the first input when a project-action form opens.
    // NodeRef is populated after <Show> mounts its children, so the
    // effect that fires on action_mode change will find the element.
    Effect::new(move |_| {
        let mode = action_mode.get();
        let target = match mode {
            ProjectAction::ExistingProject => existing_input_ref.get(),
            ProjectAction::NewProject => new_input_ref.get(),
            _ => None,
        };
        if let Some(el) = target {
            let _ = el.unchecked_into::<web_sys::HtmlElement>().focus();
        }
    });
    let create_new_project = move |ev: SubmitEvent| {
        ev.prevent_default();
        if daemon.get_value().project_create_pending.get_untracked() {
            return;
        }
        let name = new_name.get_untracked();
        let parent = new_parent.get_untracked();
        let parent = if parent.trim().is_empty() {
            "~/dev".to_string()
        } else {
            parent.trim().to_string()
        };
        let root = project_root_from_parent(&parent, &name);
        daemon.get_value().create_project(name, root);
    };

    let add_existing_project = move |ev: SubmitEvent| {
        ev.prevent_default();
        if daemon.get_value().project_create_pending.get_untracked() {
            return;
        }
        let path = existing_path.get_untracked().trim().to_string();
        let name = derive_project_name(&path);
        daemon.get_value().create_project(name, path);
    };

    let new_can_submit = move || !new_name.get().trim().is_empty();
    let existing_can_submit = move || !existing_path.get().trim().is_empty();

    // Create-project success closes the modal and clears forms.
    let prev_pending: RwSignal<bool> = RwSignal::new(false);
    Effect::new(move |_| {
        let pending = daemon.get_value().project_create_pending.get();
        let was_pending = prev_pending.get_untracked();
        prev_pending.set(pending);
        if was_pending
            && !pending
            && daemon
                .get_value()
                .project_create_error
                .get_untracked()
                .is_none()
        {
            open.set(false);
            new_name.set(String::new());
            new_parent.set(String::new());
            existing_path.set(String::new());
            action_mode.set(ProjectAction::None);
        }
    });

    view! {
        <div
            class="sessions-overlay"
            class:sessions-overlay--open=is_open
            hidden=move || !open.get()
            on:click=move |ev| {
                if daemon.get_value().project_create_pending.get_untracked() {
                    return;
                }
                let target = event_target::<web_sys::HtmlElement>(&ev);
                if target.class_list().contains("sessions-overlay") {
                    open.set(false);
                }
            }
        >
            <div class="sessions-panel ocean-lit" role="dialog" aria-modal="true" aria-label="Sessions"
                on:keydown=move |ev: web_sys::KeyboardEvent| {
                    if ev.key() == "Escape" {
                        // This panel owns the bubbled Escape. Stop it even while
                        // project creation is pending so the window rail cannot
                        // close Sessions and then a second reveal on one key.
                        ev.stop_propagation();
                        if !daemon.get_value().project_create_pending.get_untracked() {
                            ev.prevent_default();
                            open.set(false);
                        }
                    }
                }
            >
                <div class="sessions-panel__head">
                    <h2 class="sessions-panel__title">"Sessions"</h2>
                    <button
                        class="sessions-panel__close"
                        type="button"
                        aria-label="close sessions panel"
                        disabled=move || daemon.get_value().project_create_pending.get()
                        on:click=move |_| {
                            if !daemon.get_value().project_create_pending.get_untracked() {
                                open.set(false);
                            }
                        }
                    >
                        "✕"
                    </button>
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
                            section_render_key(sec)
                        }
                        children=move |sec: ProjectSection| {
                            let skey = sec.key.clone();
                            let s_is_project = sec.is_project;
                            let s_is_git = sec.is_git;
                            let s_label = sec.label.clone();
                            let s_count = section_total_sessions(&sec);
                            let worktrees = sec.worktrees.clone();
                            let main_group = sec.main_group.clone();
                            let flattened = sec.sessions.clone();
                            let glyph_key = skey.clone();
                            let show_key = skey.clone();
                            let click_key = skey.clone();
                            let new_key = skey.clone();
                            let new_session_cwd = sec.new_session_cwd.clone();
                            let can_start_project_session =
                                s_is_project && new_session_cwd.is_some();

                            let body_id = format!("sessions-group-body-{}", skey);
                            let aria_key = glyph_key.clone();
                            let glyph = move || if is_collapsed(&glyph_key) { "▸" } else { "▾" };

                            view! {
                                <div class="sessions-group">
                                    <div class="sessions-group__bar">
                                        // ── Group header ────────────────────
                                        <button
                                            class="sessions-group__head"
                                            class:sessions-group__head--other=!s_is_project
                                            type="button"
                                            aria-expanded=move || (!is_collapsed(&aria_key)).to_string()
                                            aria-controls=body_id.clone()
                                            on:click={
                                                let k = click_key.clone();
                                                move |_| toggle(k.clone())
                                            }
                                        >
                                            // Project folder/git mark or the Other-bucket glyph.
                                            {if s_is_project {
                                                if s_is_git {
                                                    view! {
                                                        <span class="project-logo"><crate::icons::GitBranch /></span>
                                                    }.into_any()
                                                } else {
                                                    view! {
                                                        <span class="project-logo"><crate::icons::Folder /></span>
                                                    }.into_any()
                                                }
                                            } else {
                                                view! {
                                                    <span class="project-logo project-logo--other">"⋯"</span>
                                                }.into_any()
                                            }}

                                            <span class="sessions-group__label">{s_label.clone()}</span>
                                            <span class="sessions-group__glyph" aria-hidden="true">{glyph}</span>
                                            <span class="sessions-group__count">{s_count}</span>
                                        </button>

                                        // ── Per-project new session (real projects only) ──
                                        <Show when=move || can_start_project_session>
                                            <button
                                                class="sessions-group__new-btn"
                                                type="button"
                                                on:click={
                                                    let k = new_key.clone();
                                                    let cwd = new_session_cwd.clone();
                                                    move |_| {
                                                        if let Some(cwd) = cwd.clone() {
                                                            daemon
                                                                .get_value()
                                                                .begin_project_session(k.clone(), cwd);
                                                            open.set(false);
                                                        }
                                                    }
                                                }
                                            >
                                                "+ new session"
                                            </button>
                                        </Show>
                                    </div>

                                    // ── Expanded body ───────────────────────
                                    <Show when=move || !is_collapsed(&show_key)>
                                        <div class="sessions-group__body" id=body_id.clone()>
                                            {if worktrees.is_some() || main_group.is_some() {
                                                let mut out: Vec<_> = Vec::new();
                                                // Main-root group first.
                                                if let Some(mg) = main_group.clone() {
                                                    let rows = mg.sessions.clone();
                                                    let count = rows.len();
                                                    let root_label = mg.root.split('/').next_back()
                                                        .filter(|s| !s.is_empty())
                                                        .unwrap_or("main")
                                                        .to_string();
                                                    let branch_label = mg.branch.clone().filter(|b| !b.is_empty());
                                                    let has_branch = branch_label.is_some();
                                                    let branch_text = branch_label.unwrap_or_default();
                                                    out.push(view! {
                                                        <div class="worktree-group worktree-group--main">
                                                            <div class="worktree-group__head">
                                                                <span class="worktree-group__root">{root_label}</span>
                                                                {if has_branch {
                                                                    view! { <span class="worktree-group__branch"><crate::icons::GitBranch />{branch_text}</span> }.into_any()
                                                                } else {
                                                                    view! { <span class="worktree-group__branch worktree-group__branch--hidden"></span> }.into_any()
                                                                }}
                                                                <span class="worktree-group__count">{count}</span>
                                                            </div>
                                                            {rows.into_iter().map(|s| session_row(s, daemon, open, current_id, false, None).into_any()).collect::<Vec<_>>()}
                                                        </div>
                                                    }.into_any());
                                                }
                                                // Worktree sub-groups.
                                                if let Some(wts) = worktrees.clone() {
                                                    out.extend(wts.into_iter().map(|wt: WorktreeGroup| {
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
                                                    }));
                                                }
                                                // True strays below groups.
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

                <div class="sessions-panel__actions">
                    <button class="sessions-panel__new-btn" type="button" on:click=start_chat>
                        "New chat"
                    </button>

                    <Show when=move || action_mode.get() == ProjectAction::None>
                        <button
                            class="sessions-create__reveal"
                            type="button"
                            on:click=move |_| {
                                daemon.get_value().project_create_error.set(None);
                                action_mode.set(ProjectAction::ExistingProject);
                            }
                        >
                            "Existing project"
                        </button>
                        <button
                            class="sessions-create__reveal"
                            type="button"
                            on:click=move |_| {
                                daemon.get_value().project_create_error.set(None);
                                new_parent.set("~/dev".to_string());
                                action_mode.set(ProjectAction::NewProject);
                            }
                        >
                            "+ New project"
                        </button>
                    </Show>

                    <Show when=move || action_mode.get() == ProjectAction::ExistingProject>
                        <form class="sessions-create" on:submit=add_existing_project>
                            <div class="sessions-create__head">
                                <span class="sessions-create__head-title">"Existing project"</span>
                                <button class="sessions-create__cancel" type="button" disabled=move || daemon.get_value().project_create_pending.get() on:click=move |_| cancel_project()>
                                    "Cancel"
                                </button>
                            </div>
                            <div class="sessions-create__inputs">
                                <input
                                    node_ref=existing_input_ref
                                    class="sessions-create__input"
                                    type="text"
                                    placeholder="Existing project folder path"
                                    aria-label="Existing project folder path"
                                    autocomplete="off"
                                    spellcheck="false"
                                    prop:value=move || existing_path.get()
                                    on:input=move |ev| existing_path.set(event_target_value(&ev))
                                />
                            </div>
                            <button
                                class="sessions-create__btn"
                                type="submit"
                                disabled=move || !existing_can_submit()
                                    || daemon.get_value().project_create_pending.get()
                            >
                                {move || if daemon.get_value().project_create_pending.get() {
                                    "Adding…"
                                } else {
                                    "Add project"
                                }}
                            </button>
                            <Show when=move || daemon.get_value().project_create_error.get().is_some()>
                                <div class="sessions-create__error">
                                    {move || daemon.get_value().project_create_error.get().unwrap_or_default()}
                                </div>
                            </Show>
                        </form>
                    </Show>

                    <Show when=move || action_mode.get() == ProjectAction::NewProject>
                        <form class="sessions-create" on:submit=create_new_project>
                            <div class="sessions-create__head">
                                <span class="sessions-create__head-title">"New project"</span>
                                <button class="sessions-create__cancel" type="button" disabled=move || daemon.get_value().project_create_pending.get() on:click=move |_| cancel_project()>
                                    "Cancel"
                                </button>
                            </div>
                            <div class="sessions-create__inputs">
                                <input
                                    node_ref=new_input_ref
                                    class="sessions-create__input"
                                    type="text"
                                    placeholder="Project name"
                                    aria-label="Project name"
                                    autocomplete="off"
                                    prop:value=move || new_name.get()
                                    on:input=move |ev| new_name.set(event_target_value(&ev))
                                />
                                <input
                                    class="sessions-create__input"
                                    type="text"
                                    placeholder="Parent folder (default: ~/dev)"
                                    aria-label="Parent folder"
                                    autocomplete="off"
                                    spellcheck="false"
                                    prop:value=move || new_parent.get()
                                    on:input=move |ev| new_parent.set(event_target_value(&ev))
                                />
                            </div>
                            <button
                                class="sessions-create__btn"
                                type="submit"
                                disabled=move || !new_can_submit()
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

            </div>
        </div>
    }
}

/// Derive dot `data-state` for a session row. v4.5 contract:
/// `permission` > `cancelling` > `running` > `recent` (≤10 min) > `idle`.
fn dot_state(active_state: Option<SessionRunState>, fresh: bool) -> &'static str {
    match active_state {
        Some(SessionRunState::WaitingForPermission) => "permission",
        Some(SessionRunState::Cancelling) => "cancelling",
        Some(SessionRunState::Running) => "running",
        _ => {
            if fresh {
                "recent"
            } else {
                "idle"
            }
        }
    }
}

/// aria-label for the session dot based on its `data-state`.
fn dot_label(state: &str) -> &'static str {
    match state {
        "permission" => "Waiting for permission",
        "cancelling" => "Cancelling",
        "running" => "Running",
        "recent" => "Recent activity",
        _ => "Idle",
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

    // Dot state: computed once from active_state + freshness.
    let dot_state_val = dot_state(session.active_state, fresh);
    let dot_aria = dot_label(dot_state_val);

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
                <span
                    class="sessions-item__dot"
                    data-state=dot_state_val
                    role="img"
                    aria-label=dot_aria
                ></span>
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
            active_turn: None,
            active_state: None,
            updated_at: updated_at.to_string(),
        }
    }

    #[test]
    fn zero_turn_sessions_remain_visible_for_tui_parity() {
        let projects = vec![project("repo", "Repo", "/repo")];
        let sessions = vec![
            session(
                "active-zero",
                "/repo",
                Some("/repo"),
                None,
                None,
                0,
                "2026-07-05T12:02:00Z",
            ),
            session(
                "stale-zero",
                "/repo",
                Some("/repo"),
                None,
                None,
                0,
                "2026-07-05T12:01:00Z",
            ),
            session(
                "with-turn",
                "/repo",
                Some("/repo"),
                None,
                None,
                1,
                "2026-07-05T12:00:00Z",
            ),
        ];

        let sections = group_for_panel(&sessions, &projects, Some("active-zero"));

        assert_eq!(sections.len(), 1);
        let ids: Vec<&str> = sections[0].sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["active-zero", "stale-zero", "with-turn"]);
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
            session(
                "workspace-match",
                "/ignored",
                Some("/repo"),
                None,
                None,
                1,
                "2026-07-05T12:01:00Z",
            ),
            session(
                "cwd-match",
                "/repo",
                None,
                None,
                None,
                1,
                "2026-07-05T12:00:00Z",
            ),
            session(
                "unmatched-newest",
                "/elsewhere",
                None,
                None,
                None,
                1,
                "2026-07-05T12:02:00Z",
            ),
        ];

        let sections = group_for_panel(&sessions, &projects, None);

        let keys: Vec<&str> = sections
            .iter()
            .map(|section| section.key.as_str())
            .collect();
        assert_eq!(keys, vec!["matched", "__other__"]);
        let matched_ids: Vec<&str> = sections[0]
            .sessions
            .iter()
            .map(|session| session.id.as_str())
            .collect();
        assert_eq!(matched_ids, vec!["workspace-match", "cwd-match"]);
        assert_eq!(sections[1].sessions[0].id, "unmatched-newest");
    }

    #[test]
    fn subdir_and_worktree_roots_group_under_their_project() {
        let projects = vec![project_with_worktrees(
            "owned",
            "Owned",
            "/repo",
            &[("/wt/repo-feature", Some("feature/x"))],
        )];
        let sessions = vec![
            session(
                "subdir",
                "/repo/crates/ui",
                Some("/repo/crates/ui"),
                None,
                None,
                1,
                "2026-07-05T12:02:00Z",
            ),
            session(
                "worktree",
                "/wt/repo-feature/src",
                None,
                None,
                None,
                1,
                "2026-07-05T12:01:00Z",
            ),
            session(
                "sibling-name",
                "/repo-x",
                None,
                None,
                None,
                1,
                "2026-07-05T12:00:00Z",
            ),
        ];

        let sections = group_for_panel(&sessions, &projects, None);

        let keys: Vec<&str> = sections.iter().map(|sec| sec.key.as_str()).collect();
        assert_eq!(keys, vec!["owned", "__other__"]);
        // Main-root subdirectories carve into the project's main group; the
        // worktree-rooted session lands in the registered-worktree sub-group.
        let main = sections[0]
            .main_group
            .as_ref()
            .expect("main group expected");
        assert_eq!(main.root, "/repo");
        assert_eq!(main.sessions[0].id, "subdir");
        assert!(sections[0].sessions.is_empty());
        let wts = sections[0].worktrees.as_ref().unwrap();
        assert_eq!(wts.len(), 1);
        assert_eq!(wts[0].root, "/wt/repo-feature");
        assert_eq!(wts[0].sessions[0].id, "worktree");
        // `/repo-x` shares the string prefix but not a path component — Other.
        assert_eq!(sections[1].sessions[0].id, "sibling-name");
    }

    #[test]
    fn nested_project_roots_prefer_longest_match() {
        let projects = vec![
            project("outer", "Outer", "/mono"),
            project("inner", "Inner", "/mono/apps/web"),
        ];
        let sessions = vec![session(
            "deep",
            "/mono/apps/web/src",
            None,
            None,
            None,
            1,
            "2026-07-05T12:00:00Z",
        )];

        let sections = group_for_panel(&sessions, &projects, None);

        // Longest matching root wins: the session belongs to the inner
        // project; outer still renders as an empty catalogue section.
        let inner = sections.iter().find(|s| s.key == "inner").unwrap();
        assert_eq!(inner.sessions[0].id, "deep");
        let outer = sections.iter().find(|s| s.key == "outer").unwrap();
        assert!(outer.sessions.is_empty());
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
        let worktrees = sec
            .worktrees
            .as_ref()
            .expect("worktree sub-groups expected");
        assert_eq!(worktrees.len(), 1);
        let feature = &worktrees[0];
        assert_eq!(feature.root, "/repo-feature");
        assert_eq!(feature.branch.as_deref(), Some("feature/redesign"));
        let wt_ids: Vec<&str> = feature.sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(wt_ids, vec!["feature-nested", "feature-root"]);
        // Sessions matching workspace_root carve into main_group; true strays stay flat.
        let main = sec.main_group.as_ref().expect("main group expected");
        assert_eq!(main.root, "/repo-main");
        assert!(main.branch.is_none());
        let main_ids: Vec<&str> = main.sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(main_ids, vec!["main-root"]);
        // True strays (not matching any worktree or the workspace root) stay flat.
        let stray_ids: Vec<&str> = sec.sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(stray_ids, vec!["boundary-trap"]);
        assert!(section_contains_session(sec, "main-root"));
        assert!(section_contains_session(sec, "feature-nested"));
        assert!(!section_contains_session(sec, "missing"));
        assert_eq!(section_total_sessions(sec), 4);
        assert_eq!(section_newest_ts(sec), "2026-07-05T12:03:00Z");
    }

    #[test]
    fn registered_worktrees_render_even_without_sessions() {
        let projects = vec![project_with_worktrees(
            "owned",
            "Owned",
            "/repo-main",
            &[
                ("/repo-feature", Some("feature/redesign")),
                ("/repo-empty", Some("feature/empty")),
            ],
        )];
        let sessions = vec![session(
            "feature-root",
            "/repo-feature",
            Some("/repo-feature"),
            Some(owner("owned", "Owned")),
            Some("feature/redesign"),
            1,
            "2026-07-05T12:01:00Z",
        )];

        let sections = group_for_panel(&sessions, &projects, None);
        let worktrees = sections[0]
            .worktrees
            .as_ref()
            .expect("registered worktrees should always render");

        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0].root, "/repo-feature");
        assert_eq!(worktrees[0].sessions.len(), 1);
        assert_eq!(worktrees[1].root, "/repo-empty");
        assert_eq!(worktrees[1].branch.as_deref(), Some("feature/empty"));
        assert!(worktrees[1].sessions.is_empty());
    }

    #[test]
    fn project_without_worktrees_renders_flat_even_with_multiple_roots() {
        let projects = vec![project("owned", "Owned", "/repo-main")];
        let sessions = vec![
            session(
                "a",
                "/repo-main",
                Some("/repo-main"),
                Some(owner("owned", "Owned")),
                None,
                1,
                "2026-07-05T12:00:00Z",
            ),
            session(
                "b",
                "/repo-feature",
                Some("/repo-feature"),
                Some(owner("owned", "Owned")),
                None,
                1,
                "2026-07-05T12:01:00Z",
            ),
        ];

        let sections = group_for_panel(&sessions, &projects, None);

        assert_eq!(sections.len(), 1);
        assert!(sections[0].worktrees.is_none());
        assert!(sections[0].main_group.is_none());
        assert_eq!(sections[0].sessions.len(), 2);
    }

    #[test]
    fn project_sections_and_rows_sort_newest_first_with_other_last() {
        let projects = vec![
            project("alpha", "Alpha", "/alpha"),
            project("beta", "Beta", "/beta"),
        ];
        let sessions = vec![
            session(
                "alpha-old",
                "/alpha",
                Some("/alpha"),
                None,
                None,
                1,
                "2026-07-05T12:00:00Z",
            ),
            session(
                "alpha-new",
                "/alpha",
                Some("/alpha"),
                None,
                None,
                1,
                "2026-07-05T12:02:00Z",
            ),
            session(
                "beta-newest-project",
                "/beta",
                Some("/beta"),
                None,
                None,
                1,
                "2026-07-05T12:03:00Z",
            ),
            session(
                "other-newest-overall",
                "/other",
                None,
                None,
                None,
                1,
                "2026-07-05T12:04:00Z",
            ),
        ];

        let sections = group_for_panel(&sessions, &projects, None);

        let keys: Vec<&str> = sections
            .iter()
            .map(|section| section.key.as_str())
            .collect();
        assert_eq!(keys, vec!["beta", "alpha", "__other__"]);
        let alpha_rows: Vec<&str> = sections[1]
            .sessions
            .iter()
            .map(|session| session.id.as_str())
            .collect();
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
            session(
                "beta-sess",
                "/beta",
                Some("/beta"),
                None,
                None,
                1,
                "2026-07-05T12:00:00Z",
            ),
            session(
                "other-sess",
                "/elsewhere",
                None,
                None,
                None,
                1,
                "2026-07-05T12:05:00Z",
            ),
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
    fn project_section_new_session_cwd_prefers_catalogue_then_explicit_fallback() {
        let catalog_projects = vec![project("owned", "Owned", "/catalog-root")];
        let catalog_sessions = vec![session(
            "catalog-session",
            "/session-root",
            Some("/session-root"),
            Some(owner("owned", "Owned")),
            None,
            1,
            "2026-07-17T12:00:00Z",
        )];
        let catalog_sections = group_for_panel(&catalog_sessions, &catalog_projects, None);
        assert_eq!(
            catalog_sections[0].new_session_cwd.as_deref(),
            Some("/catalog-root")
        );

        let orphan_sessions = vec![session(
            "orphan-session",
            "/orphan-cwd",
            Some("/orphan-workspace"),
            Some(owner("orphan", "Orphan")),
            None,
            1,
            "2026-07-17T12:00:00Z",
        )];
        let orphan_sections = group_for_panel(&orphan_sessions, &[], None);
        assert_eq!(
            orphan_sections[0].new_session_cwd.as_deref(),
            Some("/orphan-workspace")
        );

        let blank_projects = vec![project("blank", "Blank", "")];
        let blank_sessions = vec![session(
            "blank-session",
            "/section-fallback",
            None,
            Some(owner("blank", "Blank")),
            None,
            1,
            "2026-07-17T12:00:00Z",
        )];
        let blank_sections = group_for_panel(&blank_sessions, &blank_projects, None);
        assert_eq!(
            blank_sections[0].new_session_cwd.as_deref(),
            Some("/section-fallback")
        );

        let empty_sections = group_for_panel(&[], &blank_projects, None);
        assert_eq!(empty_sections[0].new_session_cwd, None);
    }

    #[test]
    fn sessions_escape_site_stops_bubbling_before_local_close() {
        let source = include_str!("sessions.rs");
        let handler_start = source
            .find("on:keydown=move |ev: web_sys::KeyboardEvent|")
            .expect("Sessions Escape production handler");
        let handler = &source[handler_start..handler_start + 900];
        let stop = handler
            .find("ev.stop_propagation()")
            .expect("Sessions Escape must stop propagation");
        let close = handler
            .find("open.set(false)")
            .expect("Sessions Escape local close");
        assert!(stop < close, "propagation must stop before local close");
    }

    #[test]
    fn project_form_focus_refs_are_attached_to_the_actual_first_inputs() {
        let source = include_str!("sessions.rs");
        let invalid_existing = ["</div>", " node_ref=existing_input_ref"].concat();
        let invalid_new = ["</div>", " node_ref=new_input_ref"].concat();
        assert!(!source.contains(&invalid_existing));
        assert!(!source.contains(&invalid_new));

        for node_ref in ["node_ref=existing_input_ref", "node_ref=new_input_ref"] {
            let ref_pos = source
                .find(node_ref)
                .expect("focus NodeRef production site");
            let input_pos = source[..ref_pos]
                .rfind("<input")
                .expect("NodeRef must follow an input opening tag");
            let close_pos = source[..ref_pos].rfind("</div>").unwrap_or(0);
            assert!(
                input_pos > close_pos,
                "{node_ref} must be an attribute of the first input"
            );
            let input_end = ref_pos
                + source[ref_pos..]
                    .find("/>")
                    .expect("NodeRef input must self-close");
            assert!(input_end > ref_pos);
        }
    }

    #[test]
    fn derive_project_name_uses_final_non_empty_path_segment() {
        for (path, expected_name) in [
            ("/Users/smaths/dev/ocean-surface", "ocean-surface"),
            ("~/dev/Ocean", "Ocean"),
            (
                "/Users/smaths/Client Projects/Ocean Surface",
                "Ocean Surface",
            ),
            ("/Users/smaths/dev/ocean-surface/", "ocean-surface"),
            ("/Users/smaths/dev/ocean-surface///", "ocean-surface"),
        ] {
            assert_eq!(derive_project_name(path), expected_name, "path={path:?}");
        }
    }

    #[test]
    fn derive_project_name_falls_back_for_blank_or_root_paths() {
        for path in ["/", "", "   \t\n"] {
            assert_eq!(derive_project_name(path), "project", "path={path:?}");
        }
    }

    #[test]
    fn project_root_from_parent_joins_parent_and_normalized_project_name() {
        assert_eq!(
            project_root_from_parent("~/dev", "Slop Check"),
            "~/dev/slop-check"
        );
    }

    #[test]
    fn project_root_from_parent_trims_trailing_parent_slash() {
        assert_eq!(
            project_root_from_parent("~/dev/", "Slop Check"),
            "~/dev/slop-check"
        );
    }

    #[test]
    fn project_root_from_parent_sanitizes_name_separators_into_single_folder() {
        for (name, expected_root) in [
            ("evil/../etc", "~/dev/evil-etc"),
            ("evil\\..\\etc", "~/dev/evil-etc"),
            ("!!!pwned!!!", "~/dev/pwned"),
            ("a...b", "~/dev/a-b"),
        ] {
            assert_eq!(
                project_root_from_parent("~/dev", name),
                expected_root,
                "name={name:?}"
            );
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

    #[test]
    fn layout_signature_changes_when_sessions_move_between_buckets() {
        let projects = vec![project_with_worktrees(
            "owned",
            "Owned",
            "/repo-main",
            &[("/repo-feature", Some("feature/redesign"))],
        )];
        let sessions = vec![
            session(
                "s1",
                "/repo-main",
                Some("/repo-main"),
                Some(owner("owned", "Owned")),
                None,
                1,
                "2026-07-05T12:00:00Z",
            ),
            session(
                "s2",
                "/repo-feature",
                Some("/repo-feature"),
                Some(owner("owned", "Owned")),
                Some("feature/redesign"),
                1,
                "2026-07-05T12:01:00Z",
            ),
            session("s3", "/other", None, None, None, 1, "2026-07-05T12:02:00Z"),
        ];
        let sections = group_for_panel(&sessions, &projects, None);
        let owned = sections.iter().find(|s| s.key == "owned").unwrap();
        let sig = section_layout_signature(owned);
        assert!(sig.contains("F0"), "flat count: {sig}");
        assert!(sig.contains("M1"), "main group: {sig}");
        assert!(sig.contains("W_"), "worktree: {sig}");

        let sessions2 = vec![
            session(
                "s1",
                "/repo-feature/x",
                Some("/repo-feature/x"),
                Some(owner("owned", "Owned")),
                Some("feature/redesign"),
                1,
                "2026-07-05T12:00:00Z",
            ),
            session(
                "s2",
                "/repo-feature",
                Some("/repo-feature"),
                Some(owner("owned", "Owned")),
                Some("feature/redesign"),
                1,
                "2026-07-05T12:01:00Z",
            ),
            session("s3", "/other", None, None, None, 1, "2026-07-05T12:02:00Z"),
        ];
        let sections2 = group_for_panel(&sessions2, &projects, None);
        let owned2 = sections2.iter().find(|s| s.key == "owned").unwrap();
        let sig2 = section_layout_signature(owned2);
        assert_ne!(
            sig, sig2,
            "signature must change when sessions move between buckets"
        );
    }

    #[test]
    fn render_key_changes_when_project_label_changes_without_content_change() {
        let projects = vec![project("owned", "dev", "/repo")];
        let sessions = vec![session(
            "s1",
            "/repo",
            Some("/repo"),
            Some(owner("owned", "dev")),
            None,
            1,
            "2026-07-05T12:00:00Z",
        )];
        let sections = group_for_panel(&sessions, &projects, None);
        let original = sections.iter().find(|s| s.key == "owned").unwrap();
        let mut renamed = original.clone();
        renamed.label = "ocean-surface".to_string();

        assert_eq!(
            section_total_sessions(original),
            section_total_sessions(&renamed)
        );
        assert_eq!(
            section_layout_signature(original),
            section_layout_signature(&renamed)
        );
        assert_ne!(
            section_render_key(original),
            section_render_key(&renamed),
            "a project rename must replace the captured section header"
        );

        let mut moved = original.clone();
        moved.new_session_cwd = Some("/repo-moved".to_string());
        assert_ne!(
            section_render_key(original),
            section_render_key(&moved),
            "a project-root change must replace the captured new-session cwd"
        );
    }

    // ── Poll lifecycle & dot state (Stream A1, v7) ──
    // Decision helpers `poll_guard_write`, `poll_should_skip`, and
    // `poll_release_in_flight` live at file scope (production).
    // The tests below exercise them directly.

    // ── Lifecycle tests ──

    #[test]
    fn poll_lifecycle_close_stale_write_rejected() {
        // gen bumped by close → old my_gen can't write, even if panel open.
        assert!(!poll_guard_write(2, 1, true));
        // Still matches if gen didn't move and panel open.
        assert!(poll_guard_write(2, 2, true));
        // Panel closed blocks write even when gen matches.
        assert!(!poll_guard_write(2, 2, false));
    }

    #[test]
    fn poll_lifecycle_reopen_fresh_write_allowed() {
        assert!(poll_guard_write(2, 2, true));
    }

    #[test]
    fn poll_lifecycle_in_flight_skip() {
        assert!(poll_should_skip(1, 1, true), "in_flight must skip");
        assert!(!poll_should_skip(1, 1, false), "no in_flight must fire");
    }

    #[test]
    fn poll_lifecycle_gen_mismatch_skip() {
        assert!(poll_should_skip(2, 1, false));
    }

    #[test]
    fn poll_lifecycle_destruction_rejects_write() {
        // gen bumped, panel closed.
        assert!(!poll_guard_write(2, 1, true), "stale write rejected");
        assert!(!poll_guard_write(2, 1, false), "stale + closed rejected");
    }

    #[test]
    fn poll_lifecycle_aba_gen_wraparound_rejected() {
        assert!(!poll_guard_write(0, u64::MAX, true));
    }

    // ── Unified release guard ──

    #[test]
    fn poll_release_gen_match_still_in_flight() {
        // gen matches + still in_flight → release allowed.
        assert!(poll_release_in_flight(2, 2, true));
        // gen matches + already cleared → no release.
        assert!(!poll_release_in_flight(2, 2, false));
        // gen moved + still in_flight → no release (stale task).
        assert!(!poll_release_in_flight(2, 1, true));
        // gen moved + already cleared → no release.
        assert!(!poll_release_in_flight(2, 1, false));
    }

    // ── Actual Abortable integration ──

    #[test]
    fn old_aborted_passes_through_unified_cleanup_does_not_clear_new_in_flight() {
        use futures_util::Future;
        use std::task::Poll;

        // Production flow:
        //   1. gen=1 task starts, in_flight=true (my_gen=1, panel open).
        //   2. close/reopen → stop_polling aborts fetch, bumps gen to 2.
        //   3. New task sets in_flight=true (my_gen=2).
        //   4. Old task's Abortable resolves Err(Aborted).
        //   5. Unified poll_release_in_flight(gen=2, my_gen=1, in_flight=true)
        //      → false (gen mismatch) → does NOT clear in_flight.
        //
        // Same as the production release guard: read the actual in_flight
        // value, not a literal true.

        let current_gen = std::cell::Cell::new(2u64); // after close/reopen
        let new_in_flight = std::cell::Cell::new(true);

        let my_gen = 1u64;
        let (fut, handle) = abortable(async {
            let _result: Result<Vec<SessionSummary>, String> =
                Err("simulated network error".into());
        });

        handle.abort();

        let mut fut = Box::pin(fut);
        let waker = futures_util::task::noop_waker();
        let mut cx = std::task::Context::from_waker(&waker);
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(Err(_aborted)) => {} // expected
            other => panic!("expected Poll::Ready(Err(Aborted)), got {other:?}"),
        }

        // Old task runs unified cleanup with actual in_flight value:
        let should_clear = poll_release_in_flight(current_gen.get(), my_gen, new_in_flight.get());
        assert!(
            !should_clear,
            "gen mismatch: stale task must not release in_flight"
        );

        assert!(
            new_in_flight.get(),
            "old abort must not clear new in_flight"
        );

        assert!(
            poll_release_in_flight(1, 1, true),
            "same gen: current owner must be allowed to release"
        );
    }

    // ── Collapse priming decider (TASK-28) ──
    // Pure `plan_collapse_priming` drives the "one open section" default. Tests
    // thread the returned state across passes to model regroupings and panel
    // generations without the Leptos runtime.

    #[test]
    fn collapse_priming_waits_for_active_session_through_catalogue_regroup() {
        let active = Some("active-sess");
        // Snapshot A: the session list arrived but the project catalogue and the
        // active session's row have not. The active id is present yet absent from
        // the layout → a provisional default that does NOT settle.
        let decoy = vec![session(
            "decoy",
            "/other",
            Some("/other"),
            None,
            None,
            1,
            "2026-07-19T12:05:00Z",
        )];
        let secs_a = group_for_panel(&decoy, &[], active);
        let plan_a = plan_collapse_priming(&CollapsePrimingState::default(), &secs_a, active);
        assert!(plan_a.collapsed.is_some(), "provisional default applied");
        assert_eq!(
            plan_a.next.primed,
            PrimedFor::Pending,
            "must not latch before the active session is located"
        );

        // Snapshot B: the catalogue and the active session's row land. It now
        // groups under its project; priming must move the default onto it and
        // collapse the unrelated newest section.
        let projects = vec![project("proj", "Proj", "/proj")];
        let sessions = vec![
            session(
                "decoy",
                "/other",
                Some("/other"),
                None,
                None,
                1,
                "2026-07-19T12:05:00Z",
            ),
            session(
                "active-sess",
                "/proj",
                Some("/proj"),
                None,
                None,
                1,
                "2026-07-19T12:01:00Z",
            ),
        ];
        let secs_b = group_for_panel(&sessions, &projects, active);
        let plan_b = plan_collapse_priming(&plan_a.next, &secs_b, active);
        let collapsed = plan_b.collapsed.expect("regroup applies a fresh default");
        assert!(
            !collapsed.contains("proj"),
            "active session's project section is expanded"
        );
        assert!(
            collapsed.contains("__other__"),
            "unrelated newest section is collapsed"
        );
    }

    #[test]
    fn collapse_priming_follows_active_session_change_before_user_input() {
        let projects = vec![project("a", "A", "/a"), project("b", "B", "/b")];
        let sessions = vec![
            session(
                "sa",
                "/a",
                Some("/a"),
                None,
                None,
                1,
                "2026-07-19T12:00:00Z",
            ),
            session(
                "sb",
                "/b",
                Some("/b"),
                None,
                None,
                1,
                "2026-07-19T12:01:00Z",
            ),
        ];

        let secs = group_for_panel(&sessions, &projects, Some("sa"));
        let plan1 = plan_collapse_priming(&CollapsePrimingState::default(), &secs, Some("sa"));
        let c1 = plan1.collapsed.expect("initial prime");
        assert!(!c1.contains("a"), "active session's section expanded");
        assert!(c1.contains("b"));

        // Active switches before any user toggle → the default follows to B.
        let secs2 = group_for_panel(&sessions, &projects, Some("sb"));
        let plan2 = plan_collapse_priming(&plan1.next, &secs2, Some("sb"));
        let c2 = plan2.collapsed.expect("re-prime on active-id change");
        assert!(!c2.contains("b"), "default followed the active session");
        assert!(c2.contains("a"));
    }

    #[test]
    fn collapse_priming_generation_resets_on_panel_reopen() {
        let projects = vec![project("a", "A", "/a"), project("b", "B", "/b")];
        let sessions = vec![
            session(
                "sa",
                "/a",
                Some("/a"),
                None,
                None,
                1,
                "2026-07-19T12:00:00Z",
            ),
            session(
                "sb",
                "/b",
                Some("/b"),
                None,
                None,
                1,
                "2026-07-19T12:01:00Z",
            ),
        ];
        let secs = group_for_panel(&sessions, &projects, Some("sa"));

        let plan = plan_collapse_priming(&CollapsePrimingState::default(), &secs, Some("sa"));
        let mut state = plan.next;
        state.user_toggled = true;
        // Same generation: the user now owns collapse state, so no re-prime.
        let idle = plan_collapse_priming(&state, &secs, Some("sa"));
        assert!(
            idle.collapsed.is_none(),
            "user owns collapse until the panel reopens"
        );
        // Close→reopen resets the generation (fresh default state) → eligible again.
        let reopened = plan_collapse_priming(&CollapsePrimingState::default(), &secs, Some("sa"));
        assert!(reopened.collapsed.is_some(), "reopen re-primes the default");
    }

    #[test]
    fn collapse_priming_user_toggle_survives_later_regroupings() {
        let projects = vec![project("a", "A", "/a"), project("b", "B", "/b")];
        let sessions = vec![
            session(
                "sa",
                "/a",
                Some("/a"),
                None,
                None,
                1,
                "2026-07-19T12:00:00Z",
            ),
            session(
                "sb",
                "/b",
                Some("/b"),
                None,
                None,
                1,
                "2026-07-19T12:01:00Z",
            ),
        ];
        let secs = group_for_panel(&sessions, &projects, Some("sa"));
        let plan = plan_collapse_priming(&CollapsePrimingState::default(), &secs, Some("sa"));
        let mut state = plan.next;
        state.user_toggled = true;

        // A later regroup grows the layout with a brand-new project/section.
        let projects2 = vec![
            project("a", "A", "/a"),
            project("b", "B", "/b"),
            project("c", "C", "/c"),
        ];
        let mut sessions2 = sessions.clone();
        sessions2.push(session(
            "sc",
            "/c",
            Some("/c"),
            None,
            None,
            1,
            "2026-07-19T12:09:00Z",
        ));
        let secs2 = group_for_panel(&sessions2, &projects2, Some("sa"));
        let plan2 = plan_collapse_priming(&state, &secs2, Some("sa"));
        assert!(
            plan2.collapsed.is_none(),
            "user toggle preserved across regrouping"
        );
    }

    #[test]
    fn collapse_priming_no_active_fallback_settles_and_reprimes_on_active() {
        let projects = vec![project("a", "A", "/a"), project("b", "B", "/b")];
        let sessions = vec![
            session(
                "sa",
                "/a",
                Some("/a"),
                None,
                None,
                1,
                "2026-07-19T12:00:00Z",
            ),
            session(
                "sb",
                "/b",
                Some("/b"),
                None,
                None,
                1,
                "2026-07-19T12:01:00Z",
            ),
        ];
        // No active session: fall back to the newest section and settle so a
        // later poll cannot make the default jump.
        let secs = group_for_panel(&sessions, &projects, None);
        let plan = plan_collapse_priming(&CollapsePrimingState::default(), &secs, None);
        let c = plan.collapsed.expect("no-active prime");
        assert!(!c.contains("b"), "newest section expanded");
        assert!(c.contains("a"));
        assert_eq!(plan.next.primed, PrimedFor::Settled(None));

        // A regroup where a different section is now newest must NOT re-prime.
        let sessions2 = vec![
            session(
                "sa",
                "/a",
                Some("/a"),
                None,
                None,
                1,
                "2026-07-19T12:09:00Z",
            ),
            session(
                "sb",
                "/b",
                Some("/b"),
                None,
                None,
                1,
                "2026-07-19T12:01:00Z",
            ),
        ];
        let secs2 = group_for_panel(&sessions2, &projects, None);
        let plan2 = plan_collapse_priming(&plan.next, &secs2, None);
        assert!(
            plan2.collapsed.is_none(),
            "settled no-active fallback does not jump between newest sections"
        );

        // Once an active session appears, priming re-evaluates onto it.
        let secs3 = group_for_panel(&sessions2, &projects, Some("sb"));
        let plan3 = plan_collapse_priming(&plan.next, &secs3, Some("sb"));
        let c3 = plan3.collapsed.expect("active appearance re-primes");
        assert!(
            !c3.contains("b"),
            "default moved onto the now-active session"
        );
    }

    // ── Dot state precedence (exact 5-state contract) ──

    #[test]
    fn dot_state_permission_steady() {
        assert_eq!(
            super::dot_state(Some(SessionRunState::WaitingForPermission), true),
            "permission"
        );
        assert_eq!(
            super::dot_state(Some(SessionRunState::WaitingForPermission), false),
            "permission"
        );
    }

    #[test]
    fn dot_state_cancelling_beats_running() {
        assert_eq!(
            super::dot_state(Some(SessionRunState::Cancelling), true),
            "cancelling"
        );
        assert_eq!(
            super::dot_state(Some(SessionRunState::Running), true),
            "running"
        );
    }

    #[test]
    fn dot_state_running_beats_recent() {
        assert_eq!(
            super::dot_state(Some(SessionRunState::Running), true),
            "running"
        );
        assert_eq!(super::dot_state(None, true), "recent");
    }

    #[test]
    fn dot_state_recent_beats_idle() {
        assert_eq!(super::dot_state(None, false), "idle");
        assert_eq!(super::dot_state(None, true), "recent");
    }

    #[test]
    fn dot_state_errored_completed_stored_fall_back() {
        assert_eq!(
            super::dot_state(Some(SessionRunState::Errored), true),
            "recent"
        );
        assert_eq!(
            super::dot_state(Some(SessionRunState::Errored), false),
            "idle"
        );
        assert_eq!(
            super::dot_state(Some(SessionRunState::Completed), false),
            "idle"
        );
        assert_eq!(
            super::dot_state(Some(SessionRunState::Unknown), true),
            "recent"
        );
    }

    #[test]
    fn dot_state_stored_is_idle_or_recent() {
        assert_eq!(
            super::dot_state(Some(SessionRunState::Stored), false),
            "idle"
        );
        assert_eq!(
            super::dot_state(Some(SessionRunState::Stored), true),
            "recent"
        );
    }
}
