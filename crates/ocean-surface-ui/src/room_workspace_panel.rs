//! The room's workspace and its command history — the read half of the lane
//! `room_repo.rs` drives.
//!
//! A federated room can have a Bedrock container workspace; every command run
//! in it — a member's exec, a clone, a build — lands in a durable ledger that
//! outlives the container itself. The daemon exposes the two reads a room
//! MEMBER gets (`room_workspace_proxy.rs`'s allowlist, `?actor_id=` asserted
//! on every call):
//!
//!   GET /v1/rooms/persistent/{key}/workspace          → status + exec index
//!   GET /v1/rooms/persistent/{key}/workspace/execs    → rows with tails
//!
//! Provision and destroy are deliberately NOT on that allowlist — they are
//! owner acts over the Bedrock API — so a room without a workspace is stated
//! as a fact here, never offered as a button.
//!
//! What the wire promises, and this panel honors:
//!
//! 1. **Typed refusals are states.** Bedrock's `workspace_absent` 404 is
//!    relayed verbatim by the daemon and means "this room has no workspace
//!    yet" — an answer, not an error. The daemon's own gates (missing actor,
//!    not a member, not federated) are typed the same way.
//! 2. **Tails are per-row, per-stream, and sometimes withheld.** The execs
//!    route returns `stdout_tail`/`stderr_tail` only on rows the caller may
//!    read back — the owner reads all, anyone else reads their own members'
//!    rows. An ABSENT tail is that permission answer and renders as one; a
//!    NULL tail is a command still running. The two must not collapse, which
//!    is why the fields deserialize through `double_option` below. Clipping
//!    is reported per stream (`stdout_clipped`/`stderr_clipped`) — there is
//!    no row-level flag on this wire.
//! 3. **A build is an exec row.** Bedrock marks build commands with
//!    `# ocean-room-build` on their first line, so "the last build's outcome"
//!    is derived from the same list, not a separate read.
//! 4. **The status route's exec index carries no tails.** The panel reads the
//!    execs route instead, and the status body's `recent_execs` is left on
//!    the floor rather than rendered as twenty rows that all look withheld.
//!
//! The daemon's federation ingest accepts only `message` events, so
//! `room.workspace.*` events never reach this surface — an open panel POLLS
//! (the `room_repo` clone-poller idiom: ticket admission, epoch retirement,
//! `(generation, key)` re-validation) so a reload, a re-entered room, or a
//! second member's panel all read current state rather than a view frozen by
//! whoever opened one first.
//!
//! A deployment whose daemon or Bedrock predates these routes answers 404
//! with no code; that renders as "not available yet", plainly. Everything
//! that turns a reply into what the operator sees is a free function below,
//! unit-testable natively.

use gloo_net::http::Request;
use leptos::prelude::*;
use serde::{Deserialize, Deserializer};
use wasm_bindgen_futures::spawn_local;

use crate::rooms::{encode, RoomAccessProjection, RoomAccessState, Rooms};

/// How often an open panel re-reads status and history. The same cadence as
/// the repo section's clone poller: honest without leaning on the daemon.
const PANEL_POLL_MS: u32 = 4_000;

/// Rows asked of the execs route. Bedrock bounds the parameter to 1..=200
/// and defaults to 50; 30 keeps the panel a read, not an archive.
const EXEC_LIST_LIMIT: u32 = 30;

/// Bedrock's build marker (`BUILD_COMMAND_MARKER` in src/room-compute.mjs):
/// a build claims the checkout by holding an exec row whose command opens
/// with this line, which is also what makes builds recognizable here.
const BUILD_COMMAND_MARKER: &str = "# ocean-room-build";

// ---- Wire types -------------------------------------------------------------

/// Bedrock's `publicWorkspaceProjection` (src/room-compute.mjs), the fields
/// this panel renders. `spec`, `capabilities` and `container` are richer
/// upstream; the provision facts are what a member acts on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct WorkspaceProjection {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub driver: String,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub last_active_at: Option<String>,
    #[serde(default)]
    pub last_flushed_at: Option<String>,
}

/// Present (null or string) versus absent, kept apart: serde folds a JSON
/// null into the same `None` as a missing field, and on this wire those are
/// different answers — null is "still running", absent is "not yours to
/// read".
fn double_option<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

/// One row of `publicExecProjection`. The tail fields exist only on rows the
/// caller may read back; the clipped flags travel with them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ExecRow {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    pub stdout_tail: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub stderr_tail: Option<Option<String>>,
    #[serde(default)]
    pub stdout_clipped: Option<bool>,
    #[serde(default)]
    pub stderr_clipped: Option<bool>,
}

/// Bedrock's thrown refusals nest their `code` under `details`; its plain
/// 404s and every daemon-side gate put `code` at the top level. Same dual
/// home as `room_repo`'s, read the same way.
#[derive(Debug, Default, Deserialize)]
struct ErrorDetails {
    #[serde(default)]
    code: Option<String>,
}

/// The lenient envelope both reads fit into. Presence of `workspace` or
/// `execs` is what success means — there is no `ok` field on this lane.
#[derive(Debug, Default, Deserialize)]
struct WorkspaceBody {
    #[serde(default)]
    workspace: Option<WorkspaceProjection>,
    #[serde(default)]
    execs: Option<Vec<ExecRow>>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    details: Option<ErrorDetails>,
}

impl WorkspaceBody {
    fn refusal_code(&self) -> Option<&str> {
        self.code
            .as_deref()
            .or_else(|| self.details.as_ref().and_then(|d| d.code.as_deref()))
    }
}

// ---- Pure helpers -----------------------------------------------------------

fn status_url(base: &str, key: &str, actor: &str) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/workspace?actor_id={}",
        encode(key),
        encode(actor),
    )
}

fn execs_url(base: &str, key: &str, actor: &str) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/workspace/execs?actor_id={}&limit={EXEC_LIST_LIMIT}",
        encode(key),
        encode(actor),
    )
}

/// What the room's workspace IS right now. `None` in the state signal means
/// "not answered yet" — only a reply mints one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WorkspaceView {
    /// A workspace record stands (any status — `provisioning`, `ready`,
    /// `failed`, `destroyed` all render as themselves).
    Present(Box<WorkspaceProjection>),
    /// `workspace_absent`: the room has the lane but no workspace yet. An
    /// answer — and the place absence is stated instead of offered.
    Absent,
    /// The daemon says this room is not federated. The access projection
    /// normally hides the section first; this keeps the classification total.
    NotFederated,
    /// The deployment in front of us does not serve these routes. Said
    /// plainly instead of erroring — this IS production behavior until the
    /// daemon and Bedrock in front of it are redeployed.
    Unavailable,
}

/// What the history read answered.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ExecsView {
    Rows(Vec<ExecRow>),
    /// The deployment does not serve the execs route.
    Unavailable,
}

/// The failure sentence for a coded refusal that is NOT a state. The same
/// vocabulary as the repo section's, because it is the same lane.
fn failure_sentence(code: &str) -> Option<String> {
    let sentence = match code {
        "not_a_room_member" => "You're not on this room's roster.",
        "room_not_found" => "This room is unknown to the daemon.",
        "room_access_revoked" => "This room's federation access was revoked.",
        "workspace_unavailable" => "The room's compute service can't be reached right now.",
        "workspace_upstream_protocol" => {
            "The room's compute service answered something this surface can't read."
        }
        _ => return None,
    };
    Some(sentence.to_string())
}

/// Map a status-route reply onto what the panel should believe. `body` is
/// `None` when the reply did not decode — which a route-less deployment
/// produces (an empty 404), so that case is an ANSWER, not a fault.
fn classify_status(status: u16, body: Option<WorkspaceBody>) -> Result<WorkspaceView, String> {
    let Some(body) = body else {
        if status == 404 {
            return Ok(WorkspaceView::Unavailable);
        }
        return Err(format!(
            "The workspace status reply could not be read ({status})."
        ));
    };
    if let Some(workspace) = body.workspace {
        return Ok(WorkspaceView::Present(Box::new(workspace)));
    }
    match body.refusal_code() {
        Some("workspace_absent") => Ok(WorkspaceView::Absent),
        Some("room_not_federated") => Ok(WorkspaceView::NotFederated),
        Some("workspace_route_not_allowed") => Ok(WorkspaceView::Unavailable),
        Some(code) => Err(failure_sentence(code)
            .or_else(|| body.error.clone())
            .unwrap_or_else(|| format!("Workspace status failed ({status})."))),
        None if status == 404 => Ok(WorkspaceView::Unavailable),
        None => Err(body
            .error
            .filter(|error| !error.is_empty())
            .map(|error| format!("Workspace status failed: {error}"))
            .unwrap_or_else(|| format!("Workspace status failed ({status})."))),
    }
}

/// Map an execs-route reply. `workspace_absent` here is an empty history —
/// the ledger outlives the container, and Bedrock's current handler answers
/// rows regardless of the workspace record, so absence can only mean nothing
/// ran.
fn classify_execs(status: u16, body: Option<WorkspaceBody>) -> Result<ExecsView, String> {
    let Some(body) = body else {
        if status == 404 {
            return Ok(ExecsView::Unavailable);
        }
        return Err(format!(
            "The command history reply could not be read ({status})."
        ));
    };
    if let Some(execs) = body.execs {
        return Ok(ExecsView::Rows(execs));
    }
    match body.refusal_code() {
        Some("workspace_absent") => Ok(ExecsView::Rows(Vec::new())),
        // The status read answers NotFederated too and hides the section;
        // an empty history here just keeps the classification total.
        Some("room_not_federated") => Ok(ExecsView::Rows(Vec::new())),
        Some("workspace_route_not_allowed") => Ok(ExecsView::Unavailable),
        Some(code) => Err(failure_sentence(code)
            .or_else(|| body.error.clone())
            .unwrap_or_else(|| format!("Command history failed ({status})."))),
        None if status == 404 => Ok(ExecsView::Unavailable),
        None => Err(body
            .error
            .filter(|error| !error.is_empty())
            .map(|error| format!("Command history failed: {error}"))
            .unwrap_or_else(|| format!("Command history failed ({status})."))),
    }
}

/// Whether an exec row is a build — Bedrock's marker opens the command.
fn is_build(command: &str) -> bool {
    command.starts_with(BUILD_COMMAND_MARKER)
}

/// The one line a row leads with. Build rows shed the marker line — the
/// member wrote `npm run build`, not the bookkeeping above it — and a
/// multi-line command shows its first line with an ellipsis; the full text
/// rides the row's title attribute.
fn command_headline(command: &str) -> String {
    let shown = command
        .strip_prefix(BUILD_COMMAND_MARKER)
        .map(str::trim_start)
        .filter(|rest| !rest.is_empty())
        .unwrap_or(command)
        .trim();
    let mut lines = shown.lines().filter(|line| !line.trim().is_empty());
    let first = lines.next().unwrap_or("").trim().to_string();
    if lines.next().is_some() {
        format!("{first} \u{2026}")
    } else {
        first
    }
}

/// The verdict mark before the headline: ran clean, went wrong, or still
/// going. The same shorthand `scripts/room.mjs` prints.
fn exec_mark(row: &ExecRow) -> &'static str {
    match (row.status.as_str(), row.exit_code) {
        ("running", _) => "\u{2026}",
        ("exited", Some(0)) => "\u{2713}",
        _ => "\u{2717}",
    }
}

/// The row's outcome in words. Exit code over adjectives — "exited 1" is
/// what the operator greps the script for.
fn exec_status_line(row: &ExecRow) -> String {
    let when = row
        .finished_at
        .as_deref()
        .or(row.started_at.as_deref())
        .filter(|at| !at.is_empty())
        .map(|at| format!(" \u{b7} {at}"))
        .unwrap_or_default();
    match (row.status.as_str(), row.exit_code) {
        ("running", _) => format!("running{when}"),
        ("exited", Some(code)) => format!("exited {code}{when}"),
        ("timeout", _) => format!("timed out{when}"),
        ("failed", _) => format!("failed{when}"),
        (status, Some(code)) => format!("{status} ({code}){when}"),
        (status, None) => format!("{status}{when}"),
    }
}

/// What a row's output area renders.
#[derive(Debug, PartialEq, Eq)]
enum RowTails {
    /// The route withheld both tails: this run's output belongs to whoever
    /// ran it. A state with a sentence, never an empty box.
    Withheld,
    /// The streams the caller may read, empty ones dropped — a command that
    /// printed nothing gets no box either.
    Streams(Vec<StreamTail>),
}

#[derive(Debug, PartialEq, Eq)]
struct StreamTail {
    label: &'static str,
    text: String,
    clipped: bool,
}

fn row_tails(row: &ExecRow) -> RowTails {
    if row.stdout_tail.is_none() && row.stderr_tail.is_none() {
        return RowTails::Withheld;
    }
    let mut streams = Vec::new();
    for (label, tail, clipped) in [
        ("stdout", &row.stdout_tail, row.stdout_clipped),
        ("stderr", &row.stderr_tail, row.stderr_clipped),
    ] {
        // `Some(None)` is a stream still being written — the status line
        // already says "running", so no box is the honest render.
        let Some(Some(text)) = tail else { continue };
        if text.is_empty() {
            continue;
        }
        streams.push(StreamTail {
            label,
            text: text.strip_suffix('\n').unwrap_or(text).to_string(),
            clipped: clipped.unwrap_or(false),
        });
    }
    RowTails::Streams(streams)
}

/// The outcome of the room's most recent build, from the same list — rows
/// arrive most-recent-first. `None` when nothing in view is a build.
fn last_build_sentence(rows: &[ExecRow]) -> Option<String> {
    let build = rows.iter().find(|row| is_build(&row.command))?;
    let command = command_headline(&build.command);
    let sentence = match (build.status.as_str(), build.exit_code) {
        ("running", _) => format!("A build is running \u{2014} {command}."),
        ("exited", Some(0)) => format!("Last build succeeded \u{2014} {command}."),
        ("exited", Some(code)) => format!("Last build exited {code} \u{2014} {command}."),
        ("timeout", _) => format!("Last build timed out \u{2014} {command}."),
        _ => format!("Last build failed \u{2014} {command}."),
    };
    Some(sentence)
}

/// The compact line under the rail header. One line; the panel has the rest.
fn glance_line(view: &WorkspaceView) -> Option<String> {
    match view {
        WorkspaceView::Present(workspace) => {
            if workspace.driver.is_empty() {
                Some(workspace.status.clone())
            } else {
                Some(format!("{} \u{b7} {}", workspace.status, workspace.driver))
            }
        }
        WorkspaceView::Absent => Some("No workspace.".to_string()),
        WorkspaceView::NotFederated | WorkspaceView::Unavailable => None,
    }
}

/// Whether the panel is worth opening: only a room that answered with a
/// workspace or an honest absence has anything to show there.
fn panel_can_open(view: Option<&WorkspaceView>) -> bool {
    matches!(
        view,
        Some(WorkspaceView::Present(_) | WorkspaceView::Absent)
    )
}

/// Latest-wins admission for an overlapping read — same shape as
/// `room_repo::read_is_current`, for the same premature-publish bug class.
fn read_is_current(ticket: u64, current: u64) -> bool {
    ticket == current
}

/// Whether the section exists for this room at all. Only a federated room
/// has a Bedrock workspace; a Local room renders nothing rather than a
/// refusal, and `None` (no room open / still loading) also renders nothing.
fn room_is_federated(access: Option<&RoomAccessProjection>) -> bool {
    access.is_some_and(|projection| projection.state != RoomAccessState::Local)
}

/// Escape owned by this panel. Same contract as `repo_escape_closes`: a
/// fixed modal at the top of the rooms surface consumes the key before the
/// drawers under it.
pub fn workspace_panel_escape_closes(panel_open: bool, default_prevented: bool) -> bool {
    panel_open && !default_prevented
}

// ---- State ------------------------------------------------------------------

/// Reactive handle for one room's workspace view.
///
/// Constructed at `RoomsWorkspace` component scope, never inside a rail
/// closure: those closures re-run on every `rooms.access` SSE update, and
/// the open panel owns a poll loop that a rebuild would orphan and respawn
/// mid-tick.
#[derive(Clone, Copy)]
pub struct RoomWorkspacePanelState {
    /// Daemon base URL, shared with `Daemon::url` through `Rooms::url`.
    pub url: RwSignal<String>,
    /// What the workspace is, once the status read has answered.
    view: RwSignal<Option<WorkspaceView>>,
    /// What the history read answered. `None` = not asked or not answered
    /// yet — the panel fetches on open, so the rail never pays for it.
    execs: RwSignal<Option<ExecsView>>,
    /// A foreground status read is in flight (the poller refreshes silently).
    loading: RwSignal<bool>,
    /// The most recent failure, either read.
    error: RwSignal<Option<String>>,
    /// Whether the reading-measure panel is open.
    panel: RwSignal<bool>,
    /// The rail control that opens the panel, so closing hands focus back.
    open_ref: NodeRef<leptos::html::Button>,
    /// Monotonic ticket per read lane; only the latest overlapping read may
    /// publish.
    ticket: RwSignal<u64>,
    execs_ticket: RwSignal<u64>,
    /// Poller generation; bumping it retires any running poll loop.
    poll_epoch: RwSignal<u64>,
}

impl RoomWorkspacePanelState {
    pub fn new(rooms: &Rooms) -> Self {
        Self {
            url: rooms.url,
            view: RwSignal::new(None),
            execs: RwSignal::new(None),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            panel: RwSignal::new(false),
            open_ref: NodeRef::new(),
            ticket: RwSignal::new(0),
            execs_ticket: RwSignal::new(0),
            poll_epoch: RwSignal::new(0),
        }
    }

    /// Whether the panel is on screen. Public because the Escape ladder that
    /// owns the key lives in `rooms_workspace`, not here.
    pub fn panel_is_open(&self) -> bool {
        self.panel.get_untracked()
    }

    /// Close the panel, retire its poll loop, and hand focus back.
    pub fn close_panel(&self) {
        self.panel.set(false);
        self.poll_epoch
            .update(|epoch| *epoch = epoch.wrapping_add(1));
        if let Some(open) = self.open_ref.get_untracked() {
            let _ = open.focus();
        }
    }

    fn base(&self) -> String {
        self.url.get_untracked().trim_end_matches('/').to_string()
    }

    /// Retire whatever is on screen, in flight, and polling. The epoch bump
    /// stops the previous room's poll loop from writing this room's state;
    /// the ticket bumps retire its unfinished reads the same way.
    fn reset(&self) {
        self.ticket
            .update(|ticket| *ticket = ticket.wrapping_add(1));
        self.execs_ticket
            .update(|ticket| *ticket = ticket.wrapping_add(1));
        self.poll_epoch
            .update(|epoch| *epoch = epoch.wrapping_add(1));
        self.view.set(None);
        self.execs.set(None);
        self.loading.set(false);
        self.error.set(None);
        self.panel.set(false);
    }

    /// Read the workspace status, foreground: the rail shows it happening.
    fn fetch_status(&self, key: String, actor: String) {
        let base = self.base();
        let me = *self;
        let ticket = self.ticket.get_untracked().wrapping_add(1);
        self.ticket.set(ticket);
        self.loading.set(true);
        self.error.set(None);
        spawn_local(async move {
            let result = read_workspace(&base, &key, &actor).await;
            me.publish_status(result, read_is_current(ticket, me.ticket.get_untracked()));
        });
    }

    /// Publish a completed status read — but only the latest one.
    fn publish_status(&self, result: Result<WorkspaceView, String>, is_current: bool) {
        if !is_current {
            return;
        }
        self.loading.set(false);
        match result {
            Ok(view) => self.view.set(Some(view)),
            // A failed refresh never blanks a standing view: what the
            // operator was reading is still the best answer this surface has.
            Err(error) => self.error.set(Some(error)),
        }
    }

    /// Read the command history, foreground.
    fn fetch_execs(&self, key: String, actor: String) {
        let base = self.base();
        let me = *self;
        let ticket = self.execs_ticket.get_untracked().wrapping_add(1);
        self.execs_ticket.set(ticket);
        spawn_local(async move {
            let result = read_execs(&base, &key, &actor).await;
            me.publish_execs(
                result,
                read_is_current(ticket, me.execs_ticket.get_untracked()),
            );
        });
    }

    /// Publish a completed history read — but only the latest one.
    fn publish_execs(&self, result: Result<ExecsView, String>, is_current: bool) {
        if !is_current {
            return;
        }
        match result {
            Ok(view) => self.execs.set(Some(view)),
            Err(error) => self.error.set(Some(error)),
        }
    }

    /// Open the panel: read both lanes fresh — a reopened panel shows the
    /// room as it is, not as it was when last opened — and start the poll
    /// loop that keeps it that way.
    fn open_panel(&self, rooms: Rooms, key: String, actor: String) {
        self.error.set(None);
        self.panel.set(true);
        self.fetch_status(key.clone(), actor.clone());
        self.fetch_execs(key.clone(), actor.clone());
        self.poll_while_open(rooms, key, actor);
    }

    /// Refresh both lanes while the panel is open. Reads silently — no
    /// loading flicker — and publishes through the same ticket admission as
    /// every other read, so an overlapping foreground fetch still wins. The
    /// epoch and `(generation, key)` checks are what keep a loop from
    /// surviving its room or doubling up with a successor.
    fn poll_while_open(&self, rooms: Rooms, key: String, actor: String) {
        let epoch = self.poll_epoch.get_untracked().wrapping_add(1);
        self.poll_epoch.set(epoch);
        let base = self.base();
        let me = *self;
        let generation = rooms.generation_snapshot();
        spawn_local(async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(PANEL_POLL_MS).await;
                if me.poll_epoch.get_untracked() != epoch
                    || !rooms.room_is_current(generation, &key)
                    || !me.panel.get_untracked()
                {
                    return;
                }
                let ticket = me.ticket.get_untracked().wrapping_add(1);
                me.ticket.set(ticket);
                let result = read_workspace(&base, &key, &actor).await;
                me.publish_status(result, read_is_current(ticket, me.ticket.get_untracked()));

                let ticket = me.execs_ticket.get_untracked().wrapping_add(1);
                me.execs_ticket.set(ticket);
                let result = read_execs(&base, &key, &actor).await;
                me.publish_execs(
                    result,
                    read_is_current(ticket, me.execs_ticket.get_untracked()),
                );
            }
        });
    }
}

/// One status read: transport, decode, classify. A body that does not decode
/// is handed to `classify_status` as `None` — an empty 404 is an ANSWER on
/// this lane (a deployment without the routes), not a fault.
async fn read_workspace(base: &str, key: &str, actor: &str) -> Result<WorkspaceView, String> {
    let url = status_url(base, key, actor);
    match Request::get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.json::<WorkspaceBody>().await.ok();
            classify_status(status, body)
        }
        Err(err) => Err(format!("Workspace status request failed: {err}")),
    }
}

async fn read_execs(base: &str, key: &str, actor: &str) -> Result<ExecsView, String> {
    let url = execs_url(base, key, actor);
    match Request::get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.json::<WorkspaceBody>().await.ok();
            classify_execs(status, body)
        }
        Err(err) => Err(format!("Command history request failed: {err}")),
    }
}

// ---- Component --------------------------------------------------------------

/// The open room's workspace: a compact rail row with a glance line, and a
/// panel where the provision facts and the command history are read.
///
/// Renders NOTHING for a Local room — no workspace exists there and a
/// refusal would only read as breakage. Everything here is a read, so no
/// `writes_allowed` gate: what a member may see, the daemon already decided
/// per row.
#[component]
pub fn RoomWorkspacePanel(rooms: Rooms, state: RoomWorkspacePanelState) -> impl IntoView {
    // The (key, actor) this section should be reading, or `None` when it
    // should be dark — the same Memo shape as `room_repo`'s, for the same
    // reason: `access` updates on every roster SSE event, and the workspace
    // lane needs `?actor_id=` on every call including reads.
    let read_target = Memo::new(move |_| {
        let key = rooms.open_key.get().filter(|key| !key.is_empty())?;
        if !room_is_federated(rooms.access.get().as_ref()) {
            return None;
        }
        if !rooms.identity_authoritative.get() {
            return None;
        }
        let actor = rooms.identity_id.get();
        if actor.is_empty() {
            return None;
        }
        Some((key, actor))
    });

    // Follow the target. Clearing FIRST is what stops the previous room's
    // workspace from being read, however briefly, under this room's name.
    Effect::new(move |_| match read_target.get() {
        Some((key, actor)) => {
            state.reset();
            state.fetch_status(key, actor);
        }
        None => state.reset(),
    });

    // The one place the open action resolves the room key and the actor
    // together; the identity refusal is the composer's, in its words.
    let actor = move || -> Option<(String, String)> {
        let key = rooms
            .open_key
            .get_untracked()
            .filter(|key| !key.is_empty())?;
        if !rooms.identity_resolved() {
            state.error.set(Some(
                "Still signing in \u{2014} try again in a moment.".to_string(),
            ));
            return None;
        }
        Some((key, rooms.identity_id.get_untracked()))
    };

    // Section visibility as a Memo, not raw reads in the closure below: the
    // closure would otherwise rebuild on every roster SSE update and poll
    // publish, tearing down the open panel and its poll loop with it.
    let visible = Memo::new(move |_| {
        room_is_federated(rooms.access.get().as_ref())
            && !matches!(state.view.get(), Some(WorkspaceView::NotFederated))
    });

    view! {
        {move || {
            if !visible.get() {
                return ().into_any();
            }
            view! {
                <div class="rooms-workspace__compute">
                    <div class="rooms-workspace__compute-head">
                        <span class="rooms-workspace__compute-title">"Workspace"</span>
                        <button
                            class="rooms-workspace__compute-open"
                            type="button"
                            node_ref=state.open_ref
                            title="Open this room's workspace and command history"
                            disabled=move || !panel_can_open(state.view.get().as_ref())
                            on:click=move |_| {
                                let Some((key, actor_id)) = actor() else { return };
                                state.open_panel(rooms, key, actor_id);
                            }
                        >
                            "open"
                        </button>
                    </div>

                    // Rendered in the rail AND the panel, like the repo
                    // error: a failure while the panel is closed must not
                    // read as a room without a workspace.
                    {move || {
                        state.error.get().map(|error| view! {
                            <div class="rooms-workspace__compute-error" role="alert">{error}</div>
                        })
                    }}

                    {move || {
                        if state.loading.get() && state.view.get().is_none() {
                            return view! {
                                <div class="rooms-workspace__compute-note">
                                    "Checking workspace\u{2026}"
                                </div>
                            }.into_any();
                        }
                        match state.view.get() {
                            Some(WorkspaceView::Unavailable) => view! {
                                <div class="rooms-workspace__compute-note">
                                    "Workspace status isn't available on this deployment yet."
                                </div>
                            }.into_any(),
                            Some(view_state) => glance_line(&view_state)
                                .map(|line| view! {
                                    <div class="rooms-workspace__compute-line">{line}</div>
                                }.into_any())
                                .unwrap_or_else(|| ().into_any()),
                            None => ().into_any(),
                        }
                    }}

                    {move || {
                        if !state.panel.get() {
                            return ().into_any();
                        }
                        view! {
                            <div
                                class="rooms-workspace__compute-scrim"
                                on:click=move |_| state.close_panel()
                            ></div>
                            <div
                                class="rooms-workspace__compute-panel"
                                role="dialog"
                                aria-modal="true"
                                aria-label="Room workspace"
                            >
                                <div class="rooms-workspace__compute-panel-head">
                                    <span class="rooms-workspace__compute-panel-title">
                                        "Workspace"
                                    </span>
                                    <button
                                        class="rooms-workspace__compute-close"
                                        type="button"
                                        aria-label="Close workspace"
                                        on:click=move |_| state.close_panel()
                                    >
                                        "\u{d7}"
                                    </button>
                                </div>
                                <div class="rooms-workspace__compute-panel-body">
                                    {move || {
                                        state.error.get().map(|error| view! {
                                            <div
                                                class="rooms-workspace__compute-error"
                                                role="alert"
                                            >
                                                {error}
                                            </div>
                                        })
                                    }}
                                    {move || match state.view.get() {
                                        Some(WorkspaceView::Present(workspace)) => {
                                            panel_facts(&workspace).into_any()
                                        }
                                        Some(WorkspaceView::Absent) => view! {
                                            // Absence stated, not offered:
                                            // provisioning is not on the
                                            // member lane.
                                            <div class="rooms-workspace__compute-note">
                                                "This room has no workspace yet \u{2014} \
                                                 provisioning is an owner act, by API for now."
                                            </div>
                                        }.into_any(),
                                        _ => ().into_any(),
                                    }}
                                    {move || {
                                        let rows = match state.execs.get() {
                                            Some(ExecsView::Rows(rows)) => rows,
                                            Some(ExecsView::Unavailable) => {
                                                return view! {
                                                    <div class="rooms-workspace__compute-note">
                                                        "Command history isn't available on \
                                                         this deployment yet."
                                                    </div>
                                                }.into_any();
                                            }
                                            // `None` is a history that has not
                                            // answered yet — the panel always
                                            // asks on open.
                                            None => {
                                                return view! {
                                                    <div class="rooms-workspace__compute-note">
                                                        "Reading command history\u{2026}"
                                                    </div>
                                                }.into_any();
                                            }
                                        };
                                        view! {
                                            {last_build_sentence(&rows).map(|sentence| view! {
                                                <div class="rooms-workspace__compute-build">
                                                    {sentence}
                                                </div>
                                            })}
                                            <div class="rooms-workspace__compute-execs-title">
                                                "Recent commands"
                                            </div>
                                            {if rows.is_empty() {
                                                view! {
                                                    <div class="rooms-workspace__compute-note">
                                                        "No commands have run in this \
                                                         workspace yet."
                                                    </div>
                                                }.into_any()
                                            } else {
                                                rows.iter()
                                                    .map(exec_row_view)
                                                    .collect::<Vec<_>>()
                                                    .into_any()
                                            }}
                                        }.into_any()
                                    }}
                                </div>
                            </div>
                        }.into_any()
                    }}
                </div>
            }.into_any()
        }}
    }
}

/// The provision facts for a standing workspace.
fn panel_facts(workspace: &WorkspaceProjection) -> impl IntoView {
    let fact = |label: &'static str, value: Option<String>| {
        value.filter(|value| !value.is_empty()).map(|value| {
            view! {
                <span class="rooms-workspace__compute-fact-label">{label}</span>
                <span class="rooms-workspace__compute-fact-value">{value}</span>
            }
        })
    };
    view! {
        <div class="rooms-workspace__compute-facts">
            {fact("status", Some(workspace.status.clone()))}
            {fact("driver", Some(workspace.driver.clone()))}
            {fact("created", workspace.created_at.clone())}
            {fact("active", workspace.last_active_at.clone())}
            {fact("flushed", workspace.last_flushed_at.clone())}
        </div>
    }
}

/// One command in the history: verdict, headline, outcome, and its output —
/// or the sentence explaining why the output is not here.
fn exec_row_view(row: &ExecRow) -> impl IntoView {
    let build = is_build(&row.command);
    let mark = exec_mark(row);
    let headline = command_headline(&row.command);
    let full = row.command.clone();
    let status_line = exec_status_line(row);
    let tails = row_tails(row);
    view! {
        <div class="rooms-workspace__compute-exec">
            <div class="rooms-workspace__compute-exec-head" title=full>
                <span class="rooms-workspace__compute-exec-mark">{mark}</span>
                {build.then(|| view! {
                    <span class="rooms-workspace__compute-exec-badge">"build"</span>
                })}
                <span class="rooms-workspace__compute-exec-command">{headline}</span>
            </div>
            <div class="rooms-workspace__compute-exec-meta">{status_line}</div>
            {match tails {
                RowTails::Withheld => view! {
                    <div class="rooms-workspace__compute-tail-note">
                        "Output withheld \u{2014} it belongs to the member who ran this."
                    </div>
                }.into_any(),
                RowTails::Streams(streams) => streams
                    .into_iter()
                    .map(|stream| view! {
                        <pre class="rooms-workspace__compute-tail" data-stream=stream.label>
                            {stream.text}
                        </pre>
                        {stream.clipped.then(|| view! {
                            <div class="rooms-workspace__compute-tail-note">
                                {format!(
                                    "{} clipped \u{2014} the stored exec row keeps more.",
                                    stream.label
                                )}
                            </div>
                        })}
                    })
                    .collect::<Vec<_>>()
                    .into_any(),
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_state() -> RoomWorkspacePanelState {
        RoomWorkspacePanelState {
            url: RwSignal::new("http://d".to_string()),
            view: RwSignal::new(None),
            execs: RwSignal::new(None),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            panel: RwSignal::new(false),
            open_ref: NodeRef::new(),
            ticket: RwSignal::new(0),
            execs_ticket: RwSignal::new(0),
            poll_epoch: RwSignal::new(0),
        }
    }

    fn body(json: &str) -> WorkspaceBody {
        serde_json::from_str(json).unwrap()
    }

    /// Bedrock's status body as the daemon relays it: projection, ports, and
    /// the tail-less exec index this panel deliberately ignores.
    fn status_json() -> &'static str {
        r#"{"workspace": {
            "room_id": "room-1",
            "status": "ready",
            "driver": "cloudflare",
            "spec": {"image": "ocean-workspace"},
            "created_at": "2026-08-20T09:00:00.000Z",
            "last_active_at": "2026-08-27T10:00:00.000Z",
            "last_flushed_at": "2026-08-27T10:01:00.000Z",
            "capabilities": {}
        },
        "ports": [],
        "recent_execs": [{"id": "e1", "command": "npm test", "status": "exited",
                          "exit_code": 0}]}"#
    }

    #[test]
    fn a_standing_workspace_is_present() {
        let view = classify_status(200, Some(body(status_json()))).unwrap();
        let WorkspaceView::Present(workspace) = view else {
            panic!("expected Present, got {view:?}");
        };
        assert_eq!(workspace.status, "ready");
        assert_eq!(workspace.driver, "cloudflare");
        assert_eq!(
            workspace.last_active_at.as_deref(),
            Some("2026-08-27T10:00:00.000Z")
        );
    }

    /// Bedrock's 404 carries a top-level code and is a STATE — the room has
    /// the lane, just no workspace yet.
    #[test]
    fn workspace_absent_is_an_answer_not_an_error() {
        let absent =
            body(r#"{"error": "This room has no workspace.", "code": "workspace_absent"}"#);
        assert_eq!(
            classify_status(404, Some(absent)),
            Ok(WorkspaceView::Absent)
        );
    }

    #[test]
    fn not_federated_is_recognized() {
        let gated = body(
            r#"{"ok": false, "code": "room_not_federated",
                "error": "this room has no Bedrock credential, so it has no workspace"}"#,
        );
        assert_eq!(
            classify_status(409, Some(gated)),
            Ok(WorkspaceView::NotFederated)
        );
    }

    /// A deployment that predates the lane answers 404 with an empty or
    /// unrecognizable body. An ANSWER ("not available yet"), never a failure
    /// — and, until the production daemon and Bedrock are redeployed, the
    /// answer most operators will see.
    #[test]
    fn a_route_less_deployment_reads_as_unavailable() {
        assert_eq!(classify_status(404, None), Ok(WorkspaceView::Unavailable));
        let plain = body(r#"{"ok": false, "error": "Not found"}"#);
        assert_eq!(
            classify_status(404, Some(plain)),
            Ok(WorkspaceView::Unavailable)
        );
        let coded = body(r#"{"ok": false, "code": "workspace_route_not_allowed"}"#);
        assert_eq!(
            classify_status(404, Some(coded)),
            Ok(WorkspaceView::Unavailable)
        );
    }

    #[test]
    fn a_gate_refusal_is_a_failure_in_words() {
        let gated = body(
            r#"{"ok": false, "code": "not_a_room_member",
                "error": "the asserted actor is not on this room's roster"}"#,
        );
        let err = classify_status(403, Some(gated)).unwrap_err();
        assert_eq!(err, "You're not on this room's roster.");
    }

    #[test]
    fn an_unreachable_bedrock_is_an_error_not_a_view() {
        let relay = body(
            r#"{"ok": false, "code": "workspace_unavailable",
                "error": "the room workspace could not be reached"}"#,
        );
        let err = classify_status(503, Some(relay)).unwrap_err();
        assert!(err.contains("can't be reached"), "got: {err}");
    }

    // ---- the execs wire -----------------------------------------------------

    /// A row the caller may read: tails present, clipping per stream.
    #[test]
    fn a_readable_row_carries_its_tails() {
        let listed = body(
            r#"{"execs": [{
                "id": "e1", "actor_member_id": "m1", "command": "npm test",
                "cwd": "/workspace", "status": "exited", "exit_code": 1,
                "truncated": false, "started_at": "2026-08-27T10:00:00.000Z",
                "finished_at": "2026-08-27T10:00:30.000Z",
                "stdout_tail": "12 passing\n", "stderr_tail": "1 failing\n",
                "stdout_clipped": false, "stderr_clipped": true}]}"#,
        );
        let ExecsView::Rows(rows) = classify_execs(200, Some(listed)).unwrap() else {
            panic!("expected rows");
        };
        let RowTails::Streams(streams) = row_tails(&rows[0]) else {
            panic!("expected streams, got withheld");
        };
        assert_eq!(streams.len(), 2);
        assert_eq!(streams[0].label, "stdout");
        // The stored trailing newline is presentation noise, not content.
        assert_eq!(streams[0].text, "12 passing");
        assert!(!streams[0].clipped);
        assert!(streams[1].clipped);
    }

    /// ABSENT tails are the route withholding another member's output — a
    /// state with a sentence, never empty boxes.
    #[test]
    fn absent_tails_are_withheld() {
        let listed = body(
            r#"{"execs": [{"id": "e2", "command": "printenv", "status": "exited",
                           "exit_code": 0}]}"#,
        );
        let ExecsView::Rows(rows) = classify_execs(200, Some(listed)).unwrap() else {
            panic!("expected rows");
        };
        assert_eq!(row_tails(&rows[0]), RowTails::Withheld);
    }

    /// NULL tails are a command still running — readable, just not written
    /// yet. Distinct from absent, which is why `double_option` exists.
    #[test]
    fn null_tails_are_running_not_withheld() {
        let listed = body(
            r#"{"execs": [{"id": "e3", "command": "npm run dev", "status": "running",
                           "exit_code": null, "stdout_tail": null, "stderr_tail": null,
                           "stdout_clipped": false, "stderr_clipped": false}]}"#,
        );
        let ExecsView::Rows(rows) = classify_execs(200, Some(listed)).unwrap() else {
            panic!("expected rows");
        };
        // Not withheld — but nothing to show either: no boxes.
        assert_eq!(row_tails(&rows[0]), RowTails::Streams(Vec::new()));
    }

    /// A command that ran and printed nothing gets no box — cribbed from
    /// `scripts/room.mjs`, which skips empty streams the same way.
    #[test]
    fn empty_tails_render_no_boxes() {
        let row = ExecRow {
            stdout_tail: Some(Some(String::new())),
            stderr_tail: Some(Some(String::new())),
            ..ExecRow::default()
        };
        assert_eq!(row_tails(&row), RowTails::Streams(Vec::new()));
    }

    /// The ledger outlives the container: an absent workspace on the execs
    /// route is an empty history, and a route-less deployment is said
    /// plainly.
    #[test]
    fn execs_states_classify_totally() {
        let absent =
            body(r#"{"error": "This room has no workspace.", "code": "workspace_absent"}"#);
        assert_eq!(
            classify_execs(404, Some(absent)),
            Ok(ExecsView::Rows(Vec::new()))
        );
        assert_eq!(classify_execs(404, None), Ok(ExecsView::Unavailable));
        let gated = body(r#"{"ok": false, "code": "not_a_room_member"}"#);
        assert_eq!(
            classify_execs(403, Some(gated)),
            Err("You're not on this room's roster.".to_string())
        );
    }

    // ---- builds -------------------------------------------------------------

    /// A build row is an exec row opening with Bedrock's marker; the marker
    /// is bookkeeping and the headline sheds it.
    #[test]
    fn a_build_row_is_recognized_and_the_marker_shed() {
        let command = "# ocean-room-build\nnpm run build";
        assert!(is_build(command));
        assert!(!is_build("npm run build"));
        assert_eq!(command_headline(command), "npm run build");
        assert_eq!(command_headline("npm test"), "npm test");
    }

    #[test]
    fn a_multi_line_command_headlines_its_first_line() {
        assert_eq!(
            command_headline("git fetch origin \\\n  && git checkout -B main"),
            "git fetch origin \\ \u{2026}"
        );
        // A marker with nothing after it still names itself rather than
        // vanishing into an unnameable row.
        assert_eq!(command_headline("# ocean-room-build"), "# ocean-room-build");
    }

    /// "The last build's outcome" is derived from the list, most-recent
    /// first — exit code over adjectives, same rule as the repo section.
    #[test]
    fn the_last_build_outcome_is_derived_from_the_list() {
        let exec = |command: &str, status: &str, exit_code: Option<i64>| ExecRow {
            command: command.to_string(),
            status: status.to_string(),
            exit_code,
            ..ExecRow::default()
        };
        let rows = vec![
            exec("npm test", "exited", Some(0)),
            exec("# ocean-room-build\nnpm run build", "exited", Some(1)),
            exec("# ocean-room-build\nnpm run build", "exited", Some(0)),
        ];
        assert_eq!(
            last_build_sentence(&rows).as_deref(),
            Some("Last build exited 1 \u{2014} npm run build.")
        );
        assert_eq!(
            last_build_sentence(&rows[..1]).as_deref(),
            None,
            "a list with no build rows claims nothing"
        );
        let running = vec![exec("# ocean-room-build\nnpm run dist", "running", None)];
        assert_eq!(
            last_build_sentence(&running).as_deref(),
            Some("A build is running \u{2014} npm run dist.")
        );
    }

    // ---- presentation -------------------------------------------------------

    #[test]
    fn the_mark_and_status_line_say_how_it_ended() {
        let row = |status: &str, exit_code: Option<i64>| ExecRow {
            status: status.to_string(),
            exit_code,
            finished_at: Some("2026-08-27T10:00:30.000Z".to_string()),
            ..ExecRow::default()
        };
        assert_eq!(exec_mark(&row("exited", Some(0))), "\u{2713}");
        assert_eq!(exec_mark(&row("exited", Some(2))), "\u{2717}");
        assert_eq!(exec_mark(&row("failed", None)), "\u{2717}");
        assert_eq!(exec_mark(&row("running", None)), "\u{2026}");
        assert_eq!(
            exec_status_line(&row("exited", Some(2))),
            "exited 2 \u{b7} 2026-08-27T10:00:30.000Z"
        );
        assert_eq!(
            exec_status_line(&row("timeout", None)),
            "timed out \u{b7} 2026-08-27T10:00:30.000Z"
        );
        // A running row has no finish time; it falls back to the start.
        let mut live = row("running", None);
        live.finished_at = None;
        live.started_at = Some("2026-08-27T10:00:00.000Z".to_string());
        assert_eq!(
            exec_status_line(&live),
            "running \u{b7} 2026-08-27T10:00:00.000Z"
        );
    }

    #[test]
    fn the_glance_line_says_what_stands() {
        let present = WorkspaceView::Present(Box::new(WorkspaceProjection {
            status: "ready".to_string(),
            driver: "cloudflare".to_string(),
            ..WorkspaceProjection::default()
        }));
        assert_eq!(
            glance_line(&present).as_deref(),
            Some("ready \u{b7} cloudflare")
        );
        assert_eq!(
            glance_line(&WorkspaceView::Absent).as_deref(),
            Some("No workspace.")
        );
        assert_eq!(glance_line(&WorkspaceView::Unavailable), None);
        assert_eq!(glance_line(&WorkspaceView::NotFederated), None);
    }

    /// The panel opens on an answer — a workspace or an honest absence —
    /// and absence deliberately opens: the history ledger outlives the
    /// container.
    #[test]
    fn the_panel_opens_only_on_an_answer() {
        assert!(panel_can_open(Some(&WorkspaceView::Absent)));
        assert!(panel_can_open(Some(
            &WorkspaceView::Present(Box::default())
        )));
        assert!(!panel_can_open(Some(&WorkspaceView::Unavailable)));
        assert!(!panel_can_open(Some(&WorkspaceView::NotFederated)));
        assert!(!panel_can_open(None));
    }

    // ---- publish admission --------------------------------------------------

    #[test]
    fn a_stale_read_publishes_nothing() {
        let state = fresh_state();
        state.loading.set(true);
        state.publish_status(Ok(WorkspaceView::Absent), false);
        assert!(state.loading.get_untracked());
        assert_eq!(state.view.get_untracked(), None);

        state.publish_execs(Ok(ExecsView::Rows(Vec::new())), false);
        assert_eq!(state.execs.get_untracked(), None);
    }

    /// A failed poll refresh must not blank what the operator is reading —
    /// the standing answer outranks a transient read error.
    #[test]
    fn a_failed_read_keeps_the_standing_view() {
        let state = fresh_state();
        state.view.set(Some(WorkspaceView::Absent));
        state.execs.set(Some(ExecsView::Rows(Vec::new())));
        state.publish_status(Err("boom".to_string()), true);
        state.publish_execs(Err("boom".to_string()), true);
        assert_eq!(state.view.get_untracked(), Some(WorkspaceView::Absent));
        assert_eq!(
            state.execs.get_untracked(),
            Some(ExecsView::Rows(Vec::new()))
        );
        assert_eq!(state.error.get_untracked().as_deref(), Some("boom"));
    }

    // ---- gates --------------------------------------------------------------

    #[test]
    fn only_a_federated_room_has_the_section() {
        assert!(!room_is_federated(None));
        let projection = |state| RoomAccessProjection {
            state,
            last_confirmed_global_sequence: None,
            members: Vec::new(),
            outbox: Vec::new(),
        };
        assert!(!room_is_federated(Some(&projection(
            RoomAccessState::Local
        ))));
        assert!(room_is_federated(Some(&projection(RoomAccessState::Live))));
    }

    #[test]
    fn escape_closes_only_an_open_unclaimed_panel() {
        assert!(workspace_panel_escape_closes(true, false));
        assert!(!workspace_panel_escape_closes(false, false));
        assert!(!workspace_panel_escape_closes(true, true));
    }

    #[test]
    fn urls_assert_the_actor_and_encode_both_segments() {
        assert_eq!(
            status_url("http://d", "team room", "user@host"),
            "http://d/v1/rooms/persistent/team%20room/workspace?actor_id=user%40host"
        );
        assert_eq!(
            execs_url("http://d", "k", "a"),
            "http://d/v1/rooms/persistent/k/workspace/execs?actor_id=a&limit=30"
        );
    }
}
