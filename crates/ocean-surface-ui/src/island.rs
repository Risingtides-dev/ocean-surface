//! Pure daemon-backed projection and ranking model for the Dynamic Island.

use std::collections::HashSet;

use crate::daemon::{
    PendingPermission, PermissionSnapshot, ProjectInfo, RequestSnapshot, RequestSnapshotState,
    SessionSummary,
};
use crate::search::fuzzy_score;
#[cfg(target_arch = "wasm32")]
use crate::sessions::is_recent;
use crate::sessions::{project_for_root, session_root, split_origin};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum IslandPriority {
    Focused,
    Recent,
    Background,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IslandSession {
    pub id: String,
    pub title: String,
    pub project: Option<String>,
    pub cwd: String,
    pub branch: Option<String>,
    pub origin: Option<String>,
    pub updated_at: String,
    pub turn_count: u32,
    pub is_focused: bool,
    pub priority: IslandPriority,
}

#[derive(Clone, Debug, PartialEq)]
pub enum IslandResult {
    Session(IslandSession),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IslandAttentionState {
    NeedsHuman,
    Failed,
    Cancelling,
    Running,
}

impl IslandAttentionState {
    pub(crate) fn rank(self) -> u8 {
        match self {
            Self::NeedsHuman => 0,
            Self::Failed => 1,
            Self::Cancelling => 2,
            Self::Running => 3,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::NeedsHuman => "Needs attention",
            Self::Failed => "Failed",
            Self::Cancelling => "Stopping",
            Self::Running => "Running",
        }
    }

    pub(crate) fn class_name(self) -> &'static str {
        match self {
            Self::NeedsHuman => "is-needs-human",
            Self::Failed => "is-failed",
            Self::Cancelling => "is-cancelling",
            Self::Running => "is-running",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IslandAttentionItem {
    pub id: String,
    pub request_id: String,
    pub permission_id: Option<String>,
    pub session_id: Option<String>,
    pub session_title: String,
    pub project: Option<String>,
    pub tool: Option<String>,
    pub detail: String,
    pub updated_at: String,
    pub state: IslandAttentionState,
}

fn session_attention_metadata(
    sessions: &[IslandSession],
    session_id: Option<&str>,
) -> (String, Option<String>) {
    if let Some(session) =
        session_id.and_then(|id| sessions.iter().find(|session| session.id == id))
    {
        return (session.title.clone(), session.project.clone());
    }
    match session_id {
        Some(id) => (
            format!("Session {}", id.chars().take(8).collect::<String>()),
            None,
        ),
        None => ("Daemon request".to_string(), None),
    }
}

fn bounded_detail(value: &str) -> String {
    const LIMIT: usize = 240;
    let trimmed = value.trim();
    let mut chars = trimmed.chars();
    let prefix = chars.by_ref().take(LIMIT).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn request_updated_at(request: &RequestSnapshot) -> String {
    request
        .updated_at
        .as_deref()
        .or(request.finished_at.as_deref())
        .or(request.started_at.as_deref())
        .unwrap_or_default()
        .to_string()
}

/// Fold the daemon's global request/permission snapshots into a compact,
/// truthful activity projection. Permissions replace their matching waiting
/// request, terminal success/cancellation is omitted, and stale failure noise is
/// bounded to the newest three authoritative errored requests.
pub(crate) fn derive_attention_items(
    requests: &[RequestSnapshot],
    permissions: &[PermissionSnapshot],
    sessions: &[IslandSession],
) -> Vec<IslandAttentionItem> {
    let permission_requests: HashSet<&str> = permissions
        .iter()
        .map(|permission| permission.request_id.as_str())
        .collect();
    let mut items = Vec::new();

    for permission in permissions {
        let (session_title, project) =
            session_attention_metadata(sessions, permission.session_id.as_deref());
        let tool = (!permission.tool.trim().is_empty()).then(|| permission.tool.trim().to_string());
        let detail = if !permission.reason.trim().is_empty() {
            bounded_detail(&permission.reason)
        } else if let Some(tool) = tool.as_deref() {
            format!("Review requested access for {tool}")
        } else {
            "Review requested access".to_string()
        };
        items.push(IslandAttentionItem {
            id: format!("permission:{}", permission.permission_id),
            request_id: permission.request_id.clone(),
            permission_id: Some(permission.permission_id.clone()),
            session_id: permission.session_id.clone(),
            session_title,
            project,
            tool,
            detail,
            updated_at: permission.created_at.clone(),
            state: IslandAttentionState::NeedsHuman,
        });
    }

    let mut ordered_requests = requests.iter().collect::<Vec<_>>();
    ordered_requests.sort_by(|a, b| {
        request_updated_at(b)
            .cmp(&request_updated_at(a))
            .then_with(|| a.request_id.cmp(&b.request_id))
    });
    let mut failed_count = 0usize;
    for request in ordered_requests {
        if permission_requests.contains(request.request_id.as_str()) {
            continue;
        }
        let state = match request.state {
            RequestSnapshotState::Queued | RequestSnapshotState::Running => {
                IslandAttentionState::Running
            }
            RequestSnapshotState::WaitingForPermission => IslandAttentionState::NeedsHuman,
            RequestSnapshotState::Cancelling => IslandAttentionState::Cancelling,
            RequestSnapshotState::Errored if failed_count < 3 => {
                failed_count += 1;
                IslandAttentionState::Failed
            }
            RequestSnapshotState::Errored
            | RequestSnapshotState::Cancelled
            | RequestSnapshotState::Completed => continue,
        };
        let (session_title, project) =
            session_attention_metadata(sessions, request.session_id.as_deref());
        let detail = request
            .message
            .as_deref()
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .map(bounded_detail)
            .unwrap_or_else(|| state.label().to_string());
        items.push(IslandAttentionItem {
            id: format!("request:{}", request.request_id),
            request_id: request.request_id.clone(),
            permission_id: request.permission_id.clone(),
            session_id: request.session_id.clone(),
            session_title,
            project,
            tool: None,
            detail,
            updated_at: request_updated_at(request),
            state,
        });
    }

    items.sort_by(|a, b| {
        a.state
            .rank()
            .cmp(&b.state.rank())
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    items
}

#[cfg(test)]
type IslandAttentionKey = (
    String,
    IslandAttentionState,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
);

#[cfg(test)]
fn attention_render_key(item: &IslandAttentionItem) -> IslandAttentionKey {
    (
        item.id.clone(),
        item.state,
        item.permission_id.clone(),
        item.session_title.clone(),
        item.project.clone(),
        item.tool.clone(),
        item.detail.clone(),
        item.updated_at.clone(),
    )
}

pub(crate) fn permission_action_state(
    permission_id: Option<&str>,
    session_id: Option<&str>,
    focused_id: Option<&str>,
    has_decision_token: bool,
    pending: &[PendingPermission],
) -> Option<bool> {
    let permission_id = permission_id?;
    let session_id = session_id?;
    if !has_decision_token || focused_id != Some(session_id) {
        return None;
    }
    pending
        .iter()
        .find(|item| item.permission_id == permission_id && item.session_id == session_id)
        .map(|item| item.deciding)
}

pub(crate) fn stop_action_state(
    state: IslandAttentionState,
    request_id: &str,
    cancellations: &[String],
) -> Option<bool> {
    (state == IslandAttentionState::Running)
        .then(|| cancellations.iter().any(|pending| pending == request_id))
}

pub(crate) fn compact_attention_counts(items: &[IslandAttentionItem]) -> (usize, usize) {
    let needs_human = items
        .iter()
        .filter(|item| item.state == IslandAttentionState::NeedsHuman)
        .count();
    let running = items
        .iter()
        .filter(|item| item.state == IslandAttentionState::Running)
        .count();
    (needs_human, running)
}

fn recently_updated(updated_at: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        is_recent(updated_at)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Browser time is intentionally unavailable in native unit tests.
        // Sorting remains fully deterministic there; production uses the
        // shared sessions-panel recency window above.
        let _ = updated_at;
        false
    }
}

/// Project daemon sessions into the compact metadata used by the Island.
pub(crate) fn derive_island_sessions(
    sessions: &[SessionSummary],
    projects: &[ProjectInfo],
    focused_id: Option<&str>,
) -> Vec<IslandSession> {
    let mut items: Vec<_> = sessions
        .iter()
        .map(|session| {
            let root = session_root(session).to_string();
            let project = session
                .owning_project
                .as_ref()
                .map(|owner| {
                    if owner.name.trim().is_empty() {
                        owner.id.clone()
                    } else {
                        owner.name.trim().to_string()
                    }
                })
                .or_else(|| {
                    project_for_root(projects, &root).map(|project| {
                        if project.name.trim().is_empty() {
                            project.id.clone()
                        } else {
                            project.name.trim().to_string()
                        }
                    })
                });
            let (origin, clean_title) = split_origin(&session.title);
            let title = if clean_title.trim().is_empty() {
                "(untitled)".to_string()
            } else {
                clean_title
            };
            let is_focused = focused_id == Some(session.id.as_str());
            let priority = if is_focused {
                IslandPriority::Focused
            } else if recently_updated(&session.updated_at) {
                IslandPriority::Recent
            } else {
                IslandPriority::Background
            };

            IslandSession {
                id: session.id.clone(),
                title,
                project,
                cwd: root,
                branch: session.git_branch.clone().filter(|value| !value.is_empty()),
                origin,
                updated_at: session.updated_at.clone(),
                turn_count: session.turn_count,
                is_focused,
                priority,
            }
        })
        .collect();

    items.sort_by(|a, b| {
        b.is_focused
            .cmp(&a.is_focused)
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
    items
}

/// Normalized session document shared by Browse filtering and `Cmd/Ctrl+P`.
pub(crate) fn island_search_text(item: &IslandSession) -> String {
    let state = match item.priority {
        IslandPriority::Focused => "focused",
        IslandPriority::Recent => "recent",
        IslandPriority::Background => "background",
    };
    let project = item.project.as_deref().unwrap_or_default();
    let branch = item.branch.as_deref().unwrap_or_default();
    let origin = item.origin.as_deref().unwrap_or_default();
    format!(
        "{} {} project:{} {} {} {} {} {} turns",
        item.title, project, project, item.cwd, branch, origin, state, item.turn_count
    )
    .to_lowercase()
}

/// Search the Island corpus while preserving derivation order for an empty
/// query and using focus/recency only as small tie-breaks for real queries.
pub(crate) fn search_island(query: &str, items: &[IslandSession]) -> Vec<IslandResult> {
    let query = query.trim();
    if query.is_empty() {
        return items.iter().cloned().map(IslandResult::Session).collect();
    }

    let query_lower = query.to_lowercase();
    let mut scored: Vec<(u8, f64, usize, IslandSession)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let document_score = fuzzy_score(query, &island_search_text(item))?;
            let title = item.title.to_lowercase();
            let title_score = fuzzy_score(query, &item.title);
            let project = item.project.as_deref().map(str::to_lowercase);
            let project_score = project
                .as_deref()
                .and_then(|project| fuzzy_score(query, project));

            // Match category is a strict tier; fuzzy score only orders items
            // inside a tier, so a long fuzzy title can never outrank an exact
            // project (and metadata can never outrank a title/project match).
            let (tier, mut score) = if title == query_lower {
                (6, title_score.unwrap_or(document_score))
            } else if title.starts_with(&query_lower) {
                (5, title_score.unwrap_or(document_score))
            } else if project.as_deref() == Some(query_lower.as_str()) {
                (4, project_score.unwrap_or(document_score))
            } else if project
                .as_deref()
                .is_some_and(|project| project.starts_with(&query_lower))
            {
                (3, project_score.unwrap_or(document_score))
            } else if let Some(title_score) = title_score {
                (2, title_score)
            } else if let Some(project_score) = project_score {
                (1, project_score)
            } else {
                (0, document_score)
            };
            score += match item.priority {
                IslandPriority::Focused => 2.0,
                IslandPriority::Recent => 1.0,
                IslandPriority::Background => 0.0,
            };
            Some((tier, score, index, item.clone()))
        })
        .collect();

    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| a.2.cmp(&b.2))
            .then_with(|| a.3.id.cmp(&b.3.id))
    });
    scored
        .into_iter()
        .map(|(_, _, _, item)| IslandResult::Session(item))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::{OwningProjectRef, WorktreeInfo};

    fn session(id: &str, title: &str, root: &str, updated_at: &str) -> SessionSummary {
        SessionSummary {
            id: id.into(),
            title: title.into(),
            cwd: root.into(),
            workspace_root: None,
            owning_project: None,
            git_branch: None,
            turn_count: 1,
            active_turn: None,
            active_state: None,
            updated_at: updated_at.into(),
        }
    }

    fn project(id: &str, name: &str, root: &str) -> ProjectInfo {
        ProjectInfo {
            id: id.into(),
            name: name.into(),
            workspace_root: root.into(),
            git_branch: None,
            git_dirty: None,
            worktrees: Vec::<WorktreeInfo>::new(),
        }
    }

    fn request(
        id: &str,
        session_id: Option<&str>,
        state: RequestSnapshotState,
        updated_at: &str,
    ) -> RequestSnapshot {
        RequestSnapshot {
            request_id: id.into(),
            session_id: session_id.map(str::to_string),
            state,
            permission_id: None,
            message: Some(format!("request {id}")),
            started_at: None,
            updated_at: Some(updated_at.into()),
            finished_at: None,
        }
    }

    fn permission(id: &str, request_id: &str, session_id: Option<&str>) -> PermissionSnapshot {
        PermissionSnapshot {
            permission_id: id.into(),
            request_id: request_id.into(),
            session_id: session_id.map(str::to_string),
            tool: "bash".into(),
            reason: "Run the requested check".into(),
            created_at: "2026-07-14T12:00:00Z".into(),
        }
    }

    #[test]
    fn attention_permission_replaces_matching_waiting_request() {
        let source = vec![session(
            "session-a",
            "Focused work",
            "/work/ocean",
            "2026-07-14T12:00:00Z",
        )];
        let sessions = derive_island_sessions(&source, &[], Some("session-a"));
        let requests = vec![
            request(
                "request-a",
                Some("session-a"),
                RequestSnapshotState::WaitingForPermission,
                "2026-07-14T12:00:00Z",
            ),
            request(
                "request-b",
                Some("session-a"),
                RequestSnapshotState::Running,
                "2026-07-14T11:59:00Z",
            ),
        ];
        let items = derive_attention_items(
            &requests,
            &[permission("permission-a", "request-a", Some("session-a"))],
            &sessions,
        );

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "permission:permission-a");
        assert_eq!(items[0].permission_id.as_deref(), Some("permission-a"));
        assert_eq!(items[0].state, IslandAttentionState::NeedsHuman);
        assert_eq!(items[0].session_title, "Focused work");
        assert_eq!(items[1].id, "request:request-b");
        assert_eq!(items[1].state, IslandAttentionState::Running);
    }

    #[test]
    fn permission_actions_require_focused_submitter_authority() {
        let pending = vec![PendingPermission {
            permission_id: "permission-a".into(),
            session_id: "session-a".into(),
            tool: "bash".into(),
            reason: "run check".into(),
            args_summary: String::new(),
            deciding: false,
            request_id: Some("request-a".into()),
            actionable: true,
        }];

        assert_eq!(
            permission_action_state(
                Some("permission-a"),
                Some("session-a"),
                Some("session-a"),
                true,
                &pending,
            ),
            Some(false)
        );
        assert_eq!(
            permission_action_state(
                Some("permission-a"),
                Some("session-a"),
                Some("session-b"),
                true,
                &pending,
            ),
            None
        );
        assert_eq!(
            permission_action_state(
                Some("permission-a"),
                Some("session-a"),
                Some("session-a"),
                false,
                &pending,
            ),
            None
        );
    }

    #[test]
    fn stop_action_is_running_only_and_tracks_transport_pending() {
        assert_eq!(
            stop_action_state(IslandAttentionState::Running, "request-a", &[]),
            Some(false)
        );
        assert_eq!(
            stop_action_state(
                IslandAttentionState::Running,
                "request-a",
                &["request-a".into()],
            ),
            Some(true)
        );
        assert_eq!(
            stop_action_state(
                IslandAttentionState::Cancelling,
                "request-a",
                &["request-a".into()],
            ),
            None
        );
    }

    #[test]
    fn attention_omits_success_and_bounds_failed_history() {
        let requests = vec![
            request(
                "failed-1",
                None,
                RequestSnapshotState::Errored,
                "2026-07-14T05:00:00Z",
            ),
            request(
                "failed-2",
                None,
                RequestSnapshotState::Errored,
                "2026-07-14T04:00:00Z",
            ),
            request(
                "failed-3",
                None,
                RequestSnapshotState::Errored,
                "2026-07-14T03:00:00Z",
            ),
            request(
                "failed-4",
                None,
                RequestSnapshotState::Errored,
                "2026-07-14T02:00:00Z",
            ),
            request(
                "done",
                None,
                RequestSnapshotState::Completed,
                "2026-07-14T06:00:00Z",
            ),
            request(
                "cancelled",
                None,
                RequestSnapshotState::Cancelled,
                "2026-07-14T07:00:00Z",
            ),
        ];
        let items = derive_attention_items(&requests, &[], &[]);
        assert_eq!(items.len(), 3);
        assert!(items
            .iter()
            .all(|item| item.state == IslandAttentionState::Failed));
        assert_eq!(items[0].request_id, "failed-1");
        assert_eq!(items[2].request_id, "failed-3");
    }

    #[test]
    fn attention_render_key_changes_with_authoritative_card_content() {
        let mut item = derive_attention_items(
            &[request(
                "request-a",
                Some("session-a"),
                RequestSnapshotState::Running,
                "2026-07-14T05:00:00Z",
            )],
            &[],
            &[],
        )
        .remove(0);
        let running_key = attention_render_key(&item);
        item.state = IslandAttentionState::Cancelling;
        item.detail = "stopping".into();
        assert_ne!(running_key, attention_render_key(&item));
    }

    #[test]
    fn compact_counts_exclude_failed_and_cancelling_items() {
        let requests = vec![
            request(
                "failed",
                None,
                RequestSnapshotState::Errored,
                "2026-07-14T05:00:00Z",
            ),
            request(
                "cancelling",
                None,
                RequestSnapshotState::Cancelling,
                "2026-07-14T04:00:00Z",
            ),
        ];
        let items = derive_attention_items(&requests, &[], &[]);
        assert_eq!(compact_attention_counts(&items), (0, 0));
    }

    #[test]
    fn attention_detail_is_unicode_safe_and_bounded() {
        let long = "水".repeat(300);
        let bounded = bounded_detail(&long);
        assert_eq!(bounded.chars().count(), 241);
        assert!(bounded.ends_with('…'));
    }

    #[test]
    fn blank_catalogue_title_uses_the_shared_untitled_fallback() {
        let items = derive_island_sessions(
            &[session("draft", "", "/draft", "2026-07-14T12:00:00Z")],
            &[],
            Some("draft"),
        );
        assert_eq!(items[0].title, "(untitled)");
    }

    #[test]
    fn focused_sorts_first_and_newer_background_sorts_next() {
        let sessions = vec![
            session("new", "New", "/new", "2026-07-14T12:00:00Z"),
            session("focused", "Focused", "/focus", "2020-01-01T00:00:00Z"),
            session("old", "Old", "/old", "2025-01-01T00:00:00Z"),
        ];
        let items = derive_island_sessions(&sessions, &[], Some("focused"));
        assert_eq!(
            items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["focused", "new", "old"]
        );
    }

    #[test]
    fn project_prefers_owning_project_and_falls_back_to_root() {
        let mut owned = session("owned", "Owned", "/elsewhere", "2026-01-01T00:00:00Z");
        owned.owning_project = Some(OwningProjectRef {
            id: "authoritative-id".into(),
            name: "Authoritative".into(),
        });
        let fallback = session(
            "fallback",
            "Fallback",
            "/work/ocean-surface/src",
            "2025-01-01T00:00:00Z",
        );
        let projects = vec![project("surface", "Ocean Surface", "/work/ocean-surface")];
        let items = derive_island_sessions(&[owned, fallback], &projects, None);
        assert_eq!(items[0].project.as_deref(), Some("Authoritative"));
        assert_eq!(items[1].project.as_deref(), Some("Ocean Surface"));
    }

    #[test]
    fn origin_prefix_is_stripped_and_exposed() {
        let items = derive_island_sessions(
            &[session(
                "web",
                "[WEB] North star",
                "/work",
                "2026-01-01T00:00:00Z",
            )],
            &[],
            None,
        );
        assert_eq!(items[0].title, "North star");
        assert_eq!(items[0].origin.as_deref(), Some("web"));
    }

    #[test]
    fn title_match_outranks_project_or_path_only_matches() {
        let title_item = IslandSession {
            id: "title".into(),
            title: "North Star".into(),
            project: None,
            cwd: "/tmp".into(),
            branch: None,
            origin: None,
            updated_at: String::new(),
            turn_count: 1,
            is_focused: false,
            priority: IslandPriority::Background,
        };
        let mut metadata_item = title_item.clone();
        metadata_item.id = "metadata".into();
        metadata_item.title = "Unrelated".into();
        metadata_item.project = Some("North Star Project".into());
        metadata_item.cwd = "/work/north/star".into();
        let results = search_island("north star", &[metadata_item, title_item]);
        let IslandResult::Session(first) = &results[0];
        assert_eq!(first.id, "title");
    }

    #[test]
    fn exact_project_outranks_fuzzy_title() {
        let fuzzy_title = IslandSession {
            id: "title".into(),
            title: "Domain".into(),
            project: None,
            cwd: "/tmp".into(),
            branch: None,
            origin: None,
            updated_at: String::new(),
            turn_count: 1,
            is_focused: false,
            priority: IslandPriority::Background,
        };
        let mut exact_project = fuzzy_title.clone();
        exact_project.id = "project".into();
        exact_project.title = "Unrelated".into();
        exact_project.project = Some("main".into());

        let results = search_island("main", &[fuzzy_title, exact_project]);
        let IslandResult::Session(first) = &results[0];
        assert_eq!(first.id, "project");
    }

    #[test]
    fn metadata_and_state_are_searchable() {
        let item = IslandSession {
            id: "one".into(),
            title: "Work".into(),
            project: Some("ocean-surface".into()),
            cwd: "/work/src/app.rs".into(),
            branch: Some("main".into()),
            origin: Some("tauri".into()),
            updated_at: String::new(),
            turn_count: 0,
            is_focused: false,
            priority: IslandPriority::Recent,
        };
        for query in [
            "project:ocean-surface",
            "src/app.rs",
            "main",
            "tauri",
            "recent",
            "0 turns",
        ] {
            assert_eq!(
                search_island(query, std::slice::from_ref(&item)).len(),
                1,
                "{query}"
            );
        }
    }

    #[test]
    fn empty_query_preserves_order_and_zero_turn_session() {
        let mut zero = IslandSession {
            id: "zero".into(),
            title: "Draft".into(),
            project: None,
            cwd: "/draft".into(),
            branch: None,
            origin: None,
            updated_at: String::new(),
            turn_count: 0,
            is_focused: false,
            priority: IslandPriority::Background,
        };
        let mut other = zero.clone();
        other.id = "other".into();
        other.title = "Other".into();
        let results = search_island("", &[zero.clone(), other]);
        assert!(matches!(&results[0], IslandResult::Session(item) if item.id == "zero"));
        assert_eq!(search_island("draft", std::slice::from_ref(&zero)).len(), 1);
        zero.title = "Changed".into();
    }

    #[test]
    fn ties_are_deterministic_in_input_order() {
        let base = IslandSession {
            id: "a".into(),
            title: "Alpha".into(),
            project: None,
            cwd: "/same".into(),
            branch: None,
            origin: None,
            updated_at: String::new(),
            turn_count: 1,
            is_focused: false,
            priority: IslandPriority::Background,
        };
        let mut second = base.clone();
        second.id = "b".into();
        let results = search_island("same", &[second, base]);
        let ids: Vec<_> = results
            .iter()
            .map(|result| match result {
                IslandResult::Session(item) => item.id.as_str(),
            })
            .collect();
        assert_eq!(ids, vec!["b", "a"]);
    }
}
