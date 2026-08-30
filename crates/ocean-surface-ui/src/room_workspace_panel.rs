//! The room's workspace: its status, files and command history — the read
//! half of the lane `room_repo.rs` drives — and the owner lifecycle verbs
//! that create and destroy the workspace itself.
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
//! The lifecycle pair rides the same lane since the daemon's allowlist
//! opened the owner leaves (the 2026-08-29 operator ruling):
//!
//!   POST /v1/rooms/persistent/{key}/workspace/provision → create + hydrate
//!   POST /v1/rooms/persistent/{key}/workspace/destroy   → flush, then discard
//!
//! Both are owner verbs: the daemon forwards them only for the actor that
//! resolves to the room credential's own principal and refuses everyone else
//! in type (`workspace_not_owner_principal`), so this panel renders the
//! controls for every member and lets that refusal be the answer — no
//! authorization invented on this side. Destroy always saves first: the
//! container is flushed back to Bedrock before the driver discards it, and
//! the `?flush=0` escape hatch is deliberately never sent from here.
//! A daemon predating the leaves 404s them; the buttons answer "not
//! available yet" in the reads' own voice.
//!
//! The owner's secrets SET rides the same lane, and VALUES are set-only
//! by ruling — the daemon's manifest pins "a secrets row is the
//! owner-gated set, and nothing else"; no route anywhere upstream returns
//! a value:
//!
//!   POST /v1/rooms/persistent/{key}/workspace/secrets/set → upstream PUT
//!
//! so the control below is a submit and its copy says so. The reply is
//! names only (`{set, removed, total}`), and the value's whole life on
//! this side is the input signal and the in-flight request body — cleared
//! on submit, never rendered back, never logged. What a room HOLDS is
//! member-visible, though: since Bedrock's status body grew a `secrets`
//! list beside the projection — name, key fingerprint, updated time,
//! never a value — the panel lists it above the form. A status body
//! WITHOUT the key is an older Bedrock, and renders no claim at all.
//!
//! Members RUN commands on the same lane — the daemon's allowlist exec
//! row is a member act (write-gated, attributed, never owner-gated):
//!
//!   POST /v1/rooms/persistent/{key}/workspace/exec → run, record, flush
//!
//! The body this side sends is `{"command"}` alone, deliberately: `cwd`
//! defaults to the workspace root, the flush default is what saves the
//! work back to Bedrock, and attribution is the daemon's — it resolves
//! the actor at its own gate and strips any client-supplied
//! `actor_member_id` as forgeable. A landed run mints no transcript
//! marker, so the form re-reads the lanes itself instead of waiting on a
//! wake that will never come.
//!
//! The owner's take-back rides the same lane — the documented recovery for
//! output that should never have been published (a token that leaked into a
//! tail before it was stored as a secret, or was rotated after a leak):
//!
//!   POST /v1/rooms/persistent/{key}/workspace/execs/purge → blank stored tails
//!
//! `{}` purges every finished row, `{exec_id}` one; still-running rows are
//! skipped on purge-all (their tails do not exist yet) and refused in type
//! (`exec_running`) when named. Owner-gated like destroy, unattributed like
//! it too. A purged row comes back with `purged: true` and NULL tails, and
//! this panel says "taken back" instead of letting it read as a command
//! that printed nothing.
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

use crate::rooms::{encode, RoomMessage, RoomMessageKind, Rooms};
use crate::rooms_workspace::room_is_federated;

/// The open panel's fallback tick. The marker wake is the primary refresh
/// now, so this only has to be honest where the push path is absent — and
/// it is what keeps another member's plain execs advancing at all, which is
/// why it stays minutes-not-hours slow.
const PANEL_POLL_MS: u32 = 10_000;

/// Every status change emits a transcript marker, so the status lane needs
/// the fallback even less than the execs lane does: it rides every Nth tick
/// (~30s) instead of every one.
const STATUS_FALLBACK_EVERY_TICKS: u64 = 3;

/// Every marker the daemon composes opens with this word — twelve variants
/// plus a catch-all, one prefix (`compose_workspace_marker` in ocean-os's
/// room_federation.rs).
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
    /// The room's stored secret names — not a projection field on the
    /// wire (the status body carries `secrets` BESIDE `workspace`, hence
    /// the skip), folded in by `classify_status` so the Present view is
    /// the whole status answer. `None` is a Bedrock that predates the
    /// list: unknown, never "none stored".
    #[serde(skip)]
    pub secrets: Option<Vec<SecretRow>>,
}

/// One row of the status body's `secrets` list — `listSecretNames`'s
/// projection (src/room-compute.mjs): name, sealing-key fingerprint,
/// updated time. Names and metadata only; no route upstream returns a
/// value, so nothing here can carry one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct SecretRow {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub key_id: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
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
    /// Bedrock projects `purged: true` only on rows the owner took back
    /// (never `false` — unpurged rows must project byte-identically to
    /// before the column existed), so absence deserializes as the fact it
    /// states. On a purged row the tails come back NULL, which without this
    /// flag would read as "still being written" — a lie about a finished
    /// command.
    #[serde(default)]
    pub purged: bool,
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

/// The destroy reply's account of the final save. `changed` counts the
/// files written back to Bedrock; `error` is the flush failing while the
/// destroy still went through — the one outcome that must never read as
/// "saved".
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
struct FlushReport {
    #[serde(default)]
    changed: Option<u64>,
    #[serde(default)]
    error: Option<String>,
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
    /// The exec reply's own row — Bedrock's `exec` projection, relayed by
    /// the daemon. The tails beside it on the wire are not read here: the
    /// refreshed history below is where the output shows.
    #[serde(default)]
    exec: Option<ExecRow>,
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
    /// `true` on the destroy reply — the record is closed, whatever the
    /// flush report beside it says.
    #[serde(default)]
    destroyed: Option<bool>,
    /// The secrets-set reply, names only: no route upstream returns a
    /// value, so nothing here can either.
    #[serde(default)]
    set: Option<Vec<String>>,
    #[serde(default)]
    removed: Option<Vec<String>>,
    #[serde(default)]
    total: Option<u64>,
    /// The status body's stored-secret names, riding beside `workspace`.
    /// Absent on a Bedrock that predates the list — a distinction that
    /// must survive to the render: absent is unknown, `[]` is "none
    /// stored".
    #[serde(default)]
    secrets: Option<Vec<SecretRow>>,
    /// The purge reply's row count — how many execs had their tails
    /// blanked. Presence is what success means on that lane; the `exec_id`
    /// echoed beside it is not read here.
    #[serde(default)]
    purged: Option<u64>,
    /// The destroy reply's flush report: `null` when no ready container
    /// stood to save.
    #[serde(default)]
    flush: Option<FlushReport>,
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

/// The owner lifecycle rides daemon-side POST leaves — the daemon maps
/// destroy to Bedrock's DELETE itself, exactly as `room_repo`'s unbind
/// rides. No `flush=` ever: the default flush is what makes destroy save
/// the work back to Bedrock, and offering the skip would make this button
/// able to discard a room's work.
fn provision_url(base: &str, key: &str, actor: &str) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/workspace/provision?actor_id={}",
        encode(key),
        encode(actor),
    )
}

fn destroy_url(base: &str, key: &str, actor: &str) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/workspace/destroy?actor_id={}",
        encode(key),
        encode(actor),
    )
}

/// The owner's secrets set — the daemon's POST leaf translating Bedrock's
/// PUT, same as destroy's DELETE ride.
fn secrets_set_url(base: &str, key: &str, actor: &str) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/workspace/secrets/set?actor_id={}",
        encode(key),
        encode(actor),
    )
}

/// The member's exec — the daemon's one attributed POST leaf. Identity
/// rides `?actor_id=` like every call on this lane; the body never carries
/// it (see `post_exec`).
fn exec_url(base: &str, key: &str, actor: &str) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/workspace/exec?actor_id={}",
        encode(key),
        encode(actor),
    )
}

/// The owner's exec take-back — owner-gated and unattributed like destroy
/// (the daemon strips any client-sent `actor_member_id`; none is composed
/// here). The body names the target: `{}` for every finished row, an
/// `exec_id` for one.
fn execs_purge_url(base: &str, key: &str, actor: &str) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/workspace/execs/purge?actor_id={}",
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
        // The owner verbs resolve the actor against the identity map, so
        // these are reachable here where the reads never earn them.
        "forged_workspace_actor" => {
            "An agent's workspace command is run by the daemon, not from here."
        }
        "workspace_actor_unmapped" => "Your identity doesn't map to this room's compute service.",
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
    if let Some(mut workspace) = body.workspace {
        // The secret names ride beside the projection on the wire; folded
        // in here so the Present view carries the whole status answer.
        workspace.secrets = body.secrets;
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

/// The owner lifecycle verbs this panel can fire. Two, deliberately: the
/// daemon's leaf pair is the whole vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkspaceCommand {
    Provision,
    Destroy,
}

impl WorkspaceCommand {
    fn noun(self) -> &'static str {
        match self {
            WorkspaceCommand::Provision => "provision",
            WorkspaceCommand::Destroy => "destroy",
        }
    }
}

/// What a lifecycle reply means for the panel. `Landed` carries its own
/// sentence but never a view: the mutation reply's claims stay out of
/// `WorkspaceView` — the caller re-reads status as truth, so the panel
/// only ever renders what any reload would.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LifecycleOutcome {
    /// The verb landed; the sentence says what the reply proved.
    Landed(String),
    /// A typed state in the calm voice: already provisioning, nothing to
    /// destroy, not the owner. Answers, not faults.
    State(String),
    /// A refusal or fault, in words an operator can act on.
    Failure(String),
    /// The daemon in front of us does not serve the lifecycle leaves yet.
    Unavailable,
}

/// The sentence a typed lifecycle state earns. `workspace_absent` can only
/// come back from destroy — provision claims an absent record instead of
/// refusing it — so its sentence states the destroy fact.
fn lifecycle_state_sentence(code: &str) -> Option<String> {
    let sentence = match code {
        "workspace_provisioning" => "A workspace for this room is already being provisioned.",
        "workspace_absent" => "This room has no workspace to destroy.",
        // The daemon's owner gate answering a non-principal actor: how the
        // room is shaped, not a fault — the calm `room_repo` uses for it.
        "workspace_not_owner_principal" => {
            "Only the room owner can provision or destroy the workspace."
        }
        "room_not_federated" => "This room has no Bedrock workspace.",
        _ => return None,
    };
    Some(sentence.to_string())
}

/// The landed-destroy sentence, from the reply's flush report. Truth over
/// comfort: a destroy whose final flush failed discarded whatever changed
/// since the last save, and saying less would lie about lost work.
fn destroy_sentence(flush: Option<&FlushReport>) -> String {
    let Some(report) = flush else {
        // No ready container stood, so there was nothing live to save.
        return "Workspace destroyed.".to_string();
    };
    if report.error.is_some() {
        return "Workspace destroyed, but the final flush failed \u{2014} changes since the \
                last flush were discarded with the container."
            .to_string();
    }
    match report.changed {
        Some(0) => {
            "Workspace destroyed \u{2014} nothing had changed since the last flush.".to_string()
        }
        Some(1) => {
            "Workspace destroyed \u{2014} 1 changed file flushed back to Bedrock first.".to_string()
        }
        Some(changed) => format!(
            "Workspace destroyed \u{2014} {changed} changed files flushed back to Bedrock first."
        ),
        None => "Workspace destroyed \u{2014} flushed back to Bedrock first.".to_string(),
    }
}

/// The stated-as-fact sentence for a daemon that predates the lifecycle
/// leaves — the same voice as every other lane's "not available yet", and
/// production behavior until a daemon carrying them is deployed.
fn lifecycle_unavailable_sentence(command: WorkspaceCommand) -> String {
    match command {
        WorkspaceCommand::Provision => {
            "Provisioning isn't available on this deployment yet.".to_string()
        }
        WorkspaceCommand::Destroy => {
            "Destroying isn't available on this deployment yet.".to_string()
        }
    }
}

/// Map a lifecycle reply. Success is the verb's own proof — provision's
/// projection (the fresh 201 or the idempotent 200), destroy's
/// `destroyed: true` — and everything else classifies like the read lanes:
/// typed states in the calm voice, refusals in words, an empty or coded
/// 404 as the deployment's honest "not yet".
fn classify_lifecycle(
    command: WorkspaceCommand,
    status: u16,
    body: Option<WorkspaceBody>,
) -> LifecycleOutcome {
    let noun = command.noun();
    let Some(body) = body else {
        if status == 404 {
            return LifecycleOutcome::Unavailable;
        }
        return LifecycleOutcome::Failure(format!(
            "The {noun} reply could not be read ({status})."
        ));
    };
    match command {
        WorkspaceCommand::Provision => {
            if body.workspace.is_some() {
                return LifecycleOutcome::Landed("Workspace provisioned.".to_string());
            }
        }
        WorkspaceCommand::Destroy => {
            if body.destroyed == Some(true) {
                return LifecycleOutcome::Landed(destroy_sentence(body.flush.as_ref()));
            }
        }
    }
    match body.refusal_code() {
        Some("workspace_route_not_allowed") => LifecycleOutcome::Unavailable,
        Some(code) => {
            if let Some(sentence) = lifecycle_state_sentence(code) {
                return LifecycleOutcome::State(sentence);
            }
            LifecycleOutcome::Failure(
                failure_sentence(code)
                    .or_else(|| body.error.clone())
                    .unwrap_or_else(|| format!("The {noun} failed ({status}).")),
            )
        }
        None if status == 404 => LifecycleOutcome::Unavailable,
        None => LifecycleOutcome::Failure(
            body.error
                .filter(|error| !error.is_empty())
                .map(|error| format!("The {noun} was refused: {error}"))
                .unwrap_or_else(|| format!("The {noun} failed ({status}).")),
        ),
    }
}

/// The sentence a typed secrets answer earns — every refusal a state, not
/// an error. `secrets_unconfigured` is the live production answer until a
/// human sets `OCEAN_ROOM_SECRET_KEY` on the Bedrock host and redeploys it.
fn secrets_state_sentence(code: &str) -> Option<String> {
    let sentence = match code {
        "secrets_unconfigured" => "This Bedrock host isn't configured for room secrets.",
        "workspace_absent" => "Secrets need a workspace \u{2014} provision one first.",
        "workspace_provisioning" => {
            "The workspace is still provisioning \u{2014} try again once it's ready."
        }
        "workspace_failed" => {
            "The workspace failed to provision \u{2014} provision it again first."
        }
        "workspace_not_owner_principal" => "Only the room owner can set secrets.",
        "room_not_federated" => "This room has no Bedrock workspace.",
        // The daemon's own cap on lane bodies, stated as the bound it is.
        "workspace_request_too_large" => {
            "That secret is larger than the 32 KiB a workspace request can carry."
        }
        _ => return None,
    };
    Some(sentence.to_string())
}

/// The landed sentence, from the reply's names — the only thing the wire
/// carries about what was stored, and all this panel will ever say about it.
fn secrets_sentence(set: &[String], removed: &[String], total: Option<u64>) -> String {
    let action = match (set.is_empty(), removed.is_empty()) {
        (false, false) => format!("Set {}, removed {}", set.join(", "), removed.join(", ")),
        (false, true) => format!("Set {}", set.join(", ")),
        (true, false) => format!("Removed {}", removed.join(", ")),
        (true, true) => "Nothing changed".to_string(),
    };
    match total {
        Some(1) => format!("{action} \u{2014} 1 secret stored."),
        Some(total) => format!("{action} \u{2014} {total} secrets stored."),
        None => format!("{action}."),
    }
}

/// Map a secrets-set reply. Success is the names reply — `set` and
/// `removed` both present — and everything else classifies like the
/// lifecycle verbs it sits beside: typed states in the calm voice,
/// refusals in words, a 404 as the deployment's honest "not yet".
fn classify_secrets(status: u16, body: Option<WorkspaceBody>) -> LifecycleOutcome {
    let Some(body) = body else {
        if status == 404 {
            return LifecycleOutcome::Unavailable;
        }
        return LifecycleOutcome::Failure(format!(
            "The secrets reply could not be read ({status})."
        ));
    };
    if let (Some(set), Some(removed)) = (&body.set, &body.removed) {
        return LifecycleOutcome::Landed(secrets_sentence(set, removed, body.total));
    }
    match body.refusal_code() {
        Some("workspace_route_not_allowed") => LifecycleOutcome::Unavailable,
        Some(code) => {
            if let Some(sentence) = secrets_state_sentence(code) {
                return LifecycleOutcome::State(sentence);
            }
            LifecycleOutcome::Failure(
                failure_sentence(code)
                    .or_else(|| body.error.clone())
                    .unwrap_or_else(|| format!("The secrets set failed ({status}).")),
            )
        }
        None if status == 404 => LifecycleOutcome::Unavailable,
        None => LifecycleOutcome::Failure(
            body.error
                .filter(|error| !error.is_empty())
                .map(|error| format!("The secrets set was refused: {error}"))
                .unwrap_or_else(|| format!("The secrets set failed ({status}).")),
        ),
    }
}

/// Whether the secrets form renders: any room that answered with the lane —
/// a workspace in any status, or an honest absence (submitting then earns
/// the "provision first" state). Never authorization — the daemon's owner
/// gate answers that in type, for every member the same.
fn secrets_form_stands(view: Option<&WorkspaceView>) -> bool {
    matches!(
        view,
        Some(WorkspaceView::Present(_) | WorkspaceView::Absent)
    )
}

/// What the secrets list may claim. `Some` rows are Bedrock's answer —
/// empty honestly renders "none stored" — while `None` renders NO claim:
/// a Bedrock that predates the list (its status body has no `secrets`
/// key), or a view with no status answer to carry one. The absent-vs-empty
/// discipline of `double_option`, one level up.
fn secret_rows(view: Option<&WorkspaceView>) -> Option<&[SecretRow]> {
    match view {
        Some(WorkspaceView::Present(workspace)) => workspace.secrets.as_deref(),
        _ => None,
    }
}

/// The sentence a typed exec answer earns — the workspace that isn't
/// ready to run anything, and the daemon's own body cap. States, not
/// faults: the command survives in the form for the retry each one
/// invites.
fn exec_state_sentence(code: &str) -> Option<String> {
    let sentence = match code {
        "workspace_absent" => "Running a command needs a workspace \u{2014} provision one first.",
        "workspace_provisioning" => {
            "The workspace is still provisioning \u{2014} try again once it's ready."
        }
        "workspace_failed" => {
            "The workspace failed to provision \u{2014} provision it again first."
        }
        "room_not_federated" => "This room has no Bedrock workspace.",
        "workspace_request_too_large" => {
            "That command is larger than the 32 KiB a workspace request can carry."
        }
        _ => return None,
    };
    Some(sentence.to_string())
}

/// The landed sentence, from the reply's own row. The exit is stated as
/// the fact it is — a member's failing command still landed — and the
/// refreshed history below is where its output reads.
fn exec_sentence(row: &ExecRow) -> String {
    match (row.status.as_str(), row.exit_code) {
        ("exited", Some(code)) => format!("Command ran \u{2014} exited {code}."),
        ("timeout", _) => {
            "The command timed out \u{2014} the history keeps what it wrote.".to_string()
        }
        _ => "Command ran.".to_string(),
    }
}

/// Map an exec reply. Success is the reply's own row, whatever its exit —
/// and everything else classifies like the verbs it sits beside: typed
/// states in the calm voice, refusals in words, a 404 as the deployment's
/// honest "not yet".
fn classify_exec(status: u16, body: Option<WorkspaceBody>) -> LifecycleOutcome {
    let Some(body) = body else {
        if status == 404 {
            return LifecycleOutcome::Unavailable;
        }
        return LifecycleOutcome::Failure(format!(
            "The command reply could not be read ({status})."
        ));
    };
    if let Some(row) = &body.exec {
        return LifecycleOutcome::Landed(exec_sentence(row));
    }
    match body.refusal_code() {
        Some("workspace_route_not_allowed") => LifecycleOutcome::Unavailable,
        Some(code) => {
            if let Some(sentence) = exec_state_sentence(code) {
                return LifecycleOutcome::State(sentence);
            }
            LifecycleOutcome::Failure(
                failure_sentence(code)
                    .or_else(|| body.error.clone())
                    .unwrap_or_else(|| format!("The command failed ({status}).")),
            )
        }
        None if status == 404 => LifecycleOutcome::Unavailable,
        None => LifecycleOutcome::Failure(
            body.error
                .filter(|error| !error.is_empty())
                .map(|error| format!("The command was refused: {error}"))
                .unwrap_or_else(|| format!("The command failed ({status}).")),
        ),
    }
}

/// Whether the run-command form renders: the same answers the secrets form
/// stands on — a workspace in any status, or an honest absence (submitting
/// then earns "provision first"). A member act throughout — the daemon's
/// exec row carries no owner gate, so none is invented here.
fn exec_form_stands(view: Option<&WorkspaceView>) -> bool {
    matches!(
        view,
        Some(WorkspaceView::Present(_) | WorkspaceView::Absent)
    )
}

/// What a purge is aimed at: the whole finished history, or one row. The
/// armed confirm carries this too, so a first click on one control can
/// never fire another's.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PurgeTarget {
    All,
    One(String),
}

/// The sentence a typed purge answer earns — states in the calm voice,
/// like the verbs this rides beside. `exec_running` is Bedrock refusing a
/// NAMED running row (its tails do not exist yet); purge-all skips those
/// silently instead.
fn purge_state_sentence(code: &str) -> Option<String> {
    let sentence = match code {
        "exec_running" => {
            "This command is still running \u{2014} its output can be taken back once it finishes."
        }
        "workspace_not_owner_principal" => "Only the room owner can take back command output.",
        "room_not_federated" => "This room has no Bedrock workspace.",
        _ => return None,
    };
    Some(sentence.to_string())
}

/// The landed sentence, from the reply's own count. Zero is an answer, not
/// a fault: a named row already purged matches nothing (purge is
/// idempotent), and a purge-all over a clean history had nothing standing.
fn purge_sentence(target: &PurgeTarget, purged: u64) -> String {
    match (target, purged) {
        (PurgeTarget::One(_), 0) => "That command's output was already taken back.".to_string(),
        (PurgeTarget::One(_), _) => {
            "Output taken back \u{2014} it no longer reads back for anyone.".to_string()
        }
        (PurgeTarget::All, 0) => "No stored output stood to take back.".to_string(),
        (PurgeTarget::All, 1) => "Took back the stored output of 1 command.".to_string(),
        (PurgeTarget::All, n) => format!("Took back the stored output of {n} commands."),
    }
}

/// Map a purge reply. Success is the reply's count, and the coded answers
/// classify like the verbs beside it. The uncoded 404 needs the target to
/// read honestly: on purge-all it can only be a deployment without the
/// route (there is no other 404 on that path), but a NAMED purge also
/// earns one for a vanished exec — Bedrock's 'No such exec in this room.'
/// — and the two are only told apart by the relayed sentence, so it is
/// relayed rather than guessed at, and the caller refreshes the history
/// either way.
fn classify_exec_purge(
    target: &PurgeTarget,
    status: u16,
    body: Option<WorkspaceBody>,
) -> LifecycleOutcome {
    let Some(body) = body else {
        if status == 404 {
            return LifecycleOutcome::Unavailable;
        }
        return LifecycleOutcome::Failure(format!("The purge reply could not be read ({status})."));
    };
    if let Some(purged) = body.purged {
        return LifecycleOutcome::Landed(purge_sentence(target, purged));
    }
    match body.refusal_code() {
        Some("workspace_route_not_allowed") => LifecycleOutcome::Unavailable,
        Some(code) => {
            if let Some(sentence) = purge_state_sentence(code) {
                return LifecycleOutcome::State(sentence);
            }
            LifecycleOutcome::Failure(
                failure_sentence(code)
                    .or_else(|| body.error.clone())
                    .unwrap_or_else(|| format!("The purge failed ({status}).")),
            )
        }
        None if status == 404 => match target {
            PurgeTarget::All => LifecycleOutcome::Unavailable,
            PurgeTarget::One(_) => LifecycleOutcome::State(
                body.error
                    .filter(|error| !error.is_empty())
                    .map(|error| format!("The purge didn't land: {error}"))
                    .unwrap_or_else(|| "The purge didn't land (404).".to_string()),
            ),
        },
        None => LifecycleOutcome::Failure(
            body.error
                .filter(|error| !error.is_empty())
                .map(|error| format!("The purge was refused: {error}"))
                .unwrap_or_else(|| format!("The purge failed ({status}).")),
        ),
    }
}

/// Which lifecycle verbs the panel offers against what stands. Not
/// authorization — the daemon's owner gate answers that in type — just the
/// verbs that can possibly land: provision claims an absent, destroyed or
/// failed record; destroy needs one still standing. A failed workspace
/// earns both — retry the provision, or clear the record for good.
fn lifecycle_verbs(view: Option<&WorkspaceView>) -> (bool, bool) {
    match view {
        Some(WorkspaceView::Absent) => (true, false),
        Some(WorkspaceView::Present(workspace)) => match workspace.status.as_str() {
            "destroyed" => (true, false),
            "failed" => (true, true),
            _ => (false, true),
        },
        _ => (false, false),
    }
}

/// Whether a fresh status answer changes what the operator is looking at.
/// This is what disarms a primed destroy confirm — the polish debt the
/// repo panel's unbind recorded: armed against one workspace state, the
/// confirm must not stand over another. Timestamps churn on every silent
/// refresh (an exec bumps `last_active_at`), so only the view's shape and
/// status count as a flip.
fn view_flip(old: Option<&WorkspaceView>, new: &WorkspaceView) -> bool {
    match (old, new) {
        (Some(WorkspaceView::Present(old)), WorkspaceView::Present(new)) => {
            old.status != new.status
        }
        (Some(old), new) => std::mem::discriminant(old) != std::mem::discriminant(new),
        (None, _) => true,
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
    /// The owner took this row's output back: the stored tails are gone for
    /// everyone, whoever asks. Checked before the tail fields — a purged
    /// row's NULL tails would otherwise read as "still being written", and
    /// its absent ones as merely withheld.
    Purged,
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
    if row.purged {
        return RowTails::Purged;
    }
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

/// The lanes this panel runs — four reads, and the lifecycle commands. A
/// standing error remembers which lane set it, so one lane's recovery can
/// never wipe another's live failure — and no read publishes as
/// `Lifecycle`, so a failed provision or destroy stands until the next
/// command starts or the room changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanelLane {
    Status,
    Execs,
    Files,
    File,
    Lifecycle,
    Secrets,
    Exec,
    ExecPurge,
}

/// Whether a lane's successful read clears the standing error: only the one
/// its own lane set. This is the absorbed fix — a blipped silent poll used
/// to leave its error standing forever over a self-healed view, and the
/// naive "success clears" would let a status success wipe a live execs
/// failure.
fn lane_success_clears(standing: Option<PanelLane>, lane: PanelLane) -> bool {
    standing == Some(lane)
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
    error: RwSignal<Option<(PanelLane, String)>>,
    /// The typed state or landed outcome worth a sentence — "already being
    /// provisioned", "workspace destroyed". Answers, not faults, in a
    /// calmer voice than `error`.
    note: RwSignal<Option<String>>,
    /// The lifecycle command in flight, if any — blocks re-submit and
    /// drives the button labels while the daemon's command budget runs.
    working: RwSignal<Option<WorkspaceCommand>>,
    /// Whether the destroy control is one click from firing. Destroy
    /// discards the container (after its flush), so the first click only
    /// arms this — and a close, reset or view flip disarms it.
    confirm_destroy: RwSignal<bool>,
    /// The secret NAME being composed. Not sensitive — it is what the reply
    /// echoes back — and it survives a failed submit for a retry.
    secret_name: RwSignal<String>,
    /// The secret VALUE being composed: the one signal in this panel that
    /// must never be rendered anywhere but its own password input. Taken
    /// out (cleared) the moment a submit fires, and cleared again on close
    /// and reset so no pasted value outlives the form it was pasted into.
    secret_value: RwSignal<String>,
    /// The secrets lane's calm sentence — the landed names, or the typed
    /// state that answered instead. Its own slot, not `note`: a provision's
    /// sentence and a secret's must not overwrite each other.
    secrets_note: RwSignal<Option<String>>,
    /// A secrets set is in flight — blocks re-submit while Bedrock writes.
    secrets_busy: RwSignal<bool>,
    /// The command being composed. Not sensitive — it becomes a readable
    /// history row the moment it lands — and it survives every outcome but
    /// a landed run, so a refusal is a retry, not a retype.
    exec_command: RwSignal<String>,
    /// The run lane's calm sentence — the landed exit, or the typed state
    /// that answered instead. Its own slot, for the same reason the
    /// secrets note has one.
    exec_note: RwSignal<Option<String>>,
    /// An exec is in flight — blocks re-submit for the whole command
    /// budget, which runs to 960s upstream.
    exec_busy: RwSignal<bool>,
    /// The purge control one click from firing, and what it is aimed at.
    /// Un-publishing output is destructive the same way destroy is, so it
    /// arms first — and a close, reset or view flip disarms it.
    confirm_purge: RwSignal<Option<PurgeTarget>>,
    /// The purge lane's calm sentence — the landed count, or the typed
    /// state that answered instead. Its own slot, like the notes beside it.
    purge_note: RwSignal<Option<String>>,
    /// A purge is in flight — blocks every purge control while the table
    /// write runs.
    purge_busy: RwSignal<bool>,
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
            note: RwSignal::new(None),
            working: RwSignal::new(None),
            confirm_destroy: RwSignal::new(false),
            secret_name: RwSignal::new(String::new()),
            secret_value: RwSignal::new(String::new()),
            secrets_note: RwSignal::new(None),
            secrets_busy: RwSignal::new(false),
            exec_command: RwSignal::new(String::new()),
            exec_note: RwSignal::new(None),
            exec_busy: RwSignal::new(false),
            confirm_purge: RwSignal::new(None),
            purge_note: RwSignal::new(None),
            purge_busy: RwSignal::new(false),
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

    /// Close the panel, retire its poll loop, and hand focus back. A
    /// reopened panel must not resume a primed destroy confirm — and must
    /// not still hold a pasted secret value either.
    pub fn close_panel(&self) {
        self.panel.set(false);
        self.confirm_destroy.set(false);
        self.confirm_purge.set(None);
        self.secret_value.set(String::new());
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
        self.note.set(None);
        self.working.set(None);
        self.confirm_destroy.set(false);
        self.secret_name.set(String::new());
        self.secret_value.set(String::new());
        self.secrets_note.set(None);
        self.secrets_busy.set(false);
        self.exec_command.set(String::new());
        self.exec_note.set(None);
        self.exec_busy.set(false);
        self.confirm_purge.set(None);
        self.purge_note.set(None);
        self.purge_busy.set(false);
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
                // A view that flipped under a primed confirm takes the
                // confirm with it — armed against one state, it must not
                // fire at another. The purge confirm rides the same rule:
                // the history it aims at belongs to the workspace whose
                // status just changed shape.
                if self
                    .view
                    .with_untracked(|old| view_flip(old.as_ref(), &view))
                {
                    self.confirm_destroy.set(false);
                    self.confirm_purge.set(None);
                }
                self.view.set(Some(view));
                self.clear_lane_error(PanelLane::Status);
            }
            // A failed refresh never blanks a standing view: what the
            // operator was reading is still the best answer this surface has.
            Err(error) => self.error.set(Some((PanelLane::Status, error))),
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
                self.clear_lane_error(PanelLane::Execs);
            }
            Err(error) => self.error.set(Some((PanelLane::Execs, error))),
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
                self.clear_lane_error(PanelLane::Files);
            }
            Err(error) => self.error.set(Some((PanelLane::Files, error))),
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
                self.clear_lane_error(PanelLane::File);
            }
            Err(error) => self.error.set(Some((PanelLane::File, error))),
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
        self.clear_lane_error(PanelLane::File);
    }

    /// A lane that answered clears the error IT set, and only that one — a
    /// healthy status read must not wipe a live execs failure, or the other
    /// way round.
    fn clear_lane_error(&self, lane: PanelLane) {
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
        self.note.set(None);
        self.secrets_note.set(None);
        self.purge_note.set(None);
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

    /// Fire a lifecycle verb. The reply never moves the view — after a
    /// landed command the lanes are re-read as truth, so the panel renders
    /// exactly what any reload would (and the daemon's transcript marker
    /// wakes every other member's panel the same way).
    fn run_lifecycle(&self, rooms: Rooms, command: WorkspaceCommand, key: String, actor: String) {
        let base = self.base();
        let me = *self;
        let generation = rooms.generation_snapshot();
        self.working.set(Some(command));
        self.confirm_destroy.set(false);
        self.error.set(None);
        self.note.set(None);
        spawn_local(async move {
            let url = match command {
                WorkspaceCommand::Provision => provision_url(&base, &key, &actor),
                WorkspaceCommand::Destroy => destroy_url(&base, &key, &actor),
            };
            let outcome = post_lifecycle(command, &url).await;
            let current = rooms.room_is_current(generation, &key);
            let landed = matches!(outcome, LifecycleOutcome::Landed(_));
            me.publish_lifecycle(command, outcome, current);
            if current && landed {
                let open = me.panel.get_untracked();
                refresh_lanes(me, &base, &key, &actor, true, open, open).await;
            }
        });
    }

    /// Publish a completed lifecycle command — but only into the room that
    /// started it. Landed and typed states share the note's calm voice;
    /// only a real refusal or fault takes the alert.
    fn publish_lifecycle(
        &self,
        command: WorkspaceCommand,
        outcome: LifecycleOutcome,
        room_is_current: bool,
    ) {
        if !room_is_current {
            return;
        }
        self.working.set(None);
        match outcome {
            LifecycleOutcome::Landed(sentence) | LifecycleOutcome::State(sentence) => {
                self.note.set(Some(sentence));
            }
            LifecycleOutcome::Unavailable => {
                self.note.set(Some(lifecycle_unavailable_sentence(command)));
            }
            LifecycleOutcome::Failure(error) => {
                self.error.set(Some((PanelLane::Lifecycle, error)));
            }
        }
    }

    /// Take the composed secret out of the form: the trimmed name, and the
    /// value — which leaves its signal HERE, before any request exists, so
    /// after a submit the value's only life is the in-flight body. Both
    /// trimmed: a pasted token's trailing newline would otherwise be stored
    /// into every future exec's environment, failing auth invisibly.
    /// `None` (nothing composed) takes nothing, so a stray click cannot
    /// wipe a half-pasted form.
    fn take_secret_submission(&self) -> Option<(String, String)> {
        let name = self.secret_name.get_untracked().trim().to_string();
        let value = self.secret_value.get_untracked().trim().to_string();
        if name.is_empty() || value.is_empty() {
            return None;
        }
        self.secret_value.set(String::new());
        Some((name, value))
    }

    /// Fire the owner's secrets set. Same shape as `run_exec`, active
    /// re-read included: the status body carries the room's secret names
    /// now, so the list above the form is exactly what a landed set
    /// changed. No marker wake follows — the daemon's allowlist files
    /// `secrets_updated` under configuration bookkeeping — so other
    /// members converge on the ~30s status fallback tick; this read is
    /// the actor's own list amending immediately instead of waiting for
    /// one. The generation guard admits the note as before — the ticket
    /// inside `refresh_lanes` guards the status read itself.
    fn run_secrets_set(&self, rooms: Rooms, key: String, actor: String) {
        let Some((name, value)) = self.take_secret_submission() else {
            return;
        };
        let base = self.base();
        let me = *self;
        let generation = rooms.generation_snapshot();
        self.secrets_busy.set(true);
        self.secrets_note.set(None);
        self.error.set(None);
        spawn_local(async move {
            let url = secrets_set_url(&base, &key, &actor);
            let outcome = post_secrets_set(&url, &name, &value).await;
            let current = rooms.room_is_current(generation, &key);
            let landed = matches!(outcome, LifecycleOutcome::Landed(_));
            me.publish_secrets(outcome, current);
            if current && landed {
                refresh_lanes(me, &base, &key, &actor, true, false, false).await;
            }
        });
    }

    /// Publish a completed secrets set — but only into the room that
    /// started it. Landed clears the name too (the form is spent); a typed
    /// state keeps it, because "provision first" is an answer the owner
    /// acts on and then retries.
    fn publish_secrets(&self, outcome: LifecycleOutcome, room_is_current: bool) {
        if !room_is_current {
            return;
        }
        self.secrets_busy.set(false);
        match outcome {
            LifecycleOutcome::Landed(sentence) => {
                self.secret_name.set(String::new());
                self.secrets_note.set(Some(sentence));
            }
            LifecycleOutcome::State(sentence) => {
                self.secrets_note.set(Some(sentence));
            }
            LifecycleOutcome::Unavailable => {
                self.secrets_note.set(Some(
                    "Setting secrets isn't available on this deployment yet.".to_string(),
                ));
            }
            LifecycleOutcome::Failure(error) => {
                self.error.set(Some((PanelLane::Secrets, error)));
            }
        }
    }

    /// Take the composed command: trimmed, an empty compose takes nothing.
    /// Unlike the secret value it stays in its signal — only a landed run
    /// spends the form, so every refusal keeps the command for its retry.
    fn take_exec_submission(&self) -> Option<String> {
        let command = self.exec_command.get_untracked().trim().to_string();
        if command.is_empty() {
            return None;
        }
        Some(command)
    }

    /// Fire a member's command. Same shape as `run_lifecycle`, including
    /// the active re-read after a landed run — necessarily: a plain exec
    /// mints no transcript marker (the daemon keeps exec chatter off
    /// transcripts), so no wake will fire and this refresh is how the new
    /// row appears in the history below.
    fn run_exec(&self, rooms: Rooms, key: String, actor: String) {
        let Some(command) = self.take_exec_submission() else {
            return;
        };
        let base = self.base();
        let me = *self;
        let generation = rooms.generation_snapshot();
        self.exec_busy.set(true);
        self.exec_note.set(None);
        self.error.set(None);
        spawn_local(async move {
            let url = exec_url(&base, &key, &actor);
            let outcome = post_exec(&url, &command).await;
            // The command budget runs to 960s — long past any panel switch
            // — so only the room that started it may hear the answer.
            let current = rooms.room_is_current(generation, &key);
            let landed = matches!(outcome, LifecycleOutcome::Landed(_));
            me.publish_exec(outcome, current);
            if current && landed {
                let open = me.panel.get_untracked();
                refresh_lanes(me, &base, &key, &actor, true, open, open).await;
            }
        });
    }

    /// Publish a completed command — but only into the room that started
    /// it. Landed spends the form; a typed state keeps the command for the
    /// retry it invites, exactly like the secrets publish beside it.
    fn publish_exec(&self, outcome: LifecycleOutcome, room_is_current: bool) {
        if !room_is_current {
            return;
        }
        self.exec_busy.set(false);
        match outcome {
            LifecycleOutcome::Landed(sentence) => {
                self.exec_command.set(String::new());
                self.exec_note.set(Some(sentence));
            }
            LifecycleOutcome::State(sentence) => {
                self.exec_note.set(Some(sentence));
            }
            LifecycleOutcome::Unavailable => {
                self.exec_note.set(Some(
                    "Running commands isn't available on this deployment yet.".to_string(),
                ));
            }
            LifecycleOutcome::Failure(error) => {
                self.error.set(Some((PanelLane::Exec, error)));
            }
        }
    }

    /// Fire the owner's take-back. Same shape as `run_exec` — but unlike
    /// the secrets set, the history below IS what a landed purge changed,
    /// so it is re-read as truth. A typed state re-reads it too: the
    /// vanished-exec answer means the list on screen is already stale.
    fn run_exec_purge(&self, rooms: Rooms, key: String, actor: String, target: PurgeTarget) {
        let base = self.base();
        let me = *self;
        let generation = rooms.generation_snapshot();
        self.purge_busy.set(true);
        self.confirm_purge.set(None);
        self.purge_note.set(None);
        self.error.set(None);
        spawn_local(async move {
            let url = execs_purge_url(&base, &key, &actor);
            let outcome = post_exec_purge(&url, &target).await;
            let current = rooms.room_is_current(generation, &key);
            let refresh = matches!(
                outcome,
                LifecycleOutcome::Landed(_) | LifecycleOutcome::State(_)
            );
            me.publish_exec_purge(outcome, current);
            if current && refresh {
                refresh_lanes(me, &base, &key, &actor, false, true, false).await;
            }
        });
    }

    /// Publish a completed purge — but only into the room that started it.
    fn publish_exec_purge(&self, outcome: LifecycleOutcome, room_is_current: bool) {
        if !room_is_current {
            return;
        }
        self.purge_busy.set(false);
        match outcome {
            LifecycleOutcome::Landed(sentence) | LifecycleOutcome::State(sentence) => {
                self.purge_note.set(Some(sentence));
            }
            LifecycleOutcome::Unavailable => {
                self.purge_note.set(Some(
                    "Taking back output isn't available on this deployment yet.".to_string(),
                ));
            }
            LifecycleOutcome::Failure(error) => {
                self.error.set(Some((PanelLane::ExecPurge, error)));
            }
        }
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

/// One lifecycle POST. `{}` because the daemon's lane demands a JSON object
/// on every POST leaf even where the upstream DELETE reads none — the same
/// contract as `room_repo`'s unbind — and provision's strict deny-extra
/// body admits `spec` alone, which this panel never shapes: the daemon's
/// default spec is the product's.
async fn post_lifecycle(command: WorkspaceCommand, url: &str) -> LifecycleOutcome {
    match Request::post(url)
        .header("content-type", "application/json")
        .json(&serde_json::json!({}))
    {
        Ok(request) => match request.send().await {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.json::<WorkspaceBody>().await.ok();
                classify_lifecycle(command, status, body)
            }
            // The container work may well continue upstream — Bedrock owns
            // the record either way — so the sentence says so instead of
            // implying the command died with the connection.
            Err(err) => LifecycleOutcome::Failure(format!(
                "The request was cut ({err}) \u{2014} the {} may still be running upstream.",
                command.noun()
            )),
        },
        Err(err) => LifecycleOutcome::Failure(format!("Workspace request encode error: {err}")),
    }
}

/// The secrets POST — the one lane call with a real body: exactly
/// `{"secrets": {NAME: value}}`, which Bedrock validates strict deny-extra
/// (null would remove, and this form deliberately never composes one). The
/// map is built by hand because the value is a variable, and this function
/// is the last place it exists.
async fn post_secrets_set(url: &str, name: &str, value: &str) -> LifecycleOutcome {
    let mut secrets = serde_json::Map::new();
    secrets.insert(
        name.to_string(),
        serde_json::Value::String(value.to_string()),
    );
    match Request::post(url)
        .header("content-type", "application/json")
        .json(&serde_json::json!({ "secrets": secrets }))
    {
        Ok(request) => match request.send().await {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.json::<WorkspaceBody>().await.ok();
                classify_secrets(status, body)
            }
            // Unlike the lifecycle verbs there is no readback to learn the
            // truth from, so the sentence says what to do about it.
            Err(err) => LifecycleOutcome::Failure(format!(
                "The request was cut ({err}) \u{2014} the secret may or may not be stored; \
                 set it again to be sure."
            )),
        },
        Err(err) => LifecycleOutcome::Failure(format!("Workspace request encode error: {err}")),
    }
}

/// The exec POST — `{"command"}` alone, deliberately: `cwd` defaults to
/// the workspace root upstream, the flush default is what saves the work
/// back to Bedrock, and the daemon inserts the gate-resolved actor itself
/// (a client-sent `actor_member_id` would be stripped as forgeable).
async fn post_exec(url: &str, command: &str) -> LifecycleOutcome {
    match Request::post(url)
        .header("content-type", "application/json")
        .json(&serde_json::json!({ "command": command }))
    {
        Ok(request) => match request.send().await {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.json::<WorkspaceBody>().await.ok();
                classify_exec(status, body)
            }
            // The container run may well continue upstream; the history is
            // where its truth lands either way, so the sentence points there.
            Err(err) => LifecycleOutcome::Failure(format!(
                "The request was cut ({err}) \u{2014} the command may still be \
                 running; the history below will say."
            )),
        },
        Err(err) => LifecycleOutcome::Failure(format!("Workspace request encode error: {err}")),
    }
}

/// The purge POST. `{}` means every finished row; a named row rides as
/// `{"exec_id"}` — validated upstream strict deny-extra, and nothing else
/// is ever composed here (the daemon would strip an `actor_member_id` as
/// forgeable anyway).
async fn post_exec_purge(url: &str, target: &PurgeTarget) -> LifecycleOutcome {
    let body = match target {
        PurgeTarget::All => serde_json::json!({}),
        PurgeTarget::One(exec_id) => serde_json::json!({ "exec_id": exec_id }),
    };
    match Request::post(url)
        .header("content-type", "application/json")
        .json(&body)
    {
        Ok(request) => match request.send().await {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.json::<WorkspaceBody>().await.ok();
                classify_exec_purge(target, status, body)
            }
            // A prompt table write — but the cut connection still hides
            // whether it landed, and the refreshed history is what says.
            Err(err) => LifecycleOutcome::Failure(format!(
                "The request was cut ({err}) \u{2014} the purge may or may not have \
                 landed; the history below will say."
            )),
        },
        Err(err) => LifecycleOutcome::Failure(format!("Workspace request encode error: {err}")),
    }
}

// ---- Component --------------------------------------------------------------

/// The open room's workspace: a compact rail row with a glance line, and a
/// panel where the provision facts, the file tree and the command history
/// are read.
///
/// Renders NOTHING for a Local room — no workspace exists there and a
/// refusal would only read as breakage. No `writes_allowed` gate even now
/// that the lifecycle verbs live here: they are owner acts the daemon gates
/// in type, strictly narrower than any gate this side could compose, and
/// what a member may read the daemon already decided per row.
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
                PanelLane::Status,
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
                                            <div class="rooms-workspace__compute-note">
                                                "This room has no workspace yet."
                                            </div>
                                        }.into_any(),
                                        _ => ().into_any(),
                                    }}
                                    {move || lifecycle_section(state, rooms, actor)}
                                    {move || secrets_section(state, rooms, actor)}
                                    {move || files_section(state, actor)}
                                    {move || exec_section(state, rooms, actor)}
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
                                            {move || {
                                                state.purge_note.get().map(|note| view! {
                                                    <div class="rooms-workspace__compute-note">
                                                        {note}
                                                    </div>
                                                })
                                            }}
                                            {(!rows.is_empty())
                                                .then(|| purge_all_control(state, rooms, actor))}
                                            {if rows.is_empty() {
                                                view! {
                                                    <div class="rooms-workspace__compute-note">
                                                        "No commands have run in this \
                                                         workspace yet."
                                                    </div>
                                                }.into_any()
                                            } else {
                                                rows.iter()
                                                    .map(|row| {
                                                        exec_row_view(state, rooms, actor, row)
                                                    })
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

/// The whole-history take-back, under the section title. Rendered for
/// every member the way destroy is — the daemon's owner gate answers
/// authorization in type — and armed before it fires, because it
/// un-publishes output for everyone at once.
fn purge_all_control(
    state: RoomWorkspacePanelState,
    rooms: Rooms,
    // `Send` on top of the sections' shared bound: the armed/disarmed pair
    // is a reactive child closure, not just an event handler.
    actor: impl Fn() -> Option<(String, String)> + Copy + Send + 'static,
) -> impl IntoView {
    view! {
        <div class="rooms-workspace__compute-actions rooms-workspace__compute-actions--purge">
            {move || {
                let busy = state.purge_busy.get();
                if state.confirm_purge.get() == Some(PurgeTarget::All) {
                    view! {
                        <span class="rooms-workspace__compute-destroy-warn">
                            "Blanks every finished command's stored output, for everyone \
                             \u{2014} still-running commands keep theirs."
                        </span>
                        <button
                            class="rooms-workspace__compute-run \
                                   rooms-workspace__compute-run--danger"
                            type="button"
                            disabled=busy
                            on:click=move |_| {
                                let Some((key, actor_id)) = actor() else { return };
                                state.run_exec_purge(rooms, key, actor_id, PurgeTarget::All);
                            }
                        >
                            "take back all"
                        </button>
                        <button
                            class="rooms-workspace__compute-run"
                            type="button"
                            on:click=move |_| state.confirm_purge.set(None)
                        >
                            "keep"
                        </button>
                    }.into_any()
                } else {
                    view! {
                        <button
                            class="rooms-workspace__compute-run \
                                   rooms-workspace__compute-run--danger"
                            type="button"
                            title="Take back the stored output of every finished command"
                            disabled=busy
                            on:click=move |_| {
                                state.confirm_purge.set(Some(PurgeTarget::All));
                            }
                        >
                            {if busy { "taking back\u{2026}" } else { "take back output\u{2026}" }}
                        </button>
                    }.into_any()
                }
            }}
        </div>
    }
}

/// One command in the history: verdict, headline, outcome, its output — or
/// the sentence explaining why the output is not here — and the owner's
/// per-row take-back, behind the same armed confirm the whole-history
/// control carries. The armed target names this row's id, so two rows'
/// confirms can never cross.
fn exec_row_view(
    state: RoomWorkspacePanelState,
    rooms: Rooms,
    // `Send` for the same reason `purge_all_control` carries it.
    actor: impl Fn() -> Option<(String, String)> + Copy + Send + 'static,
    row: &ExecRow,
) -> impl IntoView {
    let build = is_build(&row.command);
    let mark = exec_mark(row);
    let headline = command_headline(&row.command);
    let full = row.command.clone();
    let status_line = exec_status_line(row);
    let tails = row_tails(row);
    // A purged row has nothing left to take back (a second purge matches
    // nothing), so it carries no control; a running row keeps its control
    // and earns the typed `exec_running` answer.
    let purgeable = !row.purged && !row.id.is_empty();
    let row_id = row.id.clone();
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
                RowTails::Purged => view! {
                    <div class="rooms-workspace__compute-tail-note">
                        "Output taken back by the room owner."
                    </div>
                }.into_any(),
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
            {purgeable.then(|| view! {
                <div class="rooms-workspace__compute-exec-actions">
                    {move || {
                        let busy = state.purge_busy.get();
                        let armed = state
                            .confirm_purge
                            .get()
                            .is_some_and(|target| target == PurgeTarget::One(row_id.clone()));
                        if armed {
                            let fire = row_id.clone();
                            view! {
                                <span class="rooms-workspace__compute-destroy-warn">
                                    "Blanks this command's stored output, for everyone."
                                </span>
                                <button
                                    class="rooms-workspace__compute-run \
                                           rooms-workspace__compute-run--danger"
                                    type="button"
                                    disabled=busy
                                    on:click=move |_| {
                                        let Some((key, actor_id)) = actor() else { return };
                                        state.run_exec_purge(
                                            rooms,
                                            key,
                                            actor_id,
                                            PurgeTarget::One(fire.clone()),
                                        );
                                    }
                                >
                                    "take back"
                                </button>
                                <button
                                    class="rooms-workspace__compute-run"
                                    type="button"
                                    on:click=move |_| state.confirm_purge.set(None)
                                >
                                    "keep"
                                </button>
                            }.into_any()
                        } else {
                            let arm = row_id.clone();
                            view! {
                                <button
                                    class="rooms-workspace__compute-exec-purge"
                                    type="button"
                                    title="Take back this command's stored output"
                                    disabled=busy
                                    on:click=move |_| {
                                        state.confirm_purge.set(Some(PurgeTarget::One(
                                            arm.clone(),
                                        )));
                                    }
                                >
                                    "take back\u{2026}"
                                </button>
                            }.into_any()
                        }
                    }}
                </div>
            })}
        </div>
    }
}

/// The owner lifecycle controls, under the facts (or the absence they act
/// on). Rendered for every member — the daemon's typed refusal is the
/// authorization answer — and destroy sits behind a two-click confirm
/// whose copy says what actually happens: the container is flushed back
/// to Bedrock, then discarded. Both verbs run real container work under
/// the daemon's command budget, so the in-flight command disables the row.
fn lifecycle_section(
    state: RoomWorkspacePanelState,
    rooms: Rooms,
    actor: impl Fn() -> Option<(String, String)> + Copy + 'static,
) -> AnyView {
    let (provision, destroy) = lifecycle_verbs(state.view.get().as_ref());
    let note = state.note.get();
    if !provision && !destroy && note.is_none() {
        return ().into_any();
    }
    let working = state.working.get();
    let busy = working.is_some();
    view! {
        {note.map(|note| view! {
            <div class="rooms-workspace__compute-note">{note}</div>
        })}
        {(provision || destroy).then(|| view! {
            <div class="rooms-workspace__compute-actions">
                {provision.then(|| view! {
                    <button
                        class="rooms-workspace__compute-run"
                        type="button"
                        title="Provision a container workspace for this room"
                        disabled=busy
                        on:click=move |_| {
                            let Some((key, actor_id)) = actor() else { return };
                            state.run_lifecycle(
                                rooms,
                                WorkspaceCommand::Provision,
                                key,
                                actor_id,
                            );
                        }
                    >
                        {if working == Some(WorkspaceCommand::Provision) {
                            "provisioning\u{2026}"
                        } else {
                            "provision"
                        }}
                    </button>
                })}
                {destroy.then(|| {
                    if state.confirm_destroy.get() {
                        view! {
                            <span class="rooms-workspace__compute-destroy-warn">
                                "The container is flushed back to Bedrock, then discarded."
                            </span>
                            <button
                                class="rooms-workspace__compute-run rooms-workspace__compute-run--danger"
                                type="button"
                                disabled=busy
                                on:click=move |_| {
                                    let Some((key, actor_id)) = actor() else { return };
                                    state.run_lifecycle(
                                        rooms,
                                        WorkspaceCommand::Destroy,
                                        key,
                                        actor_id,
                                    );
                                }
                            >
                                "destroy"
                            </button>
                            <button
                                class="rooms-workspace__compute-run"
                                type="button"
                                on:click=move |_| state.confirm_destroy.set(false)
                            >
                                "keep"
                            </button>
                        }.into_any()
                    } else {
                        view! {
                            <button
                                class="rooms-workspace__compute-run rooms-workspace__compute-run--danger"
                                type="button"
                                title="Destroy this room's workspace \u{2014} it is flushed \
                                       back to Bedrock first"
                                disabled=busy
                                on:click=move |_| state.confirm_destroy.set(true)
                            >
                                {if working == Some(WorkspaceCommand::Destroy) {
                                    "destroying\u{2026}"
                                } else {
                                    "destroy\u{2026}"
                                }}
                            </button>
                        }.into_any()
                    }
                })}
            </div>
        })}
    }
    .into_any()
}

/// The owner's secrets set, under the lifecycle verbs — and above it,
/// what the room already holds: the status body's name list, when the
/// Bedrock in front of us serves one. Values stay set-only — the list is
/// names and metadata, there is no value on the wire to leak — and the
/// copy says so up front. Rendered for every member; the daemon's typed
/// 403 is the authorization answer, same as the verbs above.
fn secrets_section(
    state: RoomWorkspacePanelState,
    rooms: Rooms,
    actor: impl Fn() -> Option<(String, String)> + Copy + 'static,
) -> AnyView {
    let view = state.view.get();
    if !secrets_form_stands(view.as_ref()) {
        return ().into_any();
    }
    let rows = secret_rows(view.as_ref()).map(<[SecretRow]>::to_vec);
    let busy = state.secrets_busy.get();
    view! {
        <div class="rooms-workspace__compute-secrets-title">"Secrets"</div>
        <div class="rooms-workspace__compute-secrets-copy">
            "Values are set-only: injected into workspace commands, never shown again."
        </div>
        {rows.map(|rows| {
            if rows.is_empty() {
                return view! {
                    <div class="rooms-workspace__compute-note">
                        "No secrets stored for this room."
                    </div>
                }
                .into_any();
            }
            view! {
                <ul class="rooms-workspace__compute-secrets-list">
                    {rows
                        .into_iter()
                        .map(|row| view! {
                            <li class="rooms-workspace__compute-secrets-row">
                                <span
                                    class="rooms-workspace__compute-secrets-row-name"
                                    title=row
                                        .key_id
                                        .map(|key_id| format!("sealed under key {key_id}"))
                                >
                                    {row.name}
                                </span>
                                {row.updated_at.map(|at| view! {
                                    <span class="rooms-workspace__compute-secrets-row-at">
                                        {format!("updated {at}")}
                                    </span>
                                })}
                            </li>
                        })
                        .collect::<Vec<_>>()}
                </ul>
            }
            .into_any()
        })}
        {state.secrets_note.get().map(|note| view! {
            <div class="rooms-workspace__compute-note">{note}</div>
        })}
        <div class="rooms-workspace__compute-secrets-form">
            <input
                class="rooms-workspace__compute-secret-name"
                type="text"
                placeholder="GH_TOKEN"
                aria-label="Secret name"
                autocomplete="off"
                spellcheck="false"
                disabled=busy
                prop:value=move || state.secret_name.get()
                on:input=move |ev| state.secret_name.set(event_target_value(&ev))
            />
            // A password input: what is pasted never sits readable on
            // screen, and the signal behind it is cleared on submit.
            <input
                class="rooms-workspace__compute-secret-value"
                type="password"
                placeholder="value"
                aria-label="Secret value"
                autocomplete="off"
                disabled=busy
                prop:value=move || state.secret_value.get()
                on:input=move |ev| state.secret_value.set(event_target_value(&ev))
            />
            <button
                class="rooms-workspace__compute-run"
                type="button"
                title="Store this secret for the room's workspace commands"
                disabled=move || {
                    state.secrets_busy.get()
                        || state.secret_name.with(|name| name.trim().is_empty())
                        || state.secret_value.with(|value| value.trim().is_empty())
                }
                on:click=move |_| {
                    let Some((key, actor_id)) = actor() else { return };
                    state.run_secrets_set(rooms, key, actor_id);
                }
            >
                {if busy { "setting\u{2026}" } else { "set secret" }}
            </button>
        </div>
    }
    .into_any()
}

/// The member's run-command form, above the history its result lands in.
/// One input and a submit: the minimal body is the correct one, and the
/// row the run mints below carries everything else. Rendered for every
/// member — exec is a member act on the daemon's allowlist, so no owner
/// gate wraps it the way none wraps the daemon's.
fn exec_section(
    state: RoomWorkspacePanelState,
    rooms: Rooms,
    actor: impl Fn() -> Option<(String, String)> + Copy + 'static,
) -> AnyView {
    if !exec_form_stands(state.view.get().as_ref()) {
        return ().into_any();
    }
    let busy = state.exec_busy.get();
    view! {
        <div class="rooms-workspace__compute-cmd-title">"Run a command"</div>
        <div class="rooms-workspace__compute-cmd-copy">
            "Runs in the room's container under your name; the result lands in the \
             history below."
        </div>
        {state.exec_note.get().map(|note| view! {
            <div class="rooms-workspace__compute-note">{note}</div>
        })}
        <div class="rooms-workspace__compute-cmd-form">
            <input
                class="rooms-workspace__compute-cmd-input"
                type="text"
                placeholder="npm test"
                aria-label="Command to run"
                autocomplete="off"
                spellcheck="false"
                disabled=busy
                prop:value=move || state.exec_command.get()
                on:input=move |ev| state.exec_command.set(event_target_value(&ev))
            />
            <button
                class="rooms-workspace__compute-run"
                type="button"
                title="Run this command in the room's workspace"
                disabled=move || {
                    state.exec_busy.get()
                        || state.exec_command.with(|command| command.trim().is_empty())
                }
                on:click=move |_| {
                    let Some((key, actor_id)) = actor() else { return };
                    state.run_exec(rooms, key, actor_id);
                }
            >
                {if busy { "running\u{2026}" } else { "run" }}
            </button>
        </div>
    }
    .into_any()
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
            note: RwSignal::new(None),
            working: RwSignal::new(None),
            confirm_destroy: RwSignal::new(false),
            secret_name: RwSignal::new(String::new()),
            secret_value: RwSignal::new(String::new()),
            secrets_note: RwSignal::new(None),
            secrets_busy: RwSignal::new(false),
            exec_command: RwSignal::new(String::new()),
            exec_note: RwSignal::new(None),
            exec_busy: RwSignal::new(false),
            confirm_purge: RwSignal::new(None),
            purge_note: RwSignal::new(None),
            purge_busy: RwSignal::new(false),
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
            Some((PanelLane::Files, "files down".to_string())),
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
            Some((PanelLane::File, "file down".to_string())),
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
            .set(Some((PanelLane::File, "file down".to_string())));
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
            Some((PanelLane::Execs, "boom".to_string()))
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
            Some((PanelLane::Execs, "execs down".to_string())),
            "a status success must not wipe a live execs failure"
        );
        state.publish_execs(Ok(ExecsView::Rows(Vec::new())), true);
        assert_eq!(state.error.get_untracked(), None);

        assert!(lane_success_clears(
            Some(PanelLane::Status),
            PanelLane::Status
        ));
        assert!(!lane_success_clears(
            Some(PanelLane::Execs),
            PanelLane::Status
        ));
        assert!(!lane_success_clears(None, PanelLane::Execs));
    }

    // ---- gates --------------------------------------------------------------

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

    // ---- the lifecycle wire -------------------------------------------------

    #[test]
    fn the_lifecycle_urls_assert_the_actor_and_never_send_flush() {
        assert_eq!(
            provision_url("http://d", "team room", "user@host"),
            "http://d/v1/rooms/persistent/team%20room/workspace/provision?actor_id=user%40host"
        );
        let destroy = destroy_url("http://d", "k", "a");
        assert_eq!(
            destroy,
            "http://d/v1/rooms/persistent/k/workspace/destroy?actor_id=a"
        );
        assert!(
            !destroy.contains("flush"),
            "v1 must never offer the flush skip \u{2014} the default flush is what saves the work"
        );
    }

    /// The fresh 201 and the idempotent 200 both carry the projection, and
    /// both land — the daemon and Bedrock hold the idempotency, not this
    /// side.
    #[test]
    fn a_provision_reply_with_a_projection_lands() {
        for status in [201u16, 200] {
            assert_eq!(
                classify_lifecycle(
                    WorkspaceCommand::Provision,
                    status,
                    Some(body(status_json()))
                ),
                LifecycleOutcome::Landed("Workspace provisioned.".to_string())
            );
        }
    }

    /// Destroy's landed sentence follows the flush report, and a failed
    /// final flush must never read as saved.
    #[test]
    fn a_destroy_reply_sentences_its_flush_honestly() {
        let landed =
            |json: &str| classify_lifecycle(WorkspaceCommand::Destroy, 200, Some(body(json)));
        assert_eq!(
            landed(r#"{"destroyed": true, "flush": null}"#),
            LifecycleOutcome::Landed("Workspace destroyed.".to_string())
        );
        assert_eq!(
            landed(r#"{"destroyed": true, "flush": {"scanned": 40, "changed": 0}}"#),
            LifecycleOutcome::Landed(
                "Workspace destroyed \u{2014} nothing had changed since the last flush."
                    .to_string()
            )
        );
        assert_eq!(
            landed(r#"{"destroyed": true, "flush": {"scanned": 40, "changed": 1}}"#),
            LifecycleOutcome::Landed(
                "Workspace destroyed \u{2014} 1 changed file flushed back to Bedrock first."
                    .to_string()
            )
        );
        assert_eq!(
            landed(r#"{"destroyed": true, "flush": {"scanned": 40, "changed": 12}}"#),
            LifecycleOutcome::Landed(
                "Workspace destroyed \u{2014} 12 changed files flushed back to Bedrock first."
                    .to_string()
            )
        );
        let failed = landed(
            r#"{"destroyed": true,
                "flush": {"error": "runtime unreachable", "changed": 0, "scanned": 0}}"#,
        );
        let LifecycleOutcome::Landed(sentence) = failed else {
            panic!("a destroy that closed the record landed, whatever the flush said");
        };
        assert!(sentence.contains("flush failed"), "got: {sentence}");
        assert!(
            !sentence.contains("flushed back"),
            "must not read as saved: {sentence}"
        );
    }

    /// The typed lifecycle answers, most of all the owner gate: how the
    /// room is shaped for everyone who is not the owner, and never
    /// breakage. The identity-map refusals stay failures in words, and a
    /// daemon predating the leaves — its typed 404 or the bare one — reads
    /// as the deployment's honest "not yet".
    #[test]
    fn lifecycle_states_classify_totally() {
        let refused = body(
            r#"{"ok": false, "code": "workspace_not_owner_principal",
                "error": "an owner verb forwards only for the principal"}"#,
        );
        assert_eq!(
            classify_lifecycle(WorkspaceCommand::Provision, 403, Some(refused)),
            LifecycleOutcome::State(
                "Only the room owner can provision or destroy the workspace.".to_string()
            )
        );
        let racing = body(
            r#"{"error": "A workspace for this room is already being provisioned.",
                "code": "workspace_provisioning"}"#,
        );
        assert_eq!(
            classify_lifecycle(WorkspaceCommand::Provision, 409, Some(racing)),
            LifecycleOutcome::State(
                "A workspace for this room is already being provisioned.".to_string()
            )
        );
        let gone = body(
            r#"{"error": "This room has no workspace to destroy.", "code": "workspace_absent"}"#,
        );
        assert_eq!(
            classify_lifecycle(WorkspaceCommand::Destroy, 409, Some(gone)),
            LifecycleOutcome::State("This room has no workspace to destroy.".to_string())
        );
        let unmapped = body(r#"{"ok": false, "code": "workspace_actor_unmapped"}"#);
        assert_eq!(
            classify_lifecycle(WorkspaceCommand::Provision, 403, Some(unmapped)),
            LifecycleOutcome::Failure(
                "Your identity doesn't map to this room's compute service.".to_string()
            )
        );
        assert_eq!(
            classify_lifecycle(WorkspaceCommand::Provision, 404, None),
            LifecycleOutcome::Unavailable
        );
        let coded = body(r#"{"ok": false, "code": "workspace_route_not_allowed"}"#);
        assert_eq!(
            classify_lifecycle(WorkspaceCommand::Destroy, 404, Some(coded)),
            LifecycleOutcome::Unavailable
        );
    }

    /// The absorbed invariant, pinned: a mutation reply never moves the
    /// view. A landed command publishes its sentence and nothing else — the
    /// re-read that follows is what moves the panel.
    #[test]
    fn a_lifecycle_reply_never_moves_the_view() {
        let state = fresh_state();
        let standing = WorkspaceView::Present(Box::new(WorkspaceProjection {
            status: "ready".to_string(),
            ..WorkspaceProjection::default()
        }));
        state.view.set(Some(standing.clone()));
        state.working.set(Some(WorkspaceCommand::Destroy));
        state.publish_lifecycle(
            WorkspaceCommand::Destroy,
            LifecycleOutcome::Landed("Workspace destroyed.".to_string()),
            true,
        );
        assert_eq!(
            state.view.get_untracked(),
            Some(standing),
            "the reply's claim must wait for the readback"
        );
        assert_eq!(
            state.note.get_untracked().as_deref(),
            Some("Workspace destroyed.")
        );
        assert_eq!(state.working.get_untracked(), None);
    }

    /// A lifecycle publish honors the same admissions as every read: a
    /// stale room publishes nothing, a failure takes the alert voice and
    /// stands through read successes, and the unavailable answer is a
    /// stated fact in the calm one.
    #[test]
    fn a_lifecycle_publish_admits_and_isolates() {
        let state = fresh_state();
        state.working.set(Some(WorkspaceCommand::Provision));
        state.publish_lifecycle(
            WorkspaceCommand::Provision,
            LifecycleOutcome::Landed("Workspace provisioned.".to_string()),
            false,
        );
        assert_eq!(state.note.get_untracked(), None);
        assert_eq!(
            state.working.get_untracked(),
            Some(WorkspaceCommand::Provision),
            "a stale publish must not clear another room's in-flight state"
        );

        state.publish_lifecycle(
            WorkspaceCommand::Provision,
            LifecycleOutcome::Failure("refused".to_string()),
            true,
        );
        assert_eq!(
            state.error.get_untracked(),
            Some((PanelLane::Lifecycle, "refused".to_string()))
        );
        state.publish_status(Ok(WorkspaceView::Absent), true);
        state.publish_execs(Ok(ExecsView::Rows(Vec::new())), true);
        assert_eq!(
            state.error.get_untracked(),
            Some((PanelLane::Lifecycle, "refused".to_string())),
            "a read lane's success must not wipe a lifecycle failure"
        );

        state.publish_lifecycle(
            WorkspaceCommand::Destroy,
            LifecycleOutcome::Unavailable,
            true,
        );
        assert_eq!(
            state.note.get_untracked().as_deref(),
            Some("Destroying isn't available on this deployment yet.")
        );
    }

    /// The confirm's whole disarm surface, pinned — the polish debt the
    /// repo panel's unbind recorded: close, reset, and the view flipping
    /// under it. Armed against one workspace state, it must never fire at
    /// another — while the timestamp churn of an ordinary silent refresh
    /// must not defuse the operator mid-decision.
    #[test]
    fn the_destroy_confirm_disarms_on_close_reset_and_view_flips() {
        let ready = || {
            WorkspaceView::Present(Box::new(WorkspaceProjection {
                status: "ready".to_string(),
                ..WorkspaceProjection::default()
            }))
        };
        let state = fresh_state();
        state.view.set(Some(ready()));
        state.confirm_destroy.set(true);
        state.close_panel();
        assert!(!state.confirm_destroy.get_untracked(), "close must disarm");

        state.confirm_destroy.set(true);
        state.reset();
        assert!(!state.confirm_destroy.get_untracked(), "reset must disarm");

        state.view.set(Some(ready()));
        state.confirm_destroy.set(true);
        let aged = WorkspaceProjection {
            status: "ready".to_string(),
            last_active_at: Some("2026-08-29T10:00:00.000Z".to_string()),
            ..WorkspaceProjection::default()
        };
        state.publish_status(Ok(WorkspaceView::Present(Box::new(aged))), true);
        assert!(
            state.confirm_destroy.get_untracked(),
            "a timestamp churn is not a flip"
        );

        // The workspace another member destroyed flips the status — the
        // armed confirm goes with it.
        let destroyed = WorkspaceProjection {
            status: "destroyed".to_string(),
            ..WorkspaceProjection::default()
        };
        state.publish_status(Ok(WorkspaceView::Present(Box::new(destroyed))), true);
        assert!(
            !state.confirm_destroy.get_untracked(),
            "a status flip must disarm"
        );

        state.view.set(Some(ready()));
        state.confirm_destroy.set(true);
        state.publish_status(Ok(WorkspaceView::Absent), true);
        assert!(
            !state.confirm_destroy.get_untracked(),
            "a shape flip must disarm"
        );
    }

    /// Which verbs stand against which view: provision claims absence and
    /// closed records, destroy needs a standing one, failed earns both.
    /// Never authorization — the daemon's owner gate answers that.
    #[test]
    fn the_lifecycle_verbs_follow_the_view() {
        let present = |status: &str| {
            WorkspaceView::Present(Box::new(WorkspaceProjection {
                status: status.to_string(),
                ..WorkspaceProjection::default()
            }))
        };
        assert_eq!(lifecycle_verbs(Some(&WorkspaceView::Absent)), (true, false));
        assert_eq!(lifecycle_verbs(Some(&present("ready"))), (false, true));
        assert_eq!(
            lifecycle_verbs(Some(&present("provisioning"))),
            (false, true)
        );
        assert_eq!(lifecycle_verbs(Some(&present("failed"))), (true, true));
        assert_eq!(lifecycle_verbs(Some(&present("destroyed"))), (true, false));
        assert_eq!(
            lifecycle_verbs(Some(&WorkspaceView::Unavailable)),
            (false, false)
        );
        assert_eq!(
            lifecycle_verbs(Some(&WorkspaceView::NotFederated)),
            (false, false)
        );
        assert_eq!(lifecycle_verbs(None), (false, false));
    }

    // ---- the secrets wire ---------------------------------------------------
    //
    // Names only, throughout: no fixture here carries a secret value,
    // because no reply on this wire does either.

    #[test]
    fn the_secrets_url_asserts_the_actor() {
        assert_eq!(
            secrets_set_url("http://d", "team room", "user@host"),
            "http://d/v1/rooms/persistent/team%20room/workspace/secrets/set\
             ?actor_id=user%40host"
        );
    }

    /// The landed reply is `{set, removed, total}` — names the owner just
    /// asserted, and the sentence renders them and nothing else.
    #[test]
    fn a_secrets_reply_lands_names_only() {
        assert_eq!(
            classify_secrets(
                200,
                Some(body(r#"{"set": ["GH_TOKEN"], "removed": [], "total": 1}"#))
            ),
            LifecycleOutcome::Landed("Set GH_TOKEN \u{2014} 1 secret stored.".to_string())
        );
        assert_eq!(
            classify_secrets(
                200,
                Some(body(
                    r#"{"set": ["NPM_TOKEN"], "removed": ["OLD_TOKEN"], "total": 2}"#
                ))
            ),
            LifecycleOutcome::Landed(
                "Set NPM_TOKEN, removed OLD_TOKEN \u{2014} 2 secrets stored.".to_string()
            )
        );
        // A null that removed nothing lands too — the record is the truth,
        // and "nothing changed" is what it says.
        assert_eq!(
            classify_secrets(200, Some(body(r#"{"set": [], "removed": [], "total": 3}"#))),
            LifecycleOutcome::Landed("Nothing changed \u{2014} 3 secrets stored.".to_string())
        );
    }

    /// Every refusal reads as a state, not an error: the unconfigured host
    /// (production's live answer until a human sets OCEAN_ROOM_SECRET_KEY),
    /// the missing workspace, the owner gate, the daemon's 32 KiB body cap.
    /// The identity-map refusal stays a failure in words, an unknown 400
    /// relays Bedrock's own sentence, and a route-less deployment is said
    /// plainly.
    #[test]
    fn secrets_states_classify_totally() {
        let unconfigured = body(
            r#"{"error": "Room secrets require OCEAN_ROOM_SECRET_KEY on the Bedrock host.",
                "details": {"code": "secrets_unconfigured"}}"#,
        );
        assert_eq!(
            classify_secrets(501, Some(unconfigured)),
            LifecycleOutcome::State(
                "This Bedrock host isn't configured for room secrets.".to_string()
            )
        );
        let absent = body(
            r#"{"error": "This room has no workspace. Provision one first.",
                "details": {"code": "workspace_absent"}}"#,
        );
        assert_eq!(
            classify_secrets(409, Some(absent)),
            LifecycleOutcome::State(
                "Secrets need a workspace \u{2014} provision one first.".to_string()
            )
        );
        let gated = body(r#"{"ok": false, "code": "workspace_not_owner_principal"}"#);
        assert_eq!(
            classify_secrets(403, Some(gated)),
            LifecycleOutcome::State("Only the room owner can set secrets.".to_string())
        );
        let capped = body(r#"{"ok": false, "code": "workspace_request_too_large"}"#);
        assert_eq!(
            classify_secrets(413, Some(capped)),
            LifecycleOutcome::State(
                "That secret is larger than the 32 KiB a workspace request can carry.".to_string()
            )
        );
        let unmapped = body(r#"{"ok": false, "code": "workspace_actor_unmapped"}"#);
        assert_eq!(
            classify_secrets(403, Some(unmapped)),
            LifecycleOutcome::Failure(
                "Your identity doesn't map to this room's compute service.".to_string()
            )
        );
        // Bedrock's 400s (a bad name, a non-string value) carry no code this
        // panel knows; its own precise sentence is the answer to relay.
        let invalid = body(r#"{"error": "secrets.gh_token must match ^[A-Z][A-Z0-9_]{0,63}$."}"#);
        assert_eq!(
            classify_secrets(400, Some(invalid)),
            LifecycleOutcome::Failure(
                "The secrets set was refused: secrets.gh_token must match \
                 ^[A-Z][A-Z0-9_]{0,63}$."
                    .to_string()
            )
        );
        assert_eq!(classify_secrets(404, None), LifecycleOutcome::Unavailable);
        let coded = body(r#"{"ok": false, "code": "workspace_route_not_allowed"}"#);
        assert_eq!(
            classify_secrets(404, Some(coded)),
            LifecycleOutcome::Unavailable
        );
    }

    /// The form stands wherever the panel does — a workspace in any status,
    /// or an honest absence (submitting then earns "provision first") — and
    /// never over a deployment that answered "not yet".
    #[test]
    fn the_secrets_form_stands_with_an_answer() {
        assert!(secrets_form_stands(Some(&WorkspaceView::Present(
            Box::default()
        ))));
        assert!(secrets_form_stands(Some(&WorkspaceView::Absent)));
        assert!(!secrets_form_stands(Some(&WorkspaceView::Unavailable)));
        assert!(!secrets_form_stands(Some(&WorkspaceView::NotFederated)));
        assert!(!secrets_form_stands(None));
    }

    /// The status body's `secrets` list rides into the Present view, and
    /// absent-vs-empty survives the trip: a Bedrock that predates the list
    /// sends no `secrets` key — the production answer until it redeploys —
    /// and that must stay UNKNOWN, never the false "none stored" an empty
    /// list honestly is.
    #[test]
    fn the_secret_names_ride_the_status_answer() {
        let listed = body(
            r#"{"workspace": {"status": "ready", "driver": "cloudflare"},
                "secrets": [
                    {"name": "GH_TOKEN", "key_id": "k1",
                     "updated_at": "2026-08-27T10:00:00.000Z"},
                    {"name": "NPM_TOKEN", "key_id": "k1",
                     "updated_at": "2026-08-20T09:00:00.000Z"}
                ]}"#,
        );
        let view = classify_status(200, Some(listed)).unwrap();
        let rows = secret_rows(Some(&view)).expect("the rows must survive classify");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "GH_TOKEN");
        assert_eq!(rows[0].key_id.as_deref(), Some("k1"));
        assert_eq!(
            rows[0].updated_at.as_deref(),
            Some("2026-08-27T10:00:00.000Z")
        );

        // No `secrets` key at all — the pre-#43 status body verbatim.
        let older = classify_status(200, Some(body(status_json()))).unwrap();
        assert_eq!(
            secret_rows(Some(&older)),
            None,
            "an older Bedrock must render no claim"
        );

        // An empty list is Bedrock's own answer: none stored.
        let bare = body(r#"{"workspace": {"status": "ready"}, "secrets": []}"#);
        let view = classify_status(200, Some(bare)).unwrap();
        assert_eq!(secret_rows(Some(&view)), Some(&[][..]));

        // No other shape has a status answer to carry rows.
        assert_eq!(secret_rows(Some(&WorkspaceView::Absent)), None);
        assert_eq!(secret_rows(None), None);
    }

    /// A secrets-only change is not a flip: the silent status refresh a
    /// landed set fires must never disarm a primed destroy or purge
    /// confirm, exactly like the timestamp churn beside it.
    #[test]
    fn a_secrets_change_is_not_a_view_flip() {
        let ready = |secrets: Option<Vec<SecretRow>>| {
            WorkspaceView::Present(Box::new(WorkspaceProjection {
                status: "ready".to_string(),
                secrets,
                ..WorkspaceProjection::default()
            }))
        };
        let grown = ready(Some(vec![SecretRow {
            name: "GH_TOKEN".to_string(),
            ..SecretRow::default()
        }]));
        assert!(!view_flip(Some(&ready(None)), &grown));
        assert!(!view_flip(Some(&grown), &ready(Some(Vec::new()))));
        assert!(
            view_flip(Some(&grown), &WorkspaceView::Absent),
            "a shape flip still flips"
        );
    }

    /// The write-only discipline's mechanics: a submission takes the value
    /// out of its signal (trimmed, like the name — a pasted token's
    /// trailing newline must not be stored), an empty half takes nothing,
    /// and what remains behind is only ever the name.
    #[test]
    fn a_submission_takes_the_value_and_leaves_the_name() {
        let state = fresh_state();
        assert_eq!(state.take_secret_submission(), None);
        state.secret_name.set(" GH_TOKEN ".to_string());
        assert_eq!(
            state.take_secret_submission(),
            None,
            "a name without a value must take nothing"
        );
        state.secret_value.set(" v \n".to_string());
        assert_eq!(
            state.take_secret_submission(),
            Some(("GH_TOKEN".to_string(), "v".to_string()))
        );
        assert_eq!(
            state.secret_value.get_untracked(),
            "",
            "the value must not survive the submission"
        );
        assert_eq!(
            state.secret_name.get_untracked(),
            " GH_TOKEN ",
            "the name survives for a retry"
        );
    }

    /// The secrets publish honors the panel's admissions: a stale room
    /// publishes nothing, a landed set spends the form and sentences its
    /// names, a typed state keeps the name for the retry it invites, and a
    /// failure takes the alert voice in its own lane — standing through
    /// other lanes' successes like every other failure here.
    #[test]
    fn a_secrets_publish_admits_and_isolates() {
        let state = fresh_state();
        state.secrets_busy.set(true);
        state.secret_name.set("GH_TOKEN".to_string());
        state.publish_secrets(
            LifecycleOutcome::Landed("Set GH_TOKEN \u{2014} 1 secret stored.".to_string()),
            false,
        );
        assert!(
            state.secrets_busy.get_untracked(),
            "a stale publish must not clear another room's in-flight state"
        );
        assert_eq!(state.secrets_note.get_untracked(), None);

        state.publish_secrets(
            LifecycleOutcome::Landed("Set GH_TOKEN \u{2014} 1 secret stored.".to_string()),
            true,
        );
        assert!(!state.secrets_busy.get_untracked());
        assert_eq!(
            state.secrets_note.get_untracked().as_deref(),
            Some("Set GH_TOKEN \u{2014} 1 secret stored.")
        );
        assert_eq!(
            state.secret_name.get_untracked(),
            "",
            "a landed set spends the form"
        );

        state.secret_name.set("GH_TOKEN".to_string());
        state.publish_secrets(
            LifecycleOutcome::State(
                "Secrets need a workspace \u{2014} provision one first.".to_string(),
            ),
            true,
        );
        assert_eq!(
            state.secret_name.get_untracked(),
            "GH_TOKEN",
            "a typed state keeps the name for the retry it invites"
        );

        state.publish_secrets(LifecycleOutcome::Failure("refused".to_string()), true);
        assert_eq!(
            state.error.get_untracked(),
            Some((PanelLane::Secrets, "refused".to_string()))
        );
        state.publish_status(Ok(WorkspaceView::Absent), true);
        assert_eq!(
            state.error.get_untracked(),
            Some((PanelLane::Secrets, "refused".to_string())),
            "a read lane's success must not wipe a secrets failure"
        );

        state.publish_secrets(LifecycleOutcome::Unavailable, true);
        assert_eq!(
            state.secrets_note.get_untracked().as_deref(),
            Some("Setting secrets isn't available on this deployment yet.")
        );
    }

    // ---- the exec wire ------------------------------------------------------

    #[test]
    fn the_exec_url_asserts_the_actor() {
        assert_eq!(
            exec_url("http://d", "team room", "user@host"),
            "http://d/v1/rooms/persistent/team%20room/workspace/exec?actor_id=user%40host"
        );
    }

    /// The landed reply is Bedrock's own row, whatever its exit — a
    /// member's failing command still landed — and a timeout says where
    /// the output went.
    #[test]
    fn an_exec_reply_lands_whatever_its_exit() {
        let clean = body(
            r#"{"exec": {"id": "e1", "command": "npm test",
                "status": "exited", "exit_code": 0}}"#,
        );
        assert_eq!(
            classify_exec(200, Some(clean)),
            LifecycleOutcome::Landed("Command ran \u{2014} exited 0.".to_string())
        );
        let failing = body(
            r#"{"exec": {"id": "e2", "command": "npm test",
                "status": "exited", "exit_code": 2}}"#,
        );
        assert_eq!(
            classify_exec(200, Some(failing)),
            LifecycleOutcome::Landed("Command ran \u{2014} exited 2.".to_string())
        );
        let hung = body(r#"{"exec": {"id": "e3", "command": "sleep 1200", "status": "timeout"}}"#);
        assert_eq!(
            classify_exec(200, Some(hung)),
            LifecycleOutcome::Landed(
                "The command timed out \u{2014} the history keeps what it wrote.".to_string()
            )
        );
    }

    /// Typed answers read as states in the calm voice — the workspace that
    /// isn't ready and the daemon's body cap both invite a retry with the
    /// command intact. The identity-map refusal stays a failure in words,
    /// an unknown 400 relays Bedrock's own sentence, and a route-less
    /// deployment is said plainly.
    #[test]
    fn exec_states_classify_totally() {
        let absent = body(
            r#"{"error": "This room has no workspace. Provision one first.",
                "details": {"code": "workspace_absent"}}"#,
        );
        assert_eq!(
            classify_exec(409, Some(absent)),
            LifecycleOutcome::State(
                "Running a command needs a workspace \u{2014} provision one first.".to_string()
            )
        );
        let provisioning = body(
            r#"{"error": "The room workspace is still provisioning.",
                "details": {"code": "workspace_provisioning"}}"#,
        );
        assert_eq!(
            classify_exec(409, Some(provisioning)),
            LifecycleOutcome::State(
                "The workspace is still provisioning \u{2014} try again once it's ready."
                    .to_string()
            )
        );
        let capped = body(r#"{"ok": false, "code": "workspace_request_too_large"}"#);
        assert_eq!(
            classify_exec(413, Some(capped)),
            LifecycleOutcome::State(
                "That command is larger than the 32 KiB a workspace request can carry.".to_string()
            )
        );
        let unmapped = body(r#"{"ok": false, "code": "workspace_actor_unmapped"}"#);
        assert_eq!(
            classify_exec(403, Some(unmapped)),
            LifecycleOutcome::Failure(
                "Your identity doesn't map to this room's compute service.".to_string()
            )
        );
        let invalid = body(r#"{"error": "command must be a non-empty string."}"#);
        assert_eq!(
            classify_exec(400, Some(invalid)),
            LifecycleOutcome::Failure(
                "The command was refused: command must be a non-empty string.".to_string()
            )
        );
        assert_eq!(classify_exec(404, None), LifecycleOutcome::Unavailable);
        let coded = body(r#"{"ok": false, "code": "workspace_route_not_allowed"}"#);
        assert_eq!(
            classify_exec(404, Some(coded)),
            LifecycleOutcome::Unavailable
        );
    }

    /// The form stands wherever the secrets form does — on an answer,
    /// never over a deployment's "not yet".
    #[test]
    fn the_exec_form_stands_with_an_answer() {
        assert!(exec_form_stands(Some(&WorkspaceView::Present(
            Box::default()
        ))));
        assert!(exec_form_stands(Some(&WorkspaceView::Absent)));
        assert!(!exec_form_stands(Some(&WorkspaceView::Unavailable)));
        assert!(!exec_form_stands(Some(&WorkspaceView::NotFederated)));
        assert!(!exec_form_stands(None));
    }

    /// A submission trims, an empty compose takes nothing — and unlike the
    /// secret value, the command stays in its signal: only a landed run
    /// spends the form.
    #[test]
    fn an_exec_submission_trims_and_keeps_the_command() {
        let state = fresh_state();
        assert_eq!(state.take_exec_submission(), None);
        state.exec_command.set("  \n".to_string());
        assert_eq!(state.take_exec_submission(), None);
        state.exec_command.set(" npm test \n".to_string());
        assert_eq!(state.take_exec_submission(), Some("npm test".to_string()));
        assert_eq!(
            state.exec_command.get_untracked(),
            " npm test \n",
            "the command survives for the retry a refusal invites"
        );
    }

    /// The exec publish honors the panel's admissions: a stale room
    /// publishes nothing — a 960s command outlives panel switches — a
    /// landed run spends the form, a typed state keeps the command, and a
    /// failure takes the alert voice in its own lane, standing through
    /// other lanes' successes.
    #[test]
    fn an_exec_publish_admits_and_isolates() {
        let state = fresh_state();
        state.exec_busy.set(true);
        state.exec_command.set("npm test".to_string());
        state.publish_exec(
            LifecycleOutcome::Landed("Command ran \u{2014} exited 0.".to_string()),
            false,
        );
        assert!(
            state.exec_busy.get_untracked(),
            "a stale publish must not clear another room's in-flight state"
        );
        assert_eq!(state.exec_note.get_untracked(), None);

        state.publish_exec(
            LifecycleOutcome::Landed("Command ran \u{2014} exited 0.".to_string()),
            true,
        );
        assert!(!state.exec_busy.get_untracked());
        assert_eq!(
            state.exec_note.get_untracked().as_deref(),
            Some("Command ran \u{2014} exited 0.")
        );
        assert_eq!(
            state.exec_command.get_untracked(),
            "",
            "a landed run spends the form"
        );

        state.exec_command.set("npm test".to_string());
        state.publish_exec(
            LifecycleOutcome::State(
                "Running a command needs a workspace \u{2014} provision one first.".to_string(),
            ),
            true,
        );
        assert_eq!(
            state.exec_command.get_untracked(),
            "npm test",
            "a typed state keeps the command for the retry it invites"
        );

        state.publish_exec(LifecycleOutcome::Failure("refused".to_string()), true);
        assert_eq!(
            state.error.get_untracked(),
            Some((PanelLane::Exec, "refused".to_string()))
        );
        state.publish_status(Ok(WorkspaceView::Absent), true);
        assert_eq!(
            state.error.get_untracked(),
            Some((PanelLane::Exec, "refused".to_string())),
            "a read lane's success must not wipe an exec failure"
        );

        state.publish_exec(LifecycleOutcome::Unavailable, true);
        assert_eq!(
            state.exec_note.get_untracked().as_deref(),
            Some("Running commands isn't available on this deployment yet.")
        );
    }

    /// A room switch clears the run form with everything else.
    #[test]
    fn reset_clears_the_run_form() {
        let state = fresh_state();
        state.exec_command.set("npm test".to_string());
        state.exec_note.set(Some("stale".to_string()));
        state.exec_busy.set(true);
        state.reset();
        assert_eq!(state.exec_command.get_untracked(), "");
        assert_eq!(state.exec_note.get_untracked(), None);
        assert!(!state.exec_busy.get_untracked());
    }

    /// The close and reset paths both clear the value signal: a pasted
    /// secret must not outlive the form it was pasted into.
    #[test]
    fn no_pasted_value_survives_close_or_reset() {
        let state = fresh_state();
        state.secret_value.set("v".to_string());
        state.close_panel();
        assert_eq!(state.secret_value.get_untracked(), "");

        state.secret_value.set("v".to_string());
        state.secrets_note.set(Some("stale".to_string()));
        state.reset();
        assert_eq!(state.secret_value.get_untracked(), "");
        assert_eq!(state.secret_name.get_untracked(), "");
        assert_eq!(state.secrets_note.get_untracked(), None);
        assert!(!state.secrets_busy.get_untracked());
    }

    #[test]
    fn the_purge_url_asserts_the_actor() {
        assert_eq!(
            execs_purge_url("http://d", "team room", "user@host"),
            "http://d/v1/rooms/persistent/team%20room/workspace/execs/purge?actor_id=user%40host"
        );
    }

    /// A purged row says "taken back" whoever asks: the owner sees NULL
    /// tails (which unpurged would mean "still running"), everyone else
    /// sees them absent (which unpurged would mean "withheld") — both
    /// renders would lie about a row the owner blanked, so the flag wins
    /// over the tail fields.
    #[test]
    fn a_purged_row_reads_as_taken_back_not_silence() {
        let owner_view: ExecRow = serde_json::from_str(
            r#"{"id": "e1", "command": "npm test", "status": "exited", "exit_code": 0,
                "purged": true, "purged_at": "2026-08-29T10:00:00.000Z",
                "stdout_tail": null, "stderr_tail": null,
                "stdout_clipped": false, "stderr_clipped": false}"#,
        )
        .unwrap();
        assert!(owner_view.purged);
        assert_eq!(row_tails(&owner_view), RowTails::Purged);

        let member_view: ExecRow = serde_json::from_str(
            r#"{"id": "e1", "command": "npm test", "status": "exited", "exit_code": 0,
                "purged": true, "purged_at": "2026-08-29T10:00:00.000Z"}"#,
        )
        .unwrap();
        assert_eq!(row_tails(&member_view), RowTails::Purged);

        // An unpurged row is untouched by the flag's default: absent tails
        // still read as withheld, null ones as running.
        let unpurged: ExecRow = serde_json::from_str(
            r#"{"id": "e2", "command": "npm test", "status": "exited", "exit_code": 0}"#,
        )
        .unwrap();
        assert!(!unpurged.purged);
        assert_eq!(row_tails(&unpurged), RowTails::Withheld);
    }

    /// The landed sentence carries the reply's own count, and zero is an
    /// answer on both targets: an already-purged named row, or a clean
    /// history.
    #[test]
    fn a_purge_reply_lands_its_count() {
        let all = PurgeTarget::All;
        let one = PurgeTarget::One("e1".to_string());
        assert_eq!(
            classify_exec_purge(&all, 200, Some(body(r#"{"purged": 3, "exec_id": null}"#))),
            LifecycleOutcome::Landed("Took back the stored output of 3 commands.".to_string())
        );
        assert_eq!(
            classify_exec_purge(&all, 200, Some(body(r#"{"purged": 1, "exec_id": null}"#))),
            LifecycleOutcome::Landed("Took back the stored output of 1 command.".to_string())
        );
        assert_eq!(
            classify_exec_purge(&all, 200, Some(body(r#"{"purged": 0, "exec_id": null}"#))),
            LifecycleOutcome::Landed("No stored output stood to take back.".to_string())
        );
        assert_eq!(
            classify_exec_purge(&one, 200, Some(body(r#"{"purged": 1, "exec_id": "e1"}"#))),
            LifecycleOutcome::Landed(
                "Output taken back \u{2014} it no longer reads back for anyone.".to_string()
            )
        );
        assert_eq!(
            classify_exec_purge(&one, 200, Some(body(r#"{"purged": 0, "exec_id": "e1"}"#))),
            LifecycleOutcome::Landed("That command's output was already taken back.".to_string())
        );
    }

    /// Typed answers read as states in the calm voice; the uncoded 404
    /// depends on the target — purge-all has no other 404 so it is the
    /// deployment's honest "not yet", while a named purge relays Bedrock's
    /// own sentence rather than guessing between a vanished exec and a
    /// route-less deployment.
    #[test]
    fn purge_states_classify_totally() {
        let all = PurgeTarget::All;
        let one = PurgeTarget::One("e1".to_string());
        let running = body(
            r#"{"ok": false,
                "error": "This command is still running; purge it once it finishes.",
                "details": {"code": "exec_running"}}"#,
        );
        assert_eq!(
            classify_exec_purge(&one, 409, Some(running)),
            LifecycleOutcome::State(
                "This command is still running \u{2014} its output can be taken back once \
                 it finishes."
                    .to_string()
            )
        );
        let not_owner = body(r#"{"ok": false, "code": "workspace_not_owner_principal"}"#);
        assert_eq!(
            classify_exec_purge(&all, 403, Some(not_owner)),
            LifecycleOutcome::State(
                "Only the room owner can take back command output.".to_string()
            )
        );
        let coded = body(r#"{"ok": false, "code": "workspace_route_not_allowed"}"#);
        assert_eq!(
            classify_exec_purge(&all, 404, Some(coded)),
            LifecycleOutcome::Unavailable
        );
        assert_eq!(
            classify_exec_purge(&all, 404, None),
            LifecycleOutcome::Unavailable
        );
        // An old Bedrock behind a new daemon: uncoded 404, relayed verbatim.
        let routeless = body(r#"{"ok": false, "error": "Route not found."}"#);
        assert_eq!(
            classify_exec_purge(&all, 404, Some(routeless)),
            LifecycleOutcome::Unavailable
        );
        let vanished = body(r#"{"ok": false, "error": "No such exec in this room."}"#);
        assert_eq!(
            classify_exec_purge(&one, 404, Some(vanished)),
            LifecycleOutcome::State(
                "The purge didn't land: No such exec in this room.".to_string()
            )
        );
        let malformed = body(r#"{"ok": false, "error": "exec_id must be a UUID."}"#);
        assert_eq!(
            classify_exec_purge(&one, 400, Some(malformed)),
            LifecycleOutcome::Failure("The purge was refused: exec_id must be a UUID.".to_string())
        );
        let unmapped = body(r#"{"ok": false, "code": "workspace_actor_unmapped"}"#);
        assert_eq!(
            classify_exec_purge(&all, 403, Some(unmapped)),
            LifecycleOutcome::Failure(
                "Your identity doesn't map to this room's compute service.".to_string()
            )
        );
    }

    /// The purge confirm disarms exactly where the destroy confirm does:
    /// close, reset, and a status view flip — armed against one workspace
    /// state, it must not fire at another.
    #[test]
    fn the_purge_confirm_disarms_on_close_reset_and_view_flips() {
        let ready = || {
            WorkspaceView::Present(Box::new(WorkspaceProjection {
                status: "ready".to_string(),
                ..WorkspaceProjection::default()
            }))
        };
        let state = fresh_state();
        state.view.set(Some(ready()));
        state.confirm_purge.set(Some(PurgeTarget::All));
        state.close_panel();
        assert_eq!(
            state.confirm_purge.get_untracked(),
            None,
            "close must disarm"
        );

        state
            .confirm_purge
            .set(Some(PurgeTarget::One("e1".to_string())));
        state.reset();
        assert_eq!(
            state.confirm_purge.get_untracked(),
            None,
            "reset must disarm"
        );

        state.view.set(Some(ready()));
        state.confirm_purge.set(Some(PurgeTarget::All));
        let aged = WorkspaceProjection {
            status: "ready".to_string(),
            last_active_at: Some("2026-08-29T10:00:00.000Z".to_string()),
            ..WorkspaceProjection::default()
        };
        state.publish_status(Ok(WorkspaceView::Present(Box::new(aged))), true);
        assert_eq!(
            state.confirm_purge.get_untracked(),
            Some(PurgeTarget::All),
            "a timestamp churn is not a flip"
        );

        state.publish_status(Ok(WorkspaceView::Absent), true);
        assert_eq!(
            state.confirm_purge.get_untracked(),
            None,
            "a shape flip must disarm"
        );
    }

    /// Only the room that started a purge may hear its answer, states land
    /// in the calm note, and a real refusal takes the alert its own lane
    /// owns — no read's success may wipe it.
    #[test]
    fn a_purge_publish_admits_and_isolates() {
        let state = fresh_state();
        state.purge_busy.set(true);
        state.publish_exec_purge(
            LifecycleOutcome::Landed("Output taken back.".to_string()),
            false,
        );
        assert!(
            state.purge_busy.get_untracked(),
            "a stale publish must not clear another room's in-flight state"
        );
        assert_eq!(state.purge_note.get_untracked(), None);

        state.publish_exec_purge(
            LifecycleOutcome::Landed("Output taken back.".to_string()),
            true,
        );
        assert!(!state.purge_busy.get_untracked());
        assert_eq!(
            state.purge_note.get_untracked().as_deref(),
            Some("Output taken back.")
        );

        state.publish_exec_purge(LifecycleOutcome::Failure("refused".to_string()), true);
        assert_eq!(
            state.error.get_untracked(),
            Some((PanelLane::ExecPurge, "refused".to_string()))
        );
        state.publish_status(Ok(WorkspaceView::Absent), true);
        assert_eq!(
            state.error.get_untracked(),
            Some((PanelLane::ExecPurge, "refused".to_string())),
            "a read lane's success must not wipe a purge failure"
        );

        state.publish_exec_purge(LifecycleOutcome::Unavailable, true);
        assert_eq!(
            state.purge_note.get_untracked().as_deref(),
            Some("Taking back output isn't available on this deployment yet.")
        );
    }
}
