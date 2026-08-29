//! The room's workspace and its command history — the read half of the lane
//! `room_repo.rs` drives.
//!
//! A federated room can have a Bedrock container workspace; every command run
//! in it — a member's exec, a clone, a build — lands in a durable ledger that
//! outlives the container itself. The daemon exposes the reads a room
//! MEMBER gets (`room_workspace_proxy.rs`'s allowlist, `?actor_id=` asserted
//! on every call):
//!
//!   GET /v1/rooms/persistent/{key}/workspace          → status + exec index
//!   GET /v1/rooms/persistent/{key}/workspace/execs    → rows with tails
//!   GET /v1/rooms/persistent/{key}/workspace/list     → one directory's entries
//!   GET /v1/rooms/persistent/{key}/workspace/file     → one file's bounded content
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
//! 5. **A file row opens its content, bounded.** The file route projects the
//!    bytes as JSON — UTF-8 inline, binary as base64 — and text-vs-binary is
//!    the daemon's call, made by decoding the bytes in hand; this panel only
//!    represents it, and never puts base64 in the DOM. A file past the
//!    daemon's 1 MiB relay bound is refused whole with
//!    `workspace_file_too_large`, nothing truncated — too large is a STATE
//!    here, like binary, not an error.
//!
//! Workspace activity also reaches this surface live: the daemon ingests
//! Bedrock's allowlisted `room.workspace.*` ledger rows as System transcript
//! markers ("workspace provisioned", "workspace build 'x' failed", …), and
//! the room-scoped SSE tail delivers them with the rest of the transcript.
//! Those markers are this panel's wake signal — a new one triggers the same
//! silent reads the poller runs, immediately. The open panel still POLLS
//! (the `room_repo` clone-poller idiom: ticket admission, epoch retirement,
//! `(generation, key)` re-validation), but as a SLOW fallback: the tail
//! spends real time in Reconnecting, a deployment can predate the marker
//! lane, and a plain member exec deliberately emits no marker at all — the
//! daemon's allowlist keeps exec chatter off the transcript — so another
//! member's command history only advances on the fallback.
//!
//! A deployment whose daemon or Bedrock predates these routes answers 404
//! with no code; that renders as "not available yet", plainly. Everything
//! that turns a reply into what the operator sees is a free function below,
//! unit-testable natively.

use gloo_net::http::Request;
use leptos::prelude::*;
use serde::{Deserialize, Deserializer};
use wasm_bindgen_futures::spawn_local;

use crate::rooms::{
    encode, RoomAccessProjection, RoomAccessState, RoomMessage, RoomMessageKind, Rooms,
};

/// The open panel's fallback tick. The marker wake is the primary refresh
/// now, so this only has to be honest where the push path is absent — and
/// it is what keeps another member's plain execs advancing at all, which is
/// why it stays minutes-not-hours slow.
const PANEL_POLL_MS: u32 = 10_000;

/// Every status change emits a transcript marker, so the status lane needs
/// the fallback even less than the execs lane does: it rides every Nth tick
/// (~30s) instead of every one.
const STATUS_FALLBACK_EVERY_TICKS: u64 = 3;

/// Every marker the daemon composes opens with this word — nine variants,
/// one prefix (`compose_workspace_marker` in ocean-os's room_federation.rs).
const WORKSPACE_MARKER_PREFIX: &str = "workspace ";

/// Rows asked of the execs route. Bedrock bounds the parameter to 1..=200
/// and defaults to 50; 30 keeps the panel a read, not an archive.
const EXEC_LIST_LIMIT: u32 = 30;

/// Bedrock's build marker (`BUILD_COMMAND_MARKER` in src/room-compute.mjs):
/// a build claims the checkout by holding an exec row whose command opens
/// with this line, which is also what makes builds recognizable here.
const BUILD_COMMAND_MARKER: &str = "# ocean-room-build";

/// Bedrock's CI marker (`CI_COMMAND_MARKER` in src/room-ci.mjs): a CI pull
/// records its gh invocation under this line. Unlike the build marker it
/// guards no claim, so here it is only bookkeeping for the headline to shed.
const CI_COMMAND_MARKER: &str = "# ocean-room-ci";

/// Where the file browse starts and snaps back to on open: bedrock's
/// `WORKSPACE_ROOT` (src/compute/driver.mjs).
const WORKSPACE_ROOT_PATH: &str = "/workspace";

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

/// One row of the list route: the driver contract (`listFiles` in bedrock's
/// src/compute/driver.mjs) — one directory deep, name-sorted, sizes on
/// files only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct FileEntry {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub mtime: Option<String>,
}

/// Bedrock's thrown refusals nest their `code` under `details`; its plain
/// 404s and every daemon-side gate put `code` at the top level. Same dual
/// home as `room_repo`'s, read the same way.
#[derive(Debug, Default, Deserialize)]
struct ErrorDetails {
    #[serde(default)]
    code: Option<String>,
}

/// The lenient envelope all four reads fit into. Presence of `workspace`,
/// `execs`, `entries` or `content` is what success means — the file
/// projection does carry an `ok` field, but leaning on presence keeps the
/// four lanes classified one way.
#[derive(Debug, Default, Deserialize)]
struct WorkspaceBody {
    #[serde(default)]
    workspace: Option<WorkspaceProjection>,
    #[serde(default)]
    execs: Option<Vec<ExecRow>>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    entries: Option<Vec<FileEntry>>,
    /// The file projection's byte count BEFORE encoding — what the operator
    /// should read as the file's size, whatever the base64 cost on the wire.
    #[serde(default)]
    size: Option<u64>,
    /// `"utf8"` or `"base64"` on the file projection.
    #[serde(default)]
    encoding: Option<String>,
    #[serde(default)]
    content: Option<String>,
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

fn files_url(base: &str, key: &str, actor: &str, path: &str) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/workspace/list?actor_id={}&path={}",
        encode(key),
        encode(actor),
        encode(path),
    )
}

/// `path` only — no `inline=`: the daemon's allowlist row forwards nothing
/// else, and this lane never answers a raw download anyway.
fn file_url(base: &str, key: &str, actor: &str, path: &str) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/workspace/file?actor_id={}&path={}",
        encode(key),
        encode(actor),
        encode(path),
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

/// What the listing read answered.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FilesView {
    /// One directory of the tree, as bedrock normalized and sorted it.
    Listing {
        path: String,
        entries: Vec<FileEntry>,
    },
    /// No live container to list (`workspace_absent` or its provisioning /
    /// failed kin): a state, not an error — the status block above the
    /// listing already says why, so this renders nothing of its own.
    NoContainer,
    /// The deployment does not serve the list route.
    Unavailable,
}

/// Map a list-route reply. Unlike the ledger-backed execs route, listing
/// needs the live container, so bedrock refuses here (`workspace_absent`
/// and kin) where the other reads answer.
fn classify_files(status: u16, body: Option<WorkspaceBody>) -> Result<FilesView, String> {
    let Some(body) = body else {
        if status == 404 {
            return Ok(FilesView::Unavailable);
        }
        return Err(format!(
            "The file listing reply could not be read ({status})."
        ));
    };
    if let Some(entries) = body.entries {
        return Ok(FilesView::Listing {
            path: body
                .path
                .filter(|path| !path.is_empty())
                .unwrap_or_else(|| WORKSPACE_ROOT_PATH.to_string()),
            entries,
        });
    }
    match body.refusal_code() {
        Some("workspace_absent" | "workspace_provisioning" | "workspace_failed") => {
            Ok(FilesView::NoContainer)
        }
        // The status read answers NotFederated and hides the section; this
        // arm only keeps the classification total.
        Some("room_not_federated") => Ok(FilesView::NoContainer),
        Some("workspace_route_not_allowed") => Ok(FilesView::Unavailable),
        Some(code) => Err(failure_sentence(code)
            .or_else(|| body.error.clone())
            .unwrap_or_else(|| format!("File listing failed ({status})."))),
        None if status == 404 => Ok(FilesView::Unavailable),
        None => Err(body
            .error
            .filter(|error| !error.is_empty())
            .map(|error| format!("File listing failed: {error}"))
            .unwrap_or_else(|| format!("File listing failed ({status})."))),
    }
}

/// What the file read answered.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FileOpenView {
    /// The daemon decoded the bytes as UTF-8 and sent them inline.
    Text {
        path: String,
        size: Option<u64>,
        content: String,
    },
    /// The bytes did not decode: base64 on the wire, a named fact here —
    /// the content is never decoded into the DOM.
    Binary { path: String, size: Option<u64> },
    /// Past the daemon's 1 MiB relay bound, refused whole. A state — nothing
    /// was truncated, and saying "most of a file" would be lying.
    TooLarge,
    /// No live container to read from; the status block above already says
    /// why, so this renders nothing of its own — same as the listing's arm.
    NoContainer,
    /// The deployment does not serve the file route.
    Unavailable,
}

/// Map a file-route reply. Text-vs-binary is the daemon's call, made by
/// decoding the bytes in hand — the panel only represents it. `path` echoes
/// what was asked (bedrock's normalized form lives in the list rows), and
/// `size` counts bytes before encoding. An unknown refusal code falls to the
/// generic sentence rather than a blank: bedrock can mint new ones.
fn classify_file(status: u16, body: Option<WorkspaceBody>) -> Result<FileOpenView, String> {
    let Some(body) = body else {
        if status == 404 {
            return Ok(FileOpenView::Unavailable);
        }
        return Err(format!("The file reply could not be read ({status})."));
    };
    if let Some(content) = body.content {
        let path = body.path.unwrap_or_default();
        return Ok(match body.encoding.as_deref() {
            Some("base64") => FileOpenView::Binary {
                path,
                size: body.size,
            },
            // `utf8` — and any encoding a future daemon mints reads as
            // text: Leptos escapes text nodes, so the worst case is noise
            // on screen, never markup.
            _ => FileOpenView::Text {
                path,
                size: body.size,
                content,
            },
        });
    }
    match body.refusal_code() {
        Some("workspace_file_too_large") => Ok(FileOpenView::TooLarge),
        Some("workspace_absent" | "workspace_provisioning" | "workspace_failed") => {
            Ok(FileOpenView::NoContainer)
        }
        // The status read answers NotFederated and hides the section; this
        // arm only keeps the classification total.
        Some("room_not_federated") => Ok(FileOpenView::NoContainer),
        Some("workspace_route_not_allowed") => Ok(FileOpenView::Unavailable),
        Some(code) => Err(failure_sentence(code)
            .or_else(|| body.error.clone())
            .unwrap_or_else(|| format!("File read failed ({status})."))),
        None if status == 404 => Ok(FileOpenView::Unavailable),
        None => Err(body
            .error
            .filter(|error| !error.is_empty())
            .map(|error| format!("File read failed: {error}"))
            .unwrap_or_else(|| format!("File read failed ({status})."))),
    }
}

/// The directory above `path`, or `None` at the workspace root. A path that
/// does not sit under the root points back to the root rather than out of
/// the tree — bedrock normalizes what it echoes, but the browse must not
/// bet on it.
fn parent_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix(WORKSPACE_ROOT_PATH)?;
    if rest.is_empty() {
        return None;
    }
    let cut = path.rfind('/')?;
    if cut < WORKSPACE_ROOT_PATH.len() {
        return Some(WORKSPACE_ROOT_PATH.to_string());
    }
    Some(path[..cut].to_string())
}

/// A file's size for its row: bytes plainly, then one decimal per unit.
/// `None` — a directory, or a stat the driver could not take — renders
/// nothing rather than a zero it never measured.
fn entry_size_label(size: Option<u64>) -> Option<String> {
    let size = size?;
    if size < 1024 {
        return Some(format!("{size} B"));
    }
    let kb = size as f64 / 1024.0;
    if kb < 1024.0 {
        return Some(format!("{kb:.1} KB"));
    }
    let mb = kb / 1024.0;
    if mb < 1024.0 {
        return Some(format!("{mb:.1} MB"));
    }
    Some(format!("{:.1} GB", mb / 1024.0))
}

/// The row's title attribute carries what the row itself does not: the full
/// path, and the mtime when the driver took one.
fn entry_title(entry: &FileEntry) -> String {
    match entry.mtime.as_deref().filter(|mtime| !mtime.is_empty()) {
        Some(mtime) => format!("{} \u{b7} {}", entry.path, mtime),
        None => entry.path.clone(),
    }
}

/// Whether an exec row is a build — Bedrock's marker opens the command.
fn is_build(command: &str) -> bool {
    command.starts_with(BUILD_COMMAND_MARKER)
}

/// The one line a row leads with. Build and CI rows shed their marker line
/// — the member wrote `npm run build` or asked gh, not the bookkeeping
/// above it — and a multi-line command shows its first line with an
/// ellipsis; the full text rides the row's title attribute.
fn command_headline(command: &str) -> String {
    let shown = [BUILD_COMMAND_MARKER, CI_COMMAND_MARKER]
        .into_iter()
        .find_map(|marker| command.strip_prefix(marker))
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

/// Whether a transcript row is one of the daemon's workspace markers. The
/// daemon writes them as System rows authored by the lane itself, and every
/// body it composes opens with the same word — member-controlled text can
/// only ever appear inside a marker, never mint one.
pub(crate) fn is_workspace_marker(row: &RoomMessage) -> bool {
    row.kind == RoomMessageKind::System
        && row.author_id == "system"
        && row.body.starts_with(WORKSPACE_MARKER_PREFIX)
}

/// What a transcript observation means for the marker wake: the watermark to
/// store — `(room generation, highest seq seen)` — and whether the rows past
/// the old watermark include a marker worth refreshing for.
///
/// The first sight of a room admission only records the watermark: hydration
/// replays the room's whole history, and waking on it would fire a refresh
/// right after the open-room fetch already read everything. An EMPTY
/// transcript stays uninitialized for the same reason — a just-opened room
/// is cleared before it hydrates, and initializing against the cleared state
/// would make the hydration that follows read as news.
///
/// Shared with `room_repo`, which watches the same transcript for its own
/// subset of markers — `is_marker` is the caller's notion of "worth waking
/// for".
pub(crate) fn marker_wake(
    prior: Option<(u64, u64)>,
    generation: u64,
    transcript: &[RoomMessage],
    is_marker: impl Fn(&RoomMessage) -> bool,
) -> (Option<(u64, u64)>, bool) {
    let Some(latest) = transcript.last().map(|row| row.seq) else {
        return (None, false);
    };
    match prior {
        Some((seen_generation, seen)) if seen_generation == generation => {
            // New rows sit at the tail; the transcript is ascending by seq.
            let wake = transcript
                .iter()
                .rev()
                .take_while(|row| row.seq > seen)
                .any(is_marker);
            (Some((generation, latest.max(seen))), wake)
        }
        _ => (Some((generation, latest)), false),
    }
}

/// Whether this fallback tick reads the status lane too, or only execs.
fn fallback_reads_status(tick: u64) -> bool {
    tick % STATUS_FALLBACK_EVERY_TICKS == 0
}

/// The read lanes this panel runs. A standing error remembers which lane
/// set it, so one lane's recovery can never wipe another's live failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadLane {
    Status,
    Execs,
    Files,
    File,
}

/// Whether a lane's successful read clears the standing error: only the one
/// its own lane set. This is the absorbed fix — a blipped silent poll used
/// to leave its error standing forever over a self-healed view, and the
/// naive "success clears" would let a status success wipe a live execs
/// failure.
fn lane_success_clears(standing: Option<ReadLane>, lane: ReadLane) -> bool {
    standing == Some(lane)
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
    /// What the listing read answered, for `files_path`. Same open-only
    /// lifecycle as the history.
    files: RwSignal<Option<FilesView>>,
    /// The directory being browsed. What the silent refresh re-reads, so a
    /// flush updates the listing the operator is actually looking at.
    files_path: RwSignal<String>,
    /// The file open over the listing, `None` when the browse is on screen.
    /// Set the moment a row is clicked — the sub-view exists while the read
    /// is still in flight — and the listing under it stands untouched, so
    /// the back affordance returns instantly.
    open_file: RwSignal<Option<String>>,
    /// What the file read answered. `None` = not answered yet.
    file: RwSignal<Option<FileOpenView>>,
    /// A foreground status read is in flight (the poller refreshes silently).
    loading: RwSignal<bool>,
    /// The most recent failure, tagged with the lane that set it so the
    /// other lane's success cannot clear it.
    error: RwSignal<Option<(ReadLane, String)>>,
    /// The marker wake's watermark: `(room generation, highest transcript
    /// seq seen)`. `None` until the open room's transcript is first sighted.
    marker_seen: RwSignal<Option<(u64, u64)>>,
    /// Whether the reading-measure panel is open.
    panel: RwSignal<bool>,
    /// The rail control that opens the panel, so closing hands focus back.
    open_ref: NodeRef<leptos::html::Button>,
    /// Monotonic ticket per read lane; only the latest overlapping read may
    /// publish.
    ticket: RwSignal<u64>,
    execs_ticket: RwSignal<u64>,
    files_ticket: RwSignal<u64>,
    file_ticket: RwSignal<u64>,
    /// Poller generation; bumping it retires any running poll loop.
    poll_epoch: RwSignal<u64>,
}

impl RoomWorkspacePanelState {
    pub fn new(rooms: &Rooms) -> Self {
        Self {
            url: rooms.url,
            view: RwSignal::new(None),
            execs: RwSignal::new(None),
            files: RwSignal::new(None),
            files_path: RwSignal::new(WORKSPACE_ROOT_PATH.to_string()),
            open_file: RwSignal::new(None),
            file: RwSignal::new(None),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            marker_seen: RwSignal::new(None),
            panel: RwSignal::new(false),
            open_ref: NodeRef::new(),
            ticket: RwSignal::new(0),
            execs_ticket: RwSignal::new(0),
            files_ticket: RwSignal::new(0),
            file_ticket: RwSignal::new(0),
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
        self.files_ticket
            .update(|ticket| *ticket = ticket.wrapping_add(1));
        self.file_ticket
            .update(|ticket| *ticket = ticket.wrapping_add(1));
        self.poll_epoch
            .update(|epoch| *epoch = epoch.wrapping_add(1));
        self.view.set(None);
        self.execs.set(None);
        self.files.set(None);
        self.files_path.set(WORKSPACE_ROOT_PATH.to_string());
        self.open_file.set(None);
        self.file.set(None);
        self.loading.set(false);
        self.error.set(None);
        self.marker_seen.set(None);
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
            Ok(view) => {
                self.view.set(Some(view));
                self.clear_lane_error(ReadLane::Status);
            }
            // A failed refresh never blanks a standing view: what the
            // operator was reading is still the best answer this surface has.
            Err(error) => self.error.set(Some((ReadLane::Status, error))),
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
            Ok(view) => {
                self.execs.set(Some(view));
                self.clear_lane_error(ReadLane::Execs);
            }
            Err(error) => self.error.set(Some((ReadLane::Execs, error))),
        }
    }

    /// Read one directory of the tree, foreground — the open and every
    /// navigation land here.
    fn fetch_files(&self, key: String, actor: String, path: String) {
        let base = self.base();
        let me = *self;
        let ticket = self.files_ticket.get_untracked().wrapping_add(1);
        self.files_ticket.set(ticket);
        spawn_local(async move {
            let result = read_files(&base, &key, &actor, &path).await;
            me.publish_files(
                result,
                read_is_current(ticket, me.files_ticket.get_untracked()),
            );
        });
    }

    /// Publish a completed listing read — but only the latest one.
    fn publish_files(&self, result: Result<FilesView, String>, is_current: bool) {
        if !is_current {
            return;
        }
        match result {
            Ok(view) => {
                self.files.set(Some(view));
                self.clear_lane_error(ReadLane::Files);
            }
            Err(error) => self.error.set(Some((ReadLane::Files, error))),
        }
    }

    /// Browse into another directory: remember the path — the silent
    /// refresh re-reads what is on screen — then read it.
    fn navigate_files(&self, key: String, actor: String, path: String) {
        self.files_path.set(path.clone());
        self.fetch_files(key, actor, path);
    }

    /// Read one file, foreground — a file row's click lands here.
    fn fetch_file(&self, key: String, actor: String, path: String) {
        let base = self.base();
        let me = *self;
        let ticket = self.file_ticket.get_untracked().wrapping_add(1);
        self.file_ticket.set(ticket);
        spawn_local(async move {
            let result = read_file(&base, &key, &actor, &path).await;
            me.publish_file(
                result,
                read_is_current(ticket, me.file_ticket.get_untracked()),
            );
        });
    }

    /// Publish a completed file read — but only the latest one.
    fn publish_file(&self, result: Result<FileOpenView, String>, is_current: bool) {
        if !is_current {
            return;
        }
        match result {
            Ok(view) => {
                self.file.set(Some(view));
                self.clear_lane_error(ReadLane::File);
            }
            Err(error) => self.error.set(Some((ReadLane::File, error))),
        }
    }

    /// Open one file over the listing. The listing itself is left standing —
    /// the back affordance returns to it without another read.
    fn view_file(&self, key: String, actor: String, path: String) {
        self.open_file.set(Some(path.clone()));
        self.file.set(None);
        self.fetch_file(key, actor, path);
    }

    /// Back to the listing. The ticket bump retires an in-flight read so a
    /// slow answer cannot resurrect a view the operator already left, and a
    /// standing file error leaves with the lane it belongs to — abandoned,
    /// the lane would never answer again to clear its own banner.
    fn close_file(&self) {
        self.file_ticket
            .update(|ticket| *ticket = ticket.wrapping_add(1));
        self.open_file.set(None);
        self.file.set(None);
        self.clear_lane_error(ReadLane::File);
    }

    /// A lane that answered clears the error IT set, and only that one — a
    /// healthy status read must not wipe a live execs failure, or the other
    /// way round.
    fn clear_lane_error(&self, lane: ReadLane) {
        let clears = self
            .error
            .with_untracked(|slot| lane_success_clears(slot.as_ref().map(|(lane, _)| *lane), lane));
        if clears {
            self.error.set(None);
        }
    }

    /// Open the panel: read every lane fresh — a reopened panel shows the
    /// room as it is, not as it was when last opened, and the file browse
    /// starts over at the root — and start the poll loop that keeps it
    /// that way.
    fn open_panel(&self, rooms: Rooms, key: String, actor: String) {
        self.error.set(None);
        self.panel.set(true);
        self.files_path.set(WORKSPACE_ROOT_PATH.to_string());
        // A file left open when the panel last closed does not survive the
        // reopen: the browse starts over at the listing.
        self.close_file();
        self.fetch_status(key.clone(), actor.clone());
        self.fetch_execs(key.clone(), actor.clone());
        self.fetch_files(key.clone(), actor.clone(), WORKSPACE_ROOT_PATH.to_string());
        self.poll_while_open(rooms, key, actor);
    }

    /// The fallback poll while the panel is open. Reads silently — no
    /// loading flicker — and publishes through the same ticket admission as
    /// every other read, so an overlapping foreground fetch still wins. The
    /// epoch and `(generation, key)` checks are what keep a loop from
    /// surviving its room or doubling up with a successor. Execs every tick
    /// (plain execs have no marker, this loop is their only truth); status
    /// and the file listing every Nth — every status change mints a marker,
    /// and so does every write that reshapes the tree except a plain exec's,
    /// which this slow tick backstops.
    fn poll_while_open(&self, rooms: Rooms, key: String, actor: String) {
        let epoch = self.poll_epoch.get_untracked().wrapping_add(1);
        self.poll_epoch.set(epoch);
        let base = self.base();
        let me = *self;
        let generation = rooms.generation_snapshot();
        spawn_local(async move {
            let mut tick: u64 = 0;
            loop {
                gloo_timers::future::TimeoutFuture::new(PANEL_POLL_MS).await;
                if me.poll_epoch.get_untracked() != epoch
                    || !rooms.room_is_current(generation, &key)
                    || !me.panel.get_untracked()
                {
                    return;
                }
                tick = tick.wrapping_add(1);
                let slow = fallback_reads_status(tick);
                refresh_lanes(me, &base, &key, &actor, slow, true, slow).await;
            }
        });
    }

    /// A workspace marker just landed on the transcript: refresh now instead
    /// of waiting out the fallback tick. Status always — the rail glance is
    /// on screen with the panel closed — and the history and listing only
    /// where they render; an opening panel fetches everything fresh anyway.
    fn refresh_on_marker(&self, key: String, actor: String) {
        let base = self.base();
        let me = *self;
        let open = self.panel.get_untracked();
        spawn_local(async move {
            refresh_lanes(me, &base, &key, &actor, true, open, open).await;
        });
    }
}

/// The silent refresh the fallback poller and the marker wake share: read
/// the named lanes, publish through ticket admission. One shape for both
/// callers, so they cannot drift apart about what "refresh" means.
async fn refresh_lanes(
    me: RoomWorkspacePanelState,
    base: &str,
    key: &str,
    actor: &str,
    status: bool,
    execs: bool,
    files: bool,
) {
    if status {
        let ticket = me.ticket.get_untracked().wrapping_add(1);
        me.ticket.set(ticket);
        let result = read_workspace(base, key, actor).await;
        me.publish_status(result, read_is_current(ticket, me.ticket.get_untracked()));
    }
    if execs {
        let ticket = me.execs_ticket.get_untracked().wrapping_add(1);
        me.execs_ticket.set(ticket);
        let result = read_execs(base, key, actor).await;
        me.publish_execs(
            result,
            read_is_current(ticket, me.execs_ticket.get_untracked()),
        );
    }
    if files {
        // The path the operator is browsing NOW — a navigation that lands
        // mid-refresh retires this read at the ticket gate.
        let path = me.files_path.get_untracked();
        let ticket = me.files_ticket.get_untracked().wrapping_add(1);
        me.files_ticket.set(ticket);
        let result = read_files(base, key, actor, &path).await;
        me.publish_files(
            result,
            read_is_current(ticket, me.files_ticket.get_untracked()),
        );
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

async fn read_files(base: &str, key: &str, actor: &str, path: &str) -> Result<FilesView, String> {
    let url = files_url(base, key, actor, path);
    match Request::get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.json::<WorkspaceBody>().await.ok();
            classify_files(status, body)
        }
        Err(err) => Err(format!("File listing request failed: {err}")),
    }
}

async fn read_file(base: &str, key: &str, actor: &str, path: &str) -> Result<FileOpenView, String> {
    let url = file_url(base, key, actor, path);
    match Request::get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.json::<WorkspaceBody>().await.ok();
            classify_file(status, body)
        }
        Err(err) => Err(format!("File read request failed: {err}")),
    }
}

// ---- Component --------------------------------------------------------------

/// The open room's workspace: a compact rail row with a glance line, and a
/// panel where the provision facts, the file tree and the command history
/// are read.
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

    // The wake path: watch the transcript the SSE tail feeds for new
    // workspace markers and refresh on each one, so the panel reflects a
    // provision, clone or build the moment the room hears about it instead
    // of a fallback tick later. The watermark starts over with the panel's
    // reset() lifecycle AND carries the room generation, so whichever of the
    // two Effects runs first on a room switch, hydration reads as the
    // initial load, never as news.
    Effect::new(move |_| {
        let (watermark, wake) = rooms.transcript.with(|transcript| {
            marker_wake(
                state.marker_seen.get_untracked(),
                rooms.generation_snapshot(),
                transcript,
                is_workspace_marker,
            )
        });
        state.marker_seen.set(watermark);
        if !wake {
            return;
        }
        let Some((key, actor)) = read_target.get_untracked() else {
            return;
        };
        state.refresh_on_marker(key, actor);
    });

    // The one place the open action resolves the room key and the actor
    // together; the identity refusal is the composer's, in its words. Tagged
    // as the status lane's: it stands in for the reads the open would have
    // run, and any read that succeeds proves it moot.
    let actor = move || -> Option<(String, String)> {
        let key = rooms
            .open_key
            .get_untracked()
            .filter(|key| !key.is_empty())?;
        if !rooms.identity_resolved() {
            state.error.set(Some((
                ReadLane::Status,
                "Still signing in \u{2014} try again in a moment.".to_string(),
            )));
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
                        state.error.get().map(|(_, error)| view! {
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
                                        state.error.get().map(|(_, error)| view! {
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
                                    {move || files_section(state, actor)}
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

/// The workspace tree, one directory at a time — what the list route feeds.
/// A room with no live container renders nothing here: the status block
/// above already says so in its own words. An open file replaces the
/// listing with its own sub-view until the back affordance clears it.
fn files_section(
    state: RoomWorkspacePanelState,
    actor: impl Fn() -> Option<(String, String)> + Copy + 'static,
) -> AnyView {
    if let Some(requested) = state.open_file.get() {
        return file_open_section(state, requested);
    }
    let (path, entries) = match state.files.get() {
        Some(FilesView::Listing { path, entries }) => (path, entries),
        Some(FilesView::NoContainer) => return ().into_any(),
        Some(FilesView::Unavailable) => {
            return view! {
                <div class="rooms-workspace__compute-note">
                    "File listing isn't available on this deployment yet."
                </div>
            }
            .into_any();
        }
        // Not answered yet — the panel always asks on open.
        None => {
            return view! {
                <div class="rooms-workspace__compute-note">"Reading files\u{2026}"</div>
            }
            .into_any();
        }
    };
    let up = parent_path(&path);
    view! {
        <div class="rooms-workspace__compute-files-title">"Files"</div>
        <div class="rooms-workspace__compute-files-path">
            {up.map(|parent| view! {
                <button
                    class="rooms-workspace__compute-files-up"
                    type="button"
                    title="Up one directory"
                    aria-label="Up one directory"
                    on:click=move |_| {
                        let Some((key, actor_id)) = actor() else { return };
                        state.navigate_files(key, actor_id, parent.clone());
                    }
                >
                    "\u{2191}"
                </button>
            })}
            <span class="rooms-workspace__compute-files-dir">{path}</span>
        </div>
        {if entries.is_empty() {
            view! {
                <div class="rooms-workspace__compute-note">"This directory is empty."</div>
            }
            .into_any()
        } else {
            entries
                .into_iter()
                .map(|entry| file_row_view(state, actor, entry))
                .collect::<Vec<_>>()
                .into_any()
        }}
    }
    .into_any()
}

/// The open file, in the listing's place: the same path line idiom with the
/// back affordance where "up" sits in the browse, then what the read
/// answered — content, or the state that stands for it.
fn file_open_section(state: RoomWorkspacePanelState, requested: String) -> AnyView {
    let answered = state.file.get();
    // The daemon echoes the path that was asked; while the read is in
    // flight (or refused), the ask itself is the honest label.
    let shown_path = match &answered {
        Some(FileOpenView::Text { path, .. } | FileOpenView::Binary { path, .. })
            if !path.is_empty() =>
        {
            path.clone()
        }
        _ => requested,
    };
    let body = match answered {
        // Not answered yet — the click always asks.
        None => view! {
            <div class="rooms-workspace__compute-note">"Reading file\u{2026}"</div>
        }
        .into_any(),
        Some(FileOpenView::Text { size, content, .. }) => view! {
            {entry_size_label(size).map(|size| view! {
                <div class="rooms-workspace__compute-file-meta">{size}</div>
            })}
            <pre class="rooms-workspace__compute-file-text">{content}</pre>
        }
        .into_any(),
        Some(FileOpenView::Binary { size, .. }) => view! {
            <div class="rooms-workspace__compute-note">
                {match entry_size_label(size) {
                    Some(size) => {
                        format!("Binary file, {size} \u{2014} this panel shows text only.")
                    }
                    None => "Binary file \u{2014} this panel shows text only.".to_string(),
                }}
            </div>
        }
        .into_any(),
        Some(FileOpenView::TooLarge) => view! {
            <div class="rooms-workspace__compute-note">
                "This file is larger than the 1 MiB the daemon relays \u{2014} \
                 too large to open here. Nothing was truncated."
            </div>
        }
        .into_any(),
        // The status block above already says why there is no container —
        // the same silence as the listing's arm.
        Some(FileOpenView::NoContainer) => ().into_any(),
        Some(FileOpenView::Unavailable) => view! {
            <div class="rooms-workspace__compute-note">
                "Opening files isn't available on this deployment yet."
            </div>
        }
        .into_any(),
    };
    view! {
        <div class="rooms-workspace__compute-files-title">"Files"</div>
        <div class="rooms-workspace__compute-files-path">
            <button
                class="rooms-workspace__compute-files-up"
                type="button"
                title="Back to the file listing"
                aria-label="Back to the file listing"
                on:click=move |_| state.close_file()
            >
                "\u{2190}"
            </button>
            <span class="rooms-workspace__compute-files-dir">{shown_path}</span>
        </div>
        {body}
    }
    .into_any()
}

/// One entry, and both kinds are buttons now: a directory row IS the
/// navigation, and a file row opens its bounded content over the listing.
fn file_row_view(
    state: RoomWorkspacePanelState,
    actor: impl Fn() -> Option<(String, String)> + Copy + 'static,
    entry: FileEntry,
) -> AnyView {
    let title = entry_title(&entry);
    if entry.kind == "directory" {
        let target = entry.path.clone();
        view! {
            <button
                class="rooms-workspace__compute-file rooms-workspace__compute-file--dir"
                type="button"
                title=title
                on:click=move |_| {
                    let Some((key, actor_id)) = actor() else { return };
                    state.navigate_files(key, actor_id, target.clone());
                }
            >
                <span class="rooms-workspace__compute-file-kind">"dir"</span>
                <span class="rooms-workspace__compute-file-name">{entry.name}</span>
            </button>
        }
        .into_any()
    } else {
        let size = entry_size_label(entry.size);
        let target = entry.path.clone();
        view! {
            <button
                class="rooms-workspace__compute-file rooms-workspace__compute-file--openable"
                type="button"
                title=title
                on:click=move |_| {
                    let Some((key, actor_id)) = actor() else { return };
                    state.view_file(key, actor_id, target.clone());
                }
            >
                <span class="rooms-workspace__compute-file-kind">"file"</span>
                <span class="rooms-workspace__compute-file-name">{entry.name}</span>
                {size.map(|size| view! {
                    <span class="rooms-workspace__compute-file-size">{size}</span>
                })}
            </button>
        }
        .into_any()
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
            files: RwSignal::new(None),
            files_path: RwSignal::new(WORKSPACE_ROOT_PATH.to_string()),
            open_file: RwSignal::new(None),
            file: RwSignal::new(None),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            marker_seen: RwSignal::new(None),
            panel: RwSignal::new(false),
            open_ref: NodeRef::new(),
            ticket: RwSignal::new(0),
            execs_ticket: RwSignal::new(0),
            files_ticket: RwSignal::new(0),
            file_ticket: RwSignal::new(0),
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

    // ---- the files wire -----------------------------------------------------

    /// Bedrock's list body: the normalized path echoed back, one directory
    /// of driver-sorted entries, sizes on files only.
    #[test]
    fn a_listing_carries_its_path_and_rows() {
        let listed = body(
            r#"{"path": "/workspace", "entries": [
                {"name": ".git", "path": "/workspace/.git", "type": "directory",
                 "size": null, "mtime": "2026-08-27T10:00:00.000Z"},
                {"name": "package.json", "path": "/workspace/package.json",
                 "type": "file", "size": 417, "mtime": "2026-08-27T10:01:00.000Z"}]}"#,
        );
        let FilesView::Listing { path, entries } = classify_files(200, Some(listed)).unwrap()
        else {
            panic!("expected a listing");
        };
        assert_eq!(path, "/workspace");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].kind, "directory");
        assert_eq!(entries[0].size, None);
        assert_eq!(entries[1].kind, "file");
        assert_eq!(entries[1].size, Some(417));
    }

    /// Listing needs the live container, so bedrock refuses where the
    /// ledger-backed reads answer: absent and its provisioning/failed kin
    /// are states the status block already explains, never errors; gates
    /// and route-less deployments read as they do on every other lane.
    #[test]
    fn files_states_classify_totally() {
        for code in [
            "workspace_absent",
            "workspace_provisioning",
            "workspace_failed",
        ] {
            let refused = body(&format!(r#"{{"error": "no", "code": "{code}"}}"#));
            assert_eq!(
                classify_files(409, Some(refused)),
                Ok(FilesView::NoContainer),
                "{code} must be a state"
            );
        }
        assert_eq!(classify_files(404, None), Ok(FilesView::Unavailable));
        let coded = body(r#"{"ok": false, "code": "workspace_route_not_allowed"}"#);
        assert_eq!(classify_files(404, Some(coded)), Ok(FilesView::Unavailable));
        let gated = body(r#"{"ok": false, "code": "not_a_room_member"}"#);
        assert_eq!(
            classify_files(403, Some(gated)),
            Err("You're not on this room's roster.".to_string())
        );
    }

    /// The browse climbs one directory at a time and stops at the root; a
    /// path from outside the tree has no parent to offer.
    #[test]
    fn navigation_climbs_to_the_root_and_stops() {
        assert_eq!(parent_path("/workspace"), None);
        assert_eq!(parent_path("/workspace/src").as_deref(), Some("/workspace"));
        assert_eq!(
            parent_path("/workspace/src/components").as_deref(),
            Some("/workspace/src")
        );
        assert_eq!(parent_path(""), None);
        assert_eq!(parent_path("/elsewhere"), None);
    }

    /// Sizes read as a human line; a directory's `null` renders nothing
    /// rather than a zero the driver never measured.
    #[test]
    fn sizes_label_bytes_kb_mb() {
        assert_eq!(entry_size_label(None), None);
        assert_eq!(entry_size_label(Some(0)).as_deref(), Some("0 B"));
        assert_eq!(entry_size_label(Some(1023)).as_deref(), Some("1023 B"));
        assert_eq!(entry_size_label(Some(1536)).as_deref(), Some("1.5 KB"));
        assert_eq!(
            entry_size_label(Some(5 * 1024 * 1024)).as_deref(),
            Some("5.0 MB")
        );
        assert_eq!(
            entry_size_label(Some(3 * 1024 * 1024 * 1024)).as_deref(),
            Some("3.0 GB")
        );
    }

    /// The row's title carries what the row itself does not: the full path,
    /// and the mtime when the driver took one.
    #[test]
    fn the_row_title_carries_path_and_mtime() {
        let entry = FileEntry {
            name: "package.json".to_string(),
            path: "/workspace/package.json".to_string(),
            kind: "file".to_string(),
            size: Some(417),
            mtime: Some("2026-08-27T10:01:00.000Z".to_string()),
        };
        assert_eq!(
            entry_title(&entry),
            "/workspace/package.json \u{b7} 2026-08-27T10:01:00.000Z"
        );
        let bare = FileEntry {
            mtime: None,
            ..entry
        };
        assert_eq!(entry_title(&bare), "/workspace/package.json");
    }

    #[test]
    fn the_files_url_asserts_the_actor_and_encodes_the_path() {
        assert_eq!(
            files_url("http://d", "k", "a", "/workspace/my dir"),
            "http://d/v1/rooms/persistent/k/workspace/list?actor_id=a&path=%2Fworkspace%2Fmy%20dir"
        );
    }

    /// The files lane keeps the same isolation contract as the other two:
    /// a failed refresh keeps the standing listing, its failure stands
    /// through the other lanes' successes, and its own success clears it.
    #[test]
    fn the_files_lane_errors_stay_isolated() {
        let state = fresh_state();
        let listing = FilesView::Listing {
            path: "/workspace".to_string(),
            entries: Vec::new(),
        };
        state.files.set(Some(listing.clone()));
        state.publish_files(Err("files down".to_string()), true);
        assert_eq!(state.files.get_untracked(), Some(listing.clone()));
        state.publish_status(Ok(WorkspaceView::Absent), true);
        state.publish_execs(Ok(ExecsView::Rows(Vec::new())), true);
        assert_eq!(
            state.error.get_untracked(),
            Some((ReadLane::Files, "files down".to_string())),
            "the other lanes' successes must not wipe a live files failure"
        );
        state.publish_files(Ok(listing), true);
        assert_eq!(state.error.get_untracked(), None);
    }

    // ---- the file wire ------------------------------------------------------

    /// `path` only on the query string — the daemon's allowlist forwards
    /// nothing else, so an `inline=` here would be silently dropped anyway.
    #[test]
    fn the_file_url_asserts_the_actor_and_sends_path_only() {
        assert_eq!(
            file_url("http://d", "k", "a", "/workspace/my dir/README.md"),
            "http://d/v1/rooms/persistent/k/workspace/file?actor_id=a\
             &path=%2Fworkspace%2Fmy%20dir%2FREADME.md"
        );
    }

    /// The daemon's `project_file` on bytes that decode: content inline,
    /// `size` counting bytes before encoding, `path` echoing the ask.
    #[test]
    fn a_utf8_file_opens_as_text() {
        let projected = body(
            r#"{"ok": true, "path": "/workspace/README.md", "size": 12,
                "encoding": "utf8", "content": "hello ocean\n"}"#,
        );
        assert_eq!(
            classify_file(200, Some(projected)),
            Ok(FileOpenView::Text {
                path: "/workspace/README.md".to_string(),
                size: Some(12),
                content: "hello ocean\n".to_string(),
            })
        );
    }

    /// Binary arrives as base64, never a refusal — and classifies to a named
    /// state that carries the pre-encoding size, not the payload.
    #[test]
    fn a_binary_file_is_a_named_state() {
        let projected = body(
            r#"{"ok": true, "path": "/workspace/logo.png", "size": 4,
                "encoding": "base64", "content": "AAEC/w=="}"#,
        );
        assert_eq!(
            classify_file(200, Some(projected)),
            Ok(FileOpenView::Binary {
                path: "/workspace/logo.png".to_string(),
                size: Some(4),
            })
        );
    }

    /// The daemon's 413: over its 1 MiB relay bound, refused whole. A STATE
    /// — nothing was truncated, and the panel says so instead of erroring.
    #[test]
    fn a_too_large_file_is_a_state_not_an_error() {
        let refused = body(
            r#"{"ok": false, "code": "workspace_file_too_large",
                "error": "this file is larger than the 1 MiB the daemon will relay; nothing was truncated"}"#,
        );
        assert_eq!(
            classify_file(413, Some(refused)),
            Ok(FileOpenView::TooLarge)
        );
    }

    /// The rest of the lane classifies like its siblings: no container is a
    /// state the status block explains, a route-less deployment is said
    /// plainly, a gate refusal fails in words, and a code this panel has
    /// never seen falls to the body's own sentence — bedrock can mint new
    /// ones.
    #[test]
    fn file_states_classify_totally() {
        let absent =
            body(r#"{"error": "This room has no workspace.", "code": "workspace_absent"}"#);
        assert_eq!(
            classify_file(404, Some(absent)),
            Ok(FileOpenView::NoContainer)
        );
        assert_eq!(classify_file(404, None), Ok(FileOpenView::Unavailable));
        let coded = body(r#"{"ok": false, "code": "workspace_route_not_allowed"}"#);
        assert_eq!(
            classify_file(404, Some(coded)),
            Ok(FileOpenView::Unavailable)
        );
        let gated = body(r#"{"ok": false, "code": "not_a_room_member"}"#);
        assert_eq!(
            classify_file(403, Some(gated)),
            Err("You're not on this room's roster.".to_string())
        );
        let minted = body(
            r#"{"ok": false, "code": "workspace_read_capped",
                "error": "reads are capped on this workspace"}"#,
        );
        assert_eq!(
            classify_file(403, Some(minted)),
            Err("reads are capped on this workspace".to_string())
        );
    }

    /// The file lane keeps the shared error contract: a failed read keeps
    /// the standing content, its failure stands through the other lanes'
    /// successes, and its own success clears it.
    #[test]
    fn the_file_lane_errors_stay_isolated() {
        let state = fresh_state();
        let open = FileOpenView::Text {
            path: "/workspace/README.md".to_string(),
            size: Some(12),
            content: "hello ocean\n".to_string(),
        };
        state.file.set(Some(open.clone()));
        state.publish_file(Err("file down".to_string()), true);
        assert_eq!(state.file.get_untracked(), Some(open.clone()));
        state.publish_files(
            Ok(FilesView::Listing {
                path: WORKSPACE_ROOT_PATH.to_string(),
                entries: Vec::new(),
            }),
            true,
        );
        assert_eq!(
            state.error.get_untracked(),
            Some((ReadLane::File, "file down".to_string())),
            "a listing success must not wipe a live file failure"
        );
        state.publish_file(Ok(open), true);
        assert_eq!(state.error.get_untracked(), None);
    }

    /// Back returns to the listing the operator was browsing — still
    /// standing, untouched by the open — retires an in-flight read so a
    /// slow answer cannot resurrect the sub-view, and takes the lane's
    /// standing error with it: a failed open must not leave a permanent
    /// banner over a healthy listing.
    #[test]
    fn closing_the_file_returns_to_the_standing_listing() {
        let state = fresh_state();
        state.files.set(Some(FilesView::Listing {
            path: WORKSPACE_ROOT_PATH.to_string(),
            entries: Vec::new(),
        }));
        state
            .open_file
            .set(Some("/workspace/README.md".to_string()));
        let ticket = state.file_ticket.get_untracked();
        state.publish_file(Ok(FileOpenView::TooLarge), true);
        assert_eq!(state.file.get_untracked(), Some(FileOpenView::TooLarge));
        state
            .error
            .set(Some((ReadLane::File, "file down".to_string())));
        state.close_file();
        assert_eq!(state.open_file.get_untracked(), None);
        assert_eq!(state.file.get_untracked(), None);
        assert_eq!(
            state.error.get_untracked(),
            None,
            "an abandoned lane's error must not stand over the listing"
        );
        assert!(
            !read_is_current(ticket, state.file_ticket.get_untracked()),
            "closing must retire the lane's in-flight read"
        );
        assert!(
            matches!(state.files.get_untracked(), Some(FilesView::Listing { .. })),
            "the listing under the sub-view must stand"
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

    /// A CI pull's row headlines the gh line the pull actually ran, not the
    /// bookkeeping marker above it — the same shed the build marker gets.
    #[test]
    fn a_ci_row_headlines_its_gh_line() {
        let command = "# ocean-room-ci\ngh run list --branch 'main' --status completed";
        assert_eq!(
            command_headline(command),
            "gh run list --branch 'main' --status completed"
        );
        assert!(!is_build(command), "a CI pull is not a build");
        // Same self-naming rule as the build marker.
        assert_eq!(command_headline("# ocean-room-ci"), "# ocean-room-ci");
    }

    /// The last-build sentence keys on the build marker alone. Today the two
    /// markers' prefixes diverge ("# ocean-room-b…" vs "# ocean-room-ci"),
    /// so a CI pull cannot masquerade as a build; this test exists so no
    /// refactor of the marker handling ever lets one.
    #[test]
    fn a_ci_pull_never_captures_the_last_build_sentence() {
        let exec = |command: &str| ExecRow {
            command: command.to_string(),
            status: "exited".to_string(),
            exit_code: Some(0),
            ..ExecRow::default()
        };
        let rows = vec![
            exec("# ocean-room-ci\ngh run list --branch 'main' --status completed"),
            exec("# ocean-room-build\nnpm run build"),
        ];
        assert_eq!(
            last_build_sentence(&rows).as_deref(),
            Some("Last build succeeded \u{2014} npm run build."),
            "a newer CI pull must not shadow the real last build"
        );
        assert_eq!(
            last_build_sentence(&rows[..1]).as_deref(),
            None,
            "a list holding only CI pulls claims no build at all"
        );
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

        state.publish_files(Ok(FilesView::NoContainer), false);
        assert_eq!(state.files.get_untracked(), None);

        state.publish_file(Ok(FileOpenView::TooLarge), false);
        assert_eq!(state.file.get_untracked(), None);
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
        assert_eq!(
            state.error.get_untracked(),
            Some((ReadLane::Execs, "boom".to_string()))
        );
    }

    /// The absorbed fix, both halves: a lane that recovers clears the error
    /// it set — one blipped silent poll no longer leaves a standing alert
    /// over a healthy view — and only that one, so a status success can
    /// never wipe a live execs failure.
    #[test]
    fn a_lane_success_clears_only_the_error_its_own_lane_set() {
        let state = fresh_state();
        state.publish_status(Err("status blip".to_string()), true);
        state.publish_status(Ok(WorkspaceView::Absent), true);
        assert_eq!(state.error.get_untracked(), None);

        state.publish_execs(Err("execs down".to_string()), true);
        state.publish_status(Ok(WorkspaceView::Absent), true);
        assert_eq!(
            state.error.get_untracked(),
            Some((ReadLane::Execs, "execs down".to_string())),
            "a status success must not wipe a live execs failure"
        );
        state.publish_execs(Ok(ExecsView::Rows(Vec::new())), true);
        assert_eq!(state.error.get_untracked(), None);

        assert!(lane_success_clears(
            Some(ReadLane::Status),
            ReadLane::Status
        ));
        assert!(!lane_success_clears(
            Some(ReadLane::Execs),
            ReadLane::Status
        ));
        assert!(!lane_success_clears(None, ReadLane::Execs));
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

    // ---- the marker wake ----------------------------------------------------

    fn row(seq: u64, kind: RoomMessageKind, author_id: &str, body: &str) -> RoomMessage {
        RoomMessage {
            seq,
            author_id: author_id.into(),
            author_kind: crate::rooms::RoomParticipantKind::System,
            kind,
            body: body.into(),
            created_at: String::new(),
            federated: None,
            thread_parent_seq: None,
            attachment_id: None,
        }
    }

    /// Only the daemon's own markers count: System kind, the lane's author,
    /// the composed prefix. A member TYPING "workspace provisioned" is a
    /// Message row and must not trigger reads.
    #[test]
    fn only_a_system_workspace_row_is_a_marker() {
        assert!(is_workspace_marker(&row(
            1,
            RoomMessageKind::System,
            "system",
            "workspace provisioned (cloudflare, 12 files hydrated)"
        )));
        assert!(is_workspace_marker(&row(
            2,
            RoomMessageKind::System,
            "system",
            "workspace build 'build' failed (exit 1, 32.4s)"
        )));
        assert!(!is_workspace_marker(&row(
            3,
            RoomMessageKind::Message,
            "user",
            "workspace provisioned"
        )));
        assert!(!is_workspace_marker(&row(
            4,
            RoomMessageKind::System,
            "system",
            "attachment notes.md uploaded"
        )));
        assert!(!is_workspace_marker(&row(
            5,
            RoomMessageKind::System,
            "user",
            "workspace provisioned"
        )));
    }

    /// The wake fires on a NEW marker and nothing else: not on the initial
    /// hydration (the whole history is "new" then), not on a plain message,
    /// not twice for the same marker, and not across a room switch.
    #[test]
    fn a_new_marker_wakes_and_the_initial_load_does_not() {
        let history = vec![
            row(1, RoomMessageKind::Message, "user", "hello"),
            row(
                2,
                RoomMessageKind::System,
                "system",
                "workspace provisioned",
            ),
        ];
        // First sight of the room: record only, however marker-laden.
        let (watermark, wake) = marker_wake(None, 7, &history, is_workspace_marker);
        assert_eq!(watermark, Some((7, 2)));
        assert!(!wake);

        // A plain message arrives: watermark advances, no wake.
        let mut transcript = history.clone();
        transcript.push(row(3, RoomMessageKind::Message, "user", "any news?"));
        let (watermark, wake) = marker_wake(watermark, 7, &transcript, is_workspace_marker);
        assert_eq!(watermark, Some((7, 3)));
        assert!(!wake);

        // A marker arrives: wake, once.
        transcript.push(row(
            4,
            RoomMessageKind::System,
            "system",
            "workspace repo cloned: 'main' @ 0123456789ab",
        ));
        let (watermark, wake) = marker_wake(watermark, 7, &transcript, is_workspace_marker);
        assert_eq!(watermark, Some((7, 4)));
        assert!(wake);
        let (watermark, wake) = marker_wake(watermark, 7, &transcript, is_workspace_marker);
        assert_eq!(watermark, Some((7, 4)));
        assert!(!wake, "an unchanged transcript must not wake again");

        // A room switch bumps the generation: the next room's history is an
        // initial load again, whatever its seqs say.
        let next_room = vec![row(
            9,
            RoomMessageKind::System,
            "system",
            "workspace build 'build' succeeded (3.2s)",
        )];
        let (watermark, wake) = marker_wake(watermark, 8, &next_room, is_workspace_marker);
        assert_eq!(watermark, Some((8, 9)));
        assert!(!wake);
    }

    /// A cleared transcript — a room being opened, hydration pending — stays
    /// uninitialized. Initializing against it would make the hydration that
    /// follows read as news and fire a refresh right after the open fetch.
    #[test]
    fn an_empty_transcript_never_initializes_the_watermark() {
        assert_eq!(
            marker_wake(None, 7, &[], is_workspace_marker),
            (None, false)
        );
        assert_eq!(
            marker_wake(Some((7, 4)), 7, &[], is_workspace_marker),
            (None, false)
        );
    }

    /// Execs ride every fallback tick (no marker exists for them); status
    /// only every Nth, because every status change mints one.
    #[test]
    fn the_status_fallback_rides_every_third_tick() {
        let status_ticks: Vec<u64> = (1..=7)
            .filter(|tick| fallback_reads_status(*tick))
            .collect();
        assert_eq!(status_ticks, vec![3, 6]);
    }
}
