//! The room's bound repo — the browser half of the daemon's workspace lane.
//!
//! A federated room can have a git repo bound to its Bedrock workspace, cloned
//! into its container, and built there. The daemon exposes exactly what a room
//! MEMBER may do with that binding (`room_workspace_proxy.rs`'s allowlist):
//!
//!   GET  /v1/rooms/persistent/{key}/workspace/repo         → the binding
//!   POST /v1/rooms/persistent/{key}/workspace/repo/clone   → clone it
//!   POST /v1/rooms/persistent/{key}/workspace/repo/build   → run a script
//!
//! Choosing WHAT the room builds — bind and unbind — is deliberately absent
//! from that allowlist: those are owner-only upstream, and the daemon always
//! presents the room credential's bearer, so exposing them here would hand
//! every roster participant the owner's authority. This panel says so instead
//! of pretending.
//!
//! Five properties of the wire contract shape everything below:
//!
//! 1. **Every call asserts `?actor_id=`.** The daemon roster-checks it inside
//!    the same store guard that reads the room credential, and refuses an
//!    empty one before anything leaves the process. So no request goes out
//!    until bootstrap has resolved who we are.
//! 2. **The POST bodies are strict deny-extra at Bedrock.** A clone accepts
//!    only `actor_member_id` — which the daemon strips and re-installs itself
//!    (`shape_body`) — so this side posts EXACTLY `{}`. A build additionally
//!    REQUIRES `script` (an npm-script name; Bedrock composes the command).
//! 3. **Typed refusals are states, not errors.** `workspace_absent`,
//!    `repo_unbound`, `repo_cloning`, `build_running` and `repo_not_cloned`
//!    are Bedrock answering the question honestly, relayed verbatim by the
//!    daemon. They render as plain sentences, never as failures. And the code
//!    is not always where the daemon puts its own: Bedrock's thrown refusals
//!    arrive as `{error, details: {code}}` while its 404s and the daemon's own
//!    gates carry a top-level `code`, so classification reads both.
//! 4. **A Local room has no workspace** (`room_not_federated`). The access
//!    projection already knows, so the whole section renders nothing for a
//!    Local room rather than showing a refusal for a thing that cannot exist.
//! 5. **A clone or build can outlive any sane request.** The daemon budgets
//!    960s per command and Bedrock's default build budget alone is 600s. The
//!    proxy's forward timeout is raised to match (`WorkspaceCommand` lane in
//!    `ocean-surface-proxy`), but a phone on a tunnel can still lose the
//!    long-held response while the work continues upstream — Bedrock records
//!    the exec regardless. So a clone never trusts its own POST: firing one
//!    also starts polling `GET repo` for `clone_status`, and the completion
//!    state is what the panel believes. Nor is the poller the clicker's
//!    private property: a plain read that answers `cloning` — a reload
//!    mid-clone, a second member watching — starts the same poller, so every
//!    session converges on the completion, not just the one that clicked.
//!    The daemon also relays clone outcomes onto the room transcript as
//!    System markers ("workspace repo cloned…"), so a marker on the SSE tail
//!    triggers the same silent re-read immediately — the wake accelerates
//!    the poller, it never replaces it.
//!
//! A production deployment whose daemon or Bedrock predates these routes
//! answers 404 with no code; that renders as "not available yet", plainly,
//! not as a failure. Everything that turns a reply into what the operator
//! sees is a free function below, unit-testable natively.

use gloo_net::http::Request;
use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen_futures::spawn_local;

use crate::room_workspace_panel::{is_workspace_marker, marker_wake};
use crate::rooms::{encode, RoomAccessProjection, RoomAccessState, RoomMessage, Rooms};

/// How often the poller re-reads the binding while a clone is running. The
/// clone itself takes tens of seconds to minutes; 4s keeps the panel honest
/// without leaning on the daemon.
const CLONE_POLL_MS: u32 = 4_000;

/// The opening both clone-outcome markers share ("workspace repo cloned…",
/// "workspace repo clone failed…"). The other seven marker variants say
/// nothing about the binding, so they don't wake this section.
const REPO_CLONE_MARKER_PREFIX: &str = "workspace repo clone";

/// The script the build field starts at. Bedrock has no default — `script` is
/// required on the wire — and "build" is the npm convention this control is
/// for. The operator edits it freely.
const DEFAULT_BUILD_SCRIPT: &str = "build";

// ---- Wire types -------------------------------------------------------------

/// Bedrock's `publicRepoProjection` (src/room-repo.mjs), the fields this panel
/// renders. `clone_error` is present only when the daemon's Bedrock principal
/// is the room owner — optional here, never promised.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RepoProjection {
    #[serde(default)]
    pub remote: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub clone_status: String,
    #[serde(default)]
    pub head_sha: Option<String>,
    #[serde(default)]
    pub last_cloned_at: Option<String>,
    #[serde(default)]
    pub clone_error: Option<String>,
}

/// A finished build, from the 200 body. Deliberately 200 even when the script
/// exited nonzero — Bedrock treats "the build ran and failed" as the answer
/// the caller asked for, and so does this panel.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BuildReport {
    #[serde(default)]
    pub script: String,
    #[serde(default)]
    pub outcome: String,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

/// Bedrock's thrown refusals carry their `code` here, nested under `details`
/// (its top-level error writer serializes `HttpError.details` whole), while
/// its plain 404s and every daemon-side gate put `code` at the top level.
#[derive(Debug, Default, Deserialize)]
struct ErrorDetails {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    stderr: Option<String>,
}

/// The one lenient envelope every reply on this lane fits into. There is no
/// `ok` field to lean on: Bedrock's successes don't send one and the daemon
/// relays them verbatim, so presence of `repo`/`build` is what success means.
#[derive(Debug, Default, Deserialize)]
struct RepoBody {
    #[serde(default)]
    repo: Option<RepoProjection>,
    #[serde(default)]
    build: Option<BuildReport>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    details: Option<ErrorDetails>,
}

impl RepoBody {
    /// Top-level first — the daemon's own refusals and Bedrock's coded 404s —
    /// then Bedrock's thrown refusals under `details`.
    fn refusal_code(&self) -> Option<&str> {
        self.code
            .as_deref()
            .or_else(|| self.details.as_ref().and_then(|d| d.code.as_deref()))
    }
}

// ---- Pure helpers -----------------------------------------------------------

fn repo_url(base: &str, key: &str, actor: &str) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/workspace/repo?actor_id={}",
        encode(key),
        encode(actor),
    )
}

fn clone_url(base: &str, key: &str, actor: &str) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/workspace/repo/clone?actor_id={}",
        encode(key),
        encode(actor),
    )
}

fn build_url(base: &str, key: &str, actor: &str) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/workspace/repo/build?actor_id={}",
        encode(key),
        encode(actor),
    )
}

/// What the room's binding IS right now, as far as this surface can honestly
/// say. `None` in the state signal means "not answered yet" — only a reply
/// mints one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RepoView {
    /// A binding stands. Everything the panel renders comes from here.
    Bound(RepoProjection),
    /// The room has a workspace lane but no repo bound to it. An answer, not
    /// an error — and the place the panel says binding is owner-by-API.
    Unbound,
    /// The daemon says this room is not federated. The access projection
    /// normally hides the section first; this keeps the classification total.
    NotFederated,
    /// The deployment in front of us does not serve these routes (a daemon or
    /// Bedrock predating the lane). Said plainly instead of erroring.
    Unavailable,
}

/// The two commands a member can run. One at a time — Bedrock holds a
/// mutual-exclusion lock over the checkout and answers 409 to the loser, so
/// offering parallel submits would only manufacture refusals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepoCommand {
    Clone,
    Build,
}

/// What a command reply means for the panel.
#[derive(Debug, PartialEq, Eq)]
enum CommandOutcome {
    /// The clone ran to completion; the reply carries the binding as it now
    /// stands (status `cloned`, fresh `head_sha`).
    Cloned(Box<RepoProjection>),
    /// The build ran. Success OR script failure — both are this arm; the
    /// report says which.
    Built(BuildReport),
    /// A typed state: the workspace is busy or not ready, in Bedrock's own
    /// terms. Rendered as a sentence, never as a failure.
    State(String),
    /// A refusal or fault, in words an operator can act on.
    Failure(String),
}

/// The sentence a typed workspace state earns. `None` means the code is not a
/// state — the caller falls through to the failure arm.
fn state_sentence(code: &str) -> Option<String> {
    let sentence = match code {
        "workspace_absent" => {
            "This room has no workspace container yet \u{2014} provisioning is an owner act, \
             by API for now."
        }
        "repo_unbound" => {
            "No repo is bound to this room yet \u{2014} binding is an owner act, by API for now."
        }
        "repo_cloning" => "A clone is already running for this room.",
        "build_running" => {
            "A build is already running in this room \u{2014} wait for it to finish."
        }
        "repo_not_cloned" => {
            "The repo isn't cloned into the workspace yet \u{2014} clone it first."
        }
        "room_not_federated" => "This room has no Bedrock workspace.",
        _ => return None,
    };
    Some(sentence.to_string())
}

/// The failure sentence for a coded refusal that is NOT a state. The daemon's
/// gate codes and relay codes land here.
fn failure_sentence(code: &str) -> Option<String> {
    let sentence = match code {
        "not_a_room_member" => "You're not on this room's roster.",
        "forged_workspace_actor" => {
            "An agent's workspace command is run by the daemon, not from here."
        }
        "room_access_revoked" => "This room's federation access was revoked.",
        "workspace_unavailable" => "The room's compute service can't be reached right now.",
        "workspace_upstream_protocol" => {
            "The room's compute service answered something this surface can't read."
        }
        "workspace_route_not_allowed" => {
            "This Ocean deployment doesn't expose that workspace route."
        }
        _ => return None,
    };
    Some(sentence.to_string())
}

/// Map a `GET repo` reply onto what the panel should believe. `body` is `None`
/// when the reply did not decode — which a route-less deployment produces
/// (an empty 404), so that case is an ANSWER here, not a transport fault.
fn classify_status(status: u16, body: Option<RepoBody>) -> Result<RepoView, String> {
    let Some(body) = body else {
        if status == 404 {
            return Ok(RepoView::Unavailable);
        }
        return Err(format!(
            "The repo status reply could not be read ({status})."
        ));
    };
    if let Some(repo) = body.repo {
        return Ok(RepoView::Bound(repo));
    }
    match body.refusal_code() {
        Some("repo_unbound") => Ok(RepoView::Unbound),
        Some("room_not_federated") => Ok(RepoView::NotFederated),
        Some("workspace_route_not_allowed") => Ok(RepoView::Unavailable),
        Some(code) => Err(failure_sentence(code)
            .or_else(|| state_sentence(code))
            .or_else(|| body.error.clone())
            .unwrap_or_else(|| format!("Repo status failed ({status})."))),
        // A 404 with no code is a deployment that predates the lane — the
        // daemon's own unknown-route answer, or Bedrock's. An answer.
        None if status == 404 => Ok(RepoView::Unavailable),
        None => Err(body
            .error
            .filter(|error| !error.is_empty())
            .map(|error| format!("Repo status failed: {error}"))
            .unwrap_or_else(|| format!("Repo status failed ({status})."))),
    }
}

/// Map a command reply onto what the panel should show.
fn classify_command(command: RepoCommand, status: u16, body: Option<RepoBody>) -> CommandOutcome {
    let noun = match command {
        RepoCommand::Clone => "clone",
        RepoCommand::Build => "build",
    };
    let Some(body) = body else {
        return CommandOutcome::Failure(format!("The {noun} reply could not be read ({status})."));
    };
    match command {
        RepoCommand::Clone => {
            if let Some(repo) = body.repo {
                return CommandOutcome::Cloned(Box::new(repo));
            }
        }
        RepoCommand::Build => {
            if let Some(build) = body.build {
                return CommandOutcome::Built(build);
            }
        }
    }
    if let Some(code) = body.refusal_code() {
        if let Some(sentence) = state_sentence(code) {
            return CommandOutcome::State(sentence);
        }
        if code == "repo_clone_failed" {
            // The 502 carries the git stderr tail under `details` — the one
            // part of the refusal an operator can actually act on.
            let stderr = body
                .details
                .as_ref()
                .and_then(|details| details.stderr.as_deref())
                .unwrap_or("")
                .trim();
            return CommandOutcome::Failure(if stderr.is_empty() {
                "The clone failed.".to_string()
            } else {
                format!("The clone failed: {}", clip(stderr, 400))
            });
        }
        if let Some(sentence) = failure_sentence(code) {
            return CommandOutcome::Failure(sentence);
        }
    }
    CommandOutcome::Failure(
        body.error
            .filter(|error| !error.is_empty())
            .map(|error| format!("The {noun} was refused: {error}"))
            .unwrap_or_else(|| format!("The {noun} failed ({status}).")),
    )
}

/// First `max` characters, on a char boundary, with an ellipsis when clipped.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('\u{2026}');
    out
}

/// The sentence a finished build earns. Exit code over adjectives: "exited 1"
/// is what the operator greps the build script for, "failed" is not.
fn build_sentence(report: &BuildReport) -> String {
    let took = report
        .duration_ms
        .map(|ms| format!(" in {}s", ms.div_ceil(1000)))
        .unwrap_or_default();
    match (report.outcome.as_str(), report.exit_code) {
        ("succeeded", _) => format!("Build `{}` succeeded{took}.", report.script),
        (_, Some(code)) => format!("Build `{}` exited {code}{took}.", report.script),
        _ => format!("Build `{}` {}{took}.", report.script, report.outcome),
    }
}

/// A readable name for the remote: the last two path segments, `.git`
/// stripped — `github.com/acme/site.git` and `git@github.com:acme/site.git`
/// both read "acme/site". The full remote stays in the panel.
fn remote_label(remote: &str) -> String {
    let trimmed = remote
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit(['/', ':'])
        .take(2)
        .collect::<Vec<_>>();
    let mut segments: Vec<&str> = trimmed.into_iter().rev().collect();
    segments.retain(|segment| !segment.is_empty());
    if segments.is_empty() {
        remote.to_string()
    } else {
        segments.join("/")
    }
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(10).collect()
}

/// The compact line under the rail header. One line; the panel has the rest.
fn rail_line(view: &RepoView) -> Option<String> {
    match view {
        RepoView::Bound(repo) => {
            let mut line = format!(
                "{} \u{b7} {}",
                remote_label(&repo.remote),
                repo.clone_status
            );
            if repo.clone_status == "cloned" {
                if let Some(sha) = repo.head_sha.as_deref().filter(|sha| !sha.is_empty()) {
                    line.push_str(" @ ");
                    line.push_str(&short_sha(sha));
                }
            }
            Some(line)
        }
        RepoView::Unbound => Some("No repo bound.".to_string()),
        RepoView::NotFederated | RepoView::Unavailable => None,
    }
}

/// Whether the binding says a clone is running upstream — ours or another
/// member's. Shared between the poller's continuation check and `fetch`'s
/// start check, so "a poller must exist" and "the poller keeps going" can
/// never disagree about what a running clone is.
fn clone_is_running(view: Option<&RepoView>) -> bool {
    matches!(view, Some(RepoView::Bound(repo)) if repo.clone_status == "cloning")
}

/// Whether the clone poller should keep going: while our own command is still
/// in flight, or while the binding says a clone is running (ours or another
/// member's). Extracted for the same reason every admission predicate in this
/// rail is: a guard no test can reach is a guard the next edit deletes.
fn poll_should_continue(command_in_flight: bool, view: Option<&RepoView>) -> bool {
    command_in_flight || clone_is_running(view)
}

/// Latest-wins admission for an overlapping read — same shape as
/// `room_artifacts::read_is_current`, for the same premature-publish bug class.
fn read_is_current(ticket: u64, current: u64) -> bool {
    ticket == current
}

/// Whether a transcript row is a clone-outcome marker — the one workspace
/// event that changes the binding this section renders.
fn is_repo_clone_marker(row: &RoomMessage) -> bool {
    is_workspace_marker(row) && row.body.starts_with(REPO_CLONE_MARKER_PREFIX)
}

/// Where a standing error came from. A silent read that succeeds may clear
/// only a READ failure: a command refusal ("the clone failed: …") or a
/// pre-wire refusal (empty script, unresolved identity) is an answer the
/// operator has not acted on yet, and a background poll going well says
/// nothing about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepoErrorSource {
    Read,
    Command,
}

/// Whether a successful status read clears the standing error — the
/// absorbed fix: one blipped silent poll no longer leaves an alert standing
/// over a healthy self-healing view, and a command failure stands until the
/// operator acts.
fn read_success_clears(standing: Option<RepoErrorSource>) -> bool {
    standing == Some(RepoErrorSource::Read)
}

/// Refuse a build this side already knows Bedrock will reject: `script` is
/// required on the wire, and an empty one would come back as prose about a
/// field this control should have insisted on.
fn script_refusal(script: &str) -> Option<String> {
    if script.trim().is_empty() {
        return Some("A build names a package script.".to_string());
    }
    None
}

/// Whether the repo section exists for this room at all. Only a federated
/// room has a Bedrock workspace; a Local room renders nothing rather than a
/// refusal, and `None` (no room open / still loading) also renders nothing.
fn room_is_federated(access: Option<&RoomAccessProjection>) -> bool {
    access.is_some_and(|projection| projection.state != RoomAccessState::Local)
}

/// Escape owned by the repo panel. Same contract as
/// `artifacts_escape_closes`: the panel is a fixed modal at the top of the
/// rooms surface, so it consumes the key before the drawers under it.
pub fn repo_escape_closes(panel_open: bool, default_prevented: bool) -> bool {
    panel_open && !default_prevented
}

// ---- State ------------------------------------------------------------------

/// Reactive handle for one room's repo binding.
///
/// Constructed at `RoomsWorkspace` component scope, never inside a rail
/// closure: those closures re-run on every `rooms.access` SSE update, and an
/// in-flight flag rebuilt mid-request would re-enable the clone control during
/// its own clone — a second claim on a lock the first request is holding.
#[derive(Clone, Copy)]
pub struct RoomRepoState {
    /// Daemon base URL, shared with `Daemon::url` through `Rooms::url`.
    pub url: RwSignal<String>,
    /// What the binding is, once a read has answered. `None` = not answered
    /// yet, so the rail can never assert "no repo" about a room that has not
    /// replied.
    view: RwSignal<Option<RepoView>>,
    /// A foreground read is in flight (the poller refreshes silently).
    loading: RwSignal<bool>,
    /// The most recent failure, read or command, tagged with which — a
    /// read that recovers clears only a read's failure.
    error: RwSignal<Option<(RepoErrorSource, String)>>,
    /// The marker wake's watermark: `(room generation, highest transcript
    /// seq seen)`. `None` until the open room's transcript is first sighted.
    marker_seen: RwSignal<Option<(u64, u64)>>,
    /// The typed state or outcome worth a sentence: "a build is running",
    /// "build `test` exited 1". Kept apart from `error` because these are
    /// answers, not faults, and they render in a calmer voice.
    note: RwSignal<Option<String>>,
    /// The command in flight, if any — blocks re-submit and drives labels.
    working: RwSignal<Option<RepoCommand>>,
    /// Whether the reading-measure panel is open.
    panel: RwSignal<bool>,
    /// The rail control that opens the panel, so closing hands focus back.
    open_ref: NodeRef<leptos::html::Button>,
    /// The script the build control will name. Room-scoped; reset() returns
    /// it to the convention.
    build_script: RwSignal<String>,
    /// Monotonic ticket; only the latest overlapping read may publish.
    ticket: RwSignal<u64>,
    /// Poller generation; bumping it retires any running poll loop.
    poll_epoch: RwSignal<u64>,
}

impl RoomRepoState {
    pub fn new(rooms: &Rooms) -> Self {
        Self {
            url: rooms.url,
            view: RwSignal::new(None),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            marker_seen: RwSignal::new(None),
            note: RwSignal::new(None),
            working: RwSignal::new(None),
            panel: RwSignal::new(false),
            open_ref: NodeRef::new(),
            build_script: RwSignal::new(DEFAULT_BUILD_SCRIPT.to_string()),
            ticket: RwSignal::new(0),
            poll_epoch: RwSignal::new(0),
        }
    }

    /// Whether the panel is on screen. Public because the Escape ladder that
    /// owns the key lives in `rooms_workspace`, not here.
    pub fn panel_is_open(&self) -> bool {
        self.panel.get_untracked()
    }

    /// Close the panel and hand focus back to the control that opened it.
    pub fn close_panel(&self) {
        self.panel.set(false);
        if let Some(open) = self.open_ref.get_untracked() {
            let _ = open.focus();
        }
    }

    fn base(&self) -> String {
        self.url.get_untracked().trim_end_matches('/').to_string()
    }

    /// Retire whatever is on screen, in flight, and polling. The epoch bump is
    /// what stops the previous room's poll loop from writing this room's
    /// binding; the ticket bump retires its unfinished reads the same way.
    fn reset(&self) {
        self.ticket
            .update(|ticket| *ticket = ticket.wrapping_add(1));
        self.poll_epoch
            .update(|epoch| *epoch = epoch.wrapping_add(1));
        self.view.set(None);
        self.loading.set(false);
        self.error.set(None);
        self.marker_seen.set(None);
        self.note.set(None);
        self.working.set(None);
        self.panel.set(false);
        self.build_script.set(DEFAULT_BUILD_SCRIPT.to_string());
    }

    /// Read the binding, foreground: the rail shows the read happening. A
    /// read that answers `cloning` also starts the poller — the clone may
    /// have been fired by a session this one replaced, or by another member
    /// entirely, and the panel's "refreshes automatically" promise has to
    /// hold there too, not just where the clone was clicked. Starting fresh
    /// is safe even if a loop were somehow live: `poll_while_cloning` bumps
    /// the epoch, so the old loop retires instead of doubling up.
    fn fetch(&self, rooms: Rooms, key: String, actor: String) {
        let base = self.base();
        let me = *self;
        let ticket = self.ticket.get_untracked().wrapping_add(1);
        self.ticket.set(ticket);
        self.loading.set(true);
        self.error.set(None);
        spawn_local(async move {
            let result = read_status(&base, &key, &actor).await;
            let published = read_is_current(ticket, me.ticket.get_untracked());
            me.publish_status(result, published);
            if published && clone_is_running(me.view.get_untracked().as_ref()) {
                me.poll_while_cloning(rooms, key, actor);
            }
        });
    }

    /// Publish a completed read — but only the latest one.
    fn publish_status(&self, result: Result<RepoView, String>, is_current: bool) {
        if !is_current {
            return;
        }
        self.loading.set(false);
        match result {
            Ok(view) => {
                self.view.set(Some(view));
                let clears = self.error.with_untracked(|slot| {
                    read_success_clears(slot.as_ref().map(|(source, _)| *source))
                });
                if clears {
                    self.error.set(None);
                }
            }
            // A failed read never blanks a standing view: the binding the
            // operator was reading is still the best answer this surface has.
            Err(error) => self.error.set(Some((RepoErrorSource::Read, error))),
        }
    }

    /// Run the clone. The POST is NOT the source of truth for completion —
    /// see the module note — so this also starts the status poller, which
    /// keeps the panel honest even if the long-held response is lost.
    fn clone_repo(&self, rooms: Rooms, key: String, actor: String) {
        let base = self.base();
        let me = *self;
        let generation = rooms.generation_snapshot();
        self.working.set(Some(RepoCommand::Clone));
        self.error.set(None);
        self.note.set(None);
        {
            let key = key.clone();
            let actor = actor.clone();
            spawn_local(async move {
                let url = clone_url(&base, &key, &actor);
                let outcome = post_command(RepoCommand::Clone, &url, &serde_json::json!({})).await;
                me.publish_command(outcome, rooms.room_is_current(generation, &key));
            });
        }
        self.poll_while_cloning(rooms, key, actor);
    }

    /// Run a build. No poller: the outcome only exists in the POST reply (the
    /// binding does not change), and the proxy's command lane now waits out
    /// the daemon's full budget.
    fn build_repo(&self, rooms: Rooms, key: String, actor: String) {
        let script = self.build_script.get_untracked().trim().to_string();
        if let Some(refusal) = script_refusal(&script) {
            self.error.set(Some((RepoErrorSource::Command, refusal)));
            return;
        }
        let base = self.base();
        let me = *self;
        let generation = rooms.generation_snapshot();
        self.working.set(Some(RepoCommand::Build));
        self.error.set(None);
        self.note.set(None);
        spawn_local(async move {
            let url = build_url(&base, &key, &actor);
            let body = serde_json::json!({ "script": script });
            let outcome = post_command(RepoCommand::Build, &url, &body).await;
            me.publish_command(outcome, rooms.room_is_current(generation, &key));
        });
    }

    /// Publish a completed command — but only into the room that started it.
    /// `room_is_current` is the caller's `(generation, key)` re-validation,
    /// taken as an argument so every arm is reachable from a native test.
    fn publish_command(&self, outcome: CommandOutcome, room_is_current: bool) {
        if !room_is_current {
            return;
        }
        self.working.set(None);
        match outcome {
            CommandOutcome::Cloned(repo) => {
                self.note.set(Some(match repo.head_sha.as_deref() {
                    Some(sha) if !sha.is_empty() => format!("Cloned at {}.", short_sha(sha)),
                    _ => "Cloned.".to_string(),
                }));
                self.view.set(Some(RepoView::Bound(*repo)));
            }
            CommandOutcome::Built(report) => self.note.set(Some(build_sentence(&report))),
            CommandOutcome::State(sentence) => self.note.set(Some(sentence)),
            CommandOutcome::Failure(error) => {
                self.error.set(Some((RepoErrorSource::Command, error)))
            }
        }
    }

    /// Watch `clone_status` while a clone is running. Reads silently — no
    /// `loading` flicker — and publishes through the same ticket admission as
    /// every other read, so an overlapping foreground fetch still wins.
    fn poll_while_cloning(&self, rooms: Rooms, key: String, actor: String) {
        let epoch = self.poll_epoch.get_untracked().wrapping_add(1);
        self.poll_epoch.set(epoch);
        let base = self.base();
        let me = *self;
        let generation = rooms.generation_snapshot();
        spawn_local(async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(CLONE_POLL_MS).await;
                if me.poll_epoch.get_untracked() != epoch
                    || !rooms.room_is_current(generation, &key)
                {
                    return;
                }
                let ticket = me.ticket.get_untracked().wrapping_add(1);
                me.ticket.set(ticket);
                let result = read_status(&base, &key, &actor).await;
                me.publish_status(result, read_is_current(ticket, me.ticket.get_untracked()));
                if !poll_should_continue(
                    me.working.get_untracked().is_some(),
                    me.view.get_untracked().as_ref(),
                ) {
                    return;
                }
            }
        });
    }

    /// A clone-outcome marker just landed on the transcript: re-read the
    /// binding now instead of a poll tick later. Silent, and through the
    /// same ticket admission — a stale publish is harmless. The invariant
    /// that a read answering `cloning` demands a poller holds here too, so
    /// the wake can only ever accelerate the poller, never strand a running
    /// clone without one.
    fn refresh_on_marker(&self, rooms: Rooms, key: String, actor: String) {
        let base = self.base();
        let me = *self;
        spawn_local(async move {
            let ticket = me.ticket.get_untracked().wrapping_add(1);
            me.ticket.set(ticket);
            let result = read_status(&base, &key, &actor).await;
            let published = read_is_current(ticket, me.ticket.get_untracked());
            me.publish_status(result, published);
            if published && clone_is_running(me.view.get_untracked().as_ref()) {
                me.poll_while_cloning(rooms, key, actor);
            }
        });
    }
}

/// One status read: transport, decode, classify. A body that does not decode
/// is handed to `classify_status` as `None` — an empty 404 is an ANSWER on
/// this lane (a deployment without the routes), not a fault.
async fn read_status(base: &str, key: &str, actor: &str) -> Result<RepoView, String> {
    let url = repo_url(base, key, actor);
    match Request::get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.json::<RepoBody>().await.ok();
            classify_status(status, body)
        }
        Err(err) => Err(format!("Repo status request failed: {err}")),
    }
}

/// One command POST. The body is exactly what the daemon's strict lane
/// expects: `{}` for clone, `{script}` for build — `actor_member_id` is the
/// daemon's to assert, never this side's.
async fn post_command(
    command: RepoCommand,
    url: &str,
    payload: &serde_json::Value,
) -> CommandOutcome {
    match Request::post(url)
        .header("content-type", "application/json")
        .json(payload)
    {
        Ok(request) => match request.send().await {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.json::<RepoBody>().await.ok();
                classify_command(command, status, body)
            }
            // The work may well continue upstream — Bedrock records the exec
            // either way — so the sentence says so instead of implying the
            // command died with the connection.
            Err(err) => CommandOutcome::Failure(format!(
                "The request was cut ({err}) \u{2014} the command may still be running upstream."
            )),
        },
        Err(err) => CommandOutcome::Failure(format!("Repo request encode error: {err}")),
    }
}

// ---- Component --------------------------------------------------------------

/// The open room's repo binding: a compact rail row, and a panel where the
/// binding is read and clone/build actually run.
///
/// Renders NOTHING for a Local room — no workspace exists there and a refusal
/// would only read as breakage. `writes_allowed` is supplied by the workspace
/// so this control and the composer can never disagree about the same room's
/// access projection; identity is refused at the action, in the composer's
/// words, exactly as `room_summary` and `room_artifacts` do.
#[component]
pub fn RoomRepo(rooms: Rooms, state: RoomRepoState, writes_allowed: Signal<bool>) -> impl IntoView {
    // The (key, actor) this section should be reading, or `None` when it
    // should be dark. A Memo rather than raw signal reads because `access`
    // updates on every roster SSE event; the tuple only changes when the room,
    // its federation, or the resolved identity actually change, so a roster
    // update cannot re-trigger the fetch below.
    let read_target = Memo::new(move |_| {
        let key = rooms.open_key.get().filter(|key| !key.is_empty())?;
        if !room_is_federated(rooms.access.get().as_ref()) {
            return None;
        }
        // Tracked identity reads, deliberately: the workspace lane needs
        // `?actor_id=` on every call including reads, so the first fetch can
        // only go out once bootstrap has answered — and must go out THEN,
        // which an untracked read would never notice.
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
    // binding from being read, however briefly, under this room's name.
    Effect::new(move |_| match read_target.get() {
        Some((key, actor)) => {
            state.reset();
            state.fetch(rooms, key, actor);
        }
        None => state.reset(),
    });

    // The wake path: a clone finishing anywhere — another member, another
    // session — lands on the transcript as a System marker, so watching the
    // SSE-fed transcript closes the gap between "the room heard" and "this
    // panel shows it". The watermark starts over with reset() AND carries
    // the room generation, so hydration reads as the initial load, never as
    // news, whichever Effect runs first on a room switch.
    Effect::new(move |_| {
        let (watermark, wake) = rooms.transcript.with(|transcript| {
            marker_wake(
                state.marker_seen.get_untracked(),
                rooms.generation_snapshot(),
                transcript,
                is_repo_clone_marker,
            )
        });
        state.marker_seen.set(watermark);
        if !wake {
            return;
        }
        let Some((key, actor)) = read_target.get_untracked() else {
            return;
        };
        state.refresh_on_marker(rooms, key, actor);
    });

    let can_run = move || {
        writes_allowed.get()
            && state.working.get().is_none()
            && rooms.open_key.get().is_some_and(|key| !key.is_empty())
    };

    // The one place an action resolves the room key and the actor together.
    // The identity refusal is the composer's, in the composer's words.
    let actor = move || -> Option<(String, String)> {
        let key = rooms
            .open_key
            .get_untracked()
            .filter(|key| !key.is_empty())?;
        if !rooms.identity_resolved() {
            state.error.set(Some((
                RepoErrorSource::Command,
                "Still signing in \u{2014} try again in a moment.".to_string(),
            )));
            return None;
        }
        Some((key, rooms.identity_id.get_untracked()))
    };

    // The whole section, gated: a Local room has no workspace, and a daemon
    // that answered `not_federated` is the same answer. A Memo, NOT raw reads
    // in the section closure below: `access` notifies on every roster SSE
    // update and `view` on every poll publish, and a section rebuilt by
    // either would tear down the open panel — the exact mid-edit teardown the
    // state struct exists to prevent. The memo flips only when visibility
    // actually changes.
    let visible = Memo::new(move |_| {
        room_is_federated(rooms.access.get().as_ref())
            && !matches!(state.view.get(), Some(RepoView::NotFederated))
    });

    view! {
        {move || {
            if !visible.get() {
                return ().into_any();
            }
            view! {
                <div class="rooms-workspace__repo">
                    <div class="rooms-workspace__repo-head">
                        <span class="rooms-workspace__repo-title">"Repo"</span>
                        <button
                            class="rooms-workspace__repo-open"
                            type="button"
                            node_ref=state.open_ref
                            title="Open this room's repo binding"
                            disabled=move || {
                                !matches!(
                                    state.view.get(),
                                    Some(RepoView::Bound(_) | RepoView::Unbound)
                                )
                            }
                            on:click=move |_| {
                                state.error.set(None);
                                state.panel.set(true);
                            }
                        >
                            "open"
                        </button>
                    </div>

                    // Rendered in the rail AND the panel, like the artifacts
                    // error: a failure while the panel is closed must not
                    // read as a room without a repo.
                    {move || {
                        state.error.get().map(|(_, error)| view! {
                            <div class="rooms-workspace__repo-error" role="alert">{error}</div>
                        })
                    }}

                    {move || {
                        if state.loading.get() && state.view.get().is_none() {
                            return view! {
                                <div class="rooms-workspace__repo-note">
                                    "Checking repo\u{2026}"
                                </div>
                            }.into_any();
                        }
                        match state.view.get() {
                            Some(RepoView::Unavailable) => view! {
                                <div class="rooms-workspace__repo-note">
                                    "Repo binding isn't available on this deployment yet."
                                </div>
                            }.into_any(),
                            Some(view_state) => rail_line(&view_state)
                                .map(|line| view! {
                                    <div class="rooms-workspace__repo-line">{line}</div>
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
                                class="rooms-workspace__repo-scrim"
                                on:click=move |_| state.close_panel()
                            ></div>
                            <div
                                class="rooms-workspace__repo-panel"
                                role="dialog"
                                aria-modal="true"
                                aria-label="Room repo"
                            >
                                <div class="rooms-workspace__repo-panel-head">
                                    <span class="rooms-workspace__repo-panel-title">"Repo"</span>
                                    <button
                                        class="rooms-workspace__repo-close"
                                        type="button"
                                        aria-label="Close repo"
                                        on:click=move |_| state.close_panel()
                                    >
                                        "\u{d7}"
                                    </button>
                                </div>
                                <div class="rooms-workspace__repo-panel-body">
                                    {move || {
                                        state.error.get().map(|(_, error)| view! {
                                            <div class="rooms-workspace__repo-error" role="alert">
                                                {error}
                                            </div>
                                        })
                                    }}
                                    {move || {
                                        state.note.get().map(|note| view! {
                                            <div class="rooms-workspace__repo-note">{note}</div>
                                        })
                                    }}
                                    {move || match state.view.get() {
                                        Some(RepoView::Bound(repo)) => {
                                            panel_bound(state, actor, rooms, can_run, repo)
                                                .into_any()
                                        }
                                        Some(RepoView::Unbound) => view! {
                                            <div class="rooms-workspace__repo-note">
                                                "No repo is bound to this room yet."
                                            </div>
                                        }.into_any(),
                                        _ => ().into_any(),
                                    }}
                                    // The honest boundary, stated where the
                                    // missing controls would otherwise be.
                                    <div class="rooms-workspace__repo-footnote">
                                        "Binding or unbinding the repo is an owner act, \
                                         done over the Bedrock API for now."
                                    </div>
                                </div>
                            </div>
                        }.into_any()
                    }}
                </div>
            }.into_any()
        }}
    }
}

/// The panel for a standing binding: the facts, and the two member acts.
fn panel_bound(
    state: RoomRepoState,
    actor: impl Fn() -> Option<(String, String)> + Copy + Send + Sync + 'static,
    rooms: Rooms,
    can_run: impl Fn() -> bool + Copy + Send + Sync + 'static,
    repo: RepoProjection,
) -> impl IntoView {
    let cloned = repo.clone_status == "cloned";
    let cloning = repo.clone_status == "cloning";
    let status_line = match repo.head_sha.as_deref().filter(|sha| !sha.is_empty()) {
        Some(sha) if cloned => format!("{} @ {}", repo.clone_status, short_sha(sha)),
        _ => repo.clone_status.clone(),
    };
    let clone_label = move || match state.working.get() {
        Some(RepoCommand::Clone) => "cloning\u{2026}",
        _ if cloned => "re-clone",
        _ => "clone",
    };

    view! {
        <div class="rooms-workspace__repo-facts">
            <span class="rooms-workspace__repo-fact-label">"remote"</span>
            <span class="rooms-workspace__repo-fact-value">{repo.remote.clone()}</span>
            <span class="rooms-workspace__repo-fact-label">"branch"</span>
            <span class="rooms-workspace__repo-fact-value">{repo.branch.clone()}</span>
            <span class="rooms-workspace__repo-fact-label">"status"</span>
            <span class="rooms-workspace__repo-fact-value">{status_line}</span>
            {repo.last_cloned_at.clone().filter(|at| !at.is_empty()).map(|at| view! {
                <span class="rooms-workspace__repo-fact-label">"cloned"</span>
                <span class="rooms-workspace__repo-fact-value">{at}</span>
            })}
        </div>

        // Owner-only on the wire, so its presence is already permissioned;
        // when it is here, it is the reason the status says `failed`.
        {repo.clone_error.clone().filter(|error| !error.is_empty()).map(|error| view! {
            <div class="rooms-workspace__repo-error" role="alert">{error}</div>
        })}

        {cloning.then(|| view! {
            <div class="rooms-workspace__repo-note">
                "A clone is running \u{2014} status refreshes automatically."
            </div>
        })}

        <div class="rooms-workspace__repo-actions">
            <button
                class="rooms-workspace__repo-run"
                type="button"
                title="Clone the bound repo into this room's workspace"
                disabled=move || !can_run()
                on:click=move |_| {
                    let Some((key, actor_id)) = actor() else { return };
                    state.clone_repo(rooms, key, actor_id);
                }
            >
                {clone_label}
            </button>
        </div>

        {cloned.then(|| view! {
            <div class="rooms-workspace__repo-actions">
                <input
                    class="rooms-workspace__repo-input"
                    type="text"
                    aria-label="Package script to build"
                    prop:value=move || state.build_script.get()
                    on:input=move |ev| state.build_script.set(event_target_value(&ev))
                />
                <button
                    class="rooms-workspace__repo-run"
                    type="button"
                    title="Run this package script in the room's workspace"
                    disabled=move || !can_run()
                    on:click=move |_| {
                        let Some((key, actor_id)) = actor() else { return };
                        state.build_repo(rooms, key, actor_id);
                    }
                >
                    {move || {
                        if state.working.get() == Some(RepoCommand::Build) {
                            "building\u{2026}"
                        } else {
                            "build"
                        }
                    }}
                </button>
            </div>
        })}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state as `new` leaves it, for the tests that drive one directly.
    fn fresh_state() -> RoomRepoState {
        RoomRepoState {
            url: RwSignal::new("http://d".to_string()),
            view: RwSignal::new(None),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            marker_seen: RwSignal::new(None),
            note: RwSignal::new(None),
            working: RwSignal::new(None),
            panel: RwSignal::new(false),
            open_ref: NodeRef::new(),
            build_script: RwSignal::new(DEFAULT_BUILD_SCRIPT.to_string()),
            ticket: RwSignal::new(0),
            poll_epoch: RwSignal::new(0),
        }
    }

    fn body(json: &str) -> RepoBody {
        serde_json::from_str(json).unwrap()
    }

    /// Bedrock's `publicRepoProjection`, field for field, as the daemon
    /// relays it (`{repo: {...}}`, no `ok` envelope).
    fn bound_json() -> &'static str {
        r#"{"repo": {
            "room_id": "room-1",
            "remote": "https://github.com/acme/site.git",
            "branch": "main",
            "dir": "site",
            "workspace_path": "/workspace/site",
            "clone_status": "cloned",
            "credential_secret": "GIT_TOKEN",
            "head_sha": "0123456789abcdef",
            "last_cloned_at": "2026-08-27T10:00:00.000Z",
            "bound_at": "2026-08-20T09:00:00.000Z",
            "updated_at": "2026-08-27T10:00:00.000Z"
        }}"#
    }

    #[test]
    fn a_standing_binding_is_bound() {
        let view = classify_status(200, Some(body(bound_json()))).unwrap();
        let RepoView::Bound(repo) = view else {
            panic!("expected Bound, got {view:?}");
        };
        assert_eq!(repo.remote, "https://github.com/acme/site.git");
        assert_eq!(repo.clone_status, "cloned");
        assert_eq!(repo.head_sha.as_deref(), Some("0123456789abcdef"));
        // Non-owner projection: clone_error simply absent, never an error.
        assert_eq!(repo.clone_error, None);
    }

    /// Bedrock's unbound answer carries a TOP-LEVEL code (its 404 body is
    /// written directly, not through the HttpError serializer).
    #[test]
    fn repo_unbound_is_an_answer_not_an_error() {
        let unbound =
            body(r#"{"error": "This room has no repo bound to it.", "code": "repo_unbound"}"#);
        assert_eq!(classify_status(404, Some(unbound)), Ok(RepoView::Unbound));
    }

    /// The daemon's own gate refusal for a Local room. The section is hidden
    /// by the access projection first; this keeps the classification total.
    #[test]
    fn not_federated_is_recognized() {
        let gated = body(
            r#"{"ok": false, "code": "room_not_federated",
                "error": "this room has no Bedrock credential, so it has no workspace"}"#,
        );
        assert_eq!(
            classify_status(409, Some(gated)),
            Ok(RepoView::NotFederated)
        );
    }

    /// A deployment that predates the lane answers 404 with an empty or
    /// unrecognizable body — the daemon's unknown route, or Bedrock's. That
    /// is an ANSWER ("not available yet"), never a failure.
    #[test]
    fn a_route_less_deployment_reads_as_unavailable() {
        assert_eq!(classify_status(404, None), Ok(RepoView::Unavailable));
        let plain = body(r#"{"ok": false, "error": "Not found"}"#);
        assert_eq!(classify_status(404, Some(plain)), Ok(RepoView::Unavailable));
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

    /// Bedrock's THROWN refusals nest the code under `details` — its error
    /// writer serializes `HttpError.details` whole. Both homes are read.
    #[test]
    fn a_details_nested_code_is_recognized() {
        let busy = body(
            r#"{"ok": false, "error": "A build is already running in this room.",
                "details": {"code": "build_running"}}"#,
        );
        let outcome = classify_command(RepoCommand::Clone, 409, Some(busy));
        let CommandOutcome::State(sentence) = outcome else {
            panic!("expected State, got {outcome:?}");
        };
        assert!(
            sentence.contains("build is already running"),
            "got: {sentence}"
        );
    }

    /// The clone success body: `{repo, exec, head_sha}` — the binding as it
    /// now stands, no follow-up read needed.
    #[test]
    fn a_finished_clone_carries_the_new_binding() {
        let done = body(
            r#"{"repo": {"remote": "https://github.com/acme/site.git", "branch": "main",
                         "clone_status": "cloned", "head_sha": "fedcba9876543210"},
                "exec": {"id": "exec-1"}, "head_sha": "fedcba9876543210"}"#,
        );
        let outcome = classify_command(RepoCommand::Clone, 200, Some(done));
        let CommandOutcome::Cloned(repo) = outcome else {
            panic!("expected Cloned, got {outcome:?}");
        };
        assert_eq!(repo.clone_status, "cloned");
    }

    /// The 502's stderr tail is the one part of a failed clone an operator
    /// can act on; it must survive into the sentence.
    #[test]
    fn a_failed_clone_shows_the_git_stderr() {
        let failed = body(
            r#"{"ok": false, "error": "The repo clone failed.",
                "details": {"code": "repo_clone_failed", "stderr": "fatal: could not read Username"}}"#,
        );
        let outcome = classify_command(RepoCommand::Clone, 502, Some(failed));
        let CommandOutcome::Failure(sentence) = outcome else {
            panic!("expected Failure, got {outcome:?}");
        };
        assert!(
            sentence.contains("could not read Username"),
            "got: {sentence}"
        );
    }

    /// A build that ran and exited nonzero is 200 with the outcome in the
    /// body — Bedrock's deliberate design, honored here: a `Built`, never a
    /// failure.
    #[test]
    fn a_build_that_exited_nonzero_is_still_an_answer() {
        let ran = body(
            r#"{"build": {"script": "test", "outcome": "failed", "exit_code": 1,
                          "duration_ms": 12400, "repo_dir": "site"},
                "exec": {"id": "exec-2"}, "stdout": "", "stderr": "1 failing", "truncated": false}"#,
        );
        let outcome = classify_command(RepoCommand::Build, 200, Some(ran));
        let CommandOutcome::Built(report) = outcome else {
            panic!("expected Built, got {outcome:?}");
        };
        let sentence = build_sentence(&report);
        assert!(sentence.contains("exited 1"), "got: {sentence}");
        assert!(sentence.contains("13s"), "got: {sentence}");
    }

    #[test]
    fn a_build_before_the_clone_is_a_state() {
        let unready = body(
            r#"{"ok": false, "error": "This room's repo has not been cloned into the workspace.",
                "details": {"code": "repo_not_cloned", "clone_status": "pending"}}"#,
        );
        let outcome = classify_command(RepoCommand::Build, 409, Some(unready));
        let CommandOutcome::State(sentence) = outcome else {
            panic!("expected State, got {outcome:?}");
        };
        assert!(sentence.contains("clone it first"), "got: {sentence}");
    }

    #[test]
    fn a_daemon_gate_refusal_is_a_failure_in_words() {
        let gated = body(
            r#"{"ok": false, "code": "not_a_room_member",
                "error": "the asserted actor is not on this room's roster"}"#,
        );
        let outcome = classify_command(RepoCommand::Clone, 403, Some(gated));
        assert_eq!(
            outcome,
            CommandOutcome::Failure("You're not on this room's roster.".to_string())
        );
    }

    // ---- publish admission --------------------------------------------------

    #[test]
    fn a_stale_read_publishes_nothing() {
        let state = fresh_state();
        state.loading.set(true);
        state.publish_status(Ok(RepoView::Unbound), false);
        assert!(state.loading.get_untracked());
        assert_eq!(state.view.get_untracked(), None);
    }

    /// A failed refresh must not blank the binding the operator is reading —
    /// the standing view outranks a transient read error.
    #[test]
    fn a_failed_read_keeps_the_standing_view() {
        let state = fresh_state();
        state.view.set(Some(RepoView::Unbound));
        state.publish_status(Err("boom".to_string()), true);
        assert_eq!(state.view.get_untracked(), Some(RepoView::Unbound));
        assert_eq!(
            state.error.get_untracked(),
            Some((RepoErrorSource::Read, "boom".to_string()))
        );
    }

    /// The absorbed fix: a read that recovers clears the error a read set —
    /// one blipped silent poll no longer leaves an alert standing over a
    /// healthy view — and NEVER a command's, which the operator has not
    /// acted on yet.
    #[test]
    fn a_read_success_clears_only_a_read_error() {
        let state = fresh_state();
        state.publish_status(Err("net blip".to_string()), true);
        state.publish_status(Ok(RepoView::Unbound), true);
        assert_eq!(state.error.get_untracked(), None);

        state.working.set(Some(RepoCommand::Clone));
        state.publish_command(
            CommandOutcome::Failure("The clone failed.".to_string()),
            true,
        );
        state.publish_status(Ok(RepoView::Unbound), true);
        assert_eq!(
            state.error.get_untracked(),
            Some((RepoErrorSource::Command, "The clone failed.".to_string())),
            "a background read success must not clear a command failure"
        );

        assert!(read_success_clears(Some(RepoErrorSource::Read)));
        assert!(!read_success_clears(Some(RepoErrorSource::Command)));
        assert!(!read_success_clears(None));
    }

    #[test]
    fn a_command_for_a_departed_room_publishes_nothing() {
        let state = fresh_state();
        state.working.set(Some(RepoCommand::Build));
        state.publish_command(CommandOutcome::Failure("late".to_string()), false);
        // `reset` cleared this state for whoever is on screen now; the late
        // completion must not re-disturb it.
        assert_eq!(state.working.get_untracked(), Some(RepoCommand::Build));
        assert_eq!(state.error.get_untracked(), None);
    }

    /// A typed state lands in `note`, a fault in `error`: the panel renders
    /// them in different voices because only one of them is a problem.
    #[test]
    fn states_and_failures_land_in_different_signals() {
        let state = fresh_state();
        state.working.set(Some(RepoCommand::Clone));
        state.publish_command(CommandOutcome::State("busy".to_string()), true);
        assert_eq!(state.working.get_untracked(), None);
        assert_eq!(state.note.get_untracked().as_deref(), Some("busy"));
        assert_eq!(state.error.get_untracked(), None);

        state.publish_command(CommandOutcome::Failure("broke".to_string()), true);
        assert_eq!(
            state.error.get_untracked(),
            Some((RepoErrorSource::Command, "broke".to_string()))
        );
    }

    #[test]
    fn a_finished_clone_updates_the_view_and_says_the_sha() {
        let state = fresh_state();
        state.working.set(Some(RepoCommand::Clone));
        let repo = RepoProjection {
            remote: "https://github.com/acme/site.git".into(),
            branch: "main".into(),
            clone_status: "cloned".into(),
            head_sha: Some("0123456789abcdef".into()),
            last_cloned_at: None,
            clone_error: None,
        };
        state.publish_command(CommandOutcome::Cloned(Box::new(repo.clone())), true);
        assert_eq!(state.view.get_untracked(), Some(RepoView::Bound(repo)));
        assert_eq!(
            state.note.get_untracked().as_deref(),
            Some("Cloned at 0123456789.")
        );
    }

    // ---- poller -------------------------------------------------------------

    /// A poller must exist in ANY session that observes a running clone — a
    /// reload mid-clone, a second member watching — not just the one that
    /// clicked clone. `fetch` starts one off this predicate; pin both
    /// directions so the build controls can't silently vanish behind a
    /// permanently stale "cloning" view.
    #[test]
    fn a_read_that_answers_cloning_demands_a_poller() {
        let bound = |status: &str| {
            RepoView::Bound(RepoProjection {
                remote: String::new(),
                branch: String::new(),
                clone_status: status.into(),
                head_sha: None,
                last_cloned_at: None,
                clone_error: None,
            })
        };
        assert!(clone_is_running(Some(&bound("cloning"))));
        assert!(!clone_is_running(Some(&bound("cloned"))));
        assert!(!clone_is_running(Some(&bound("failed"))));
        assert!(!clone_is_running(Some(&RepoView::Unbound)));
        assert!(!clone_is_running(None));
    }

    #[test]
    fn the_poller_runs_while_the_command_or_the_clone_does() {
        let cloning = RepoView::Bound(RepoProjection {
            remote: String::new(),
            branch: String::new(),
            clone_status: "cloning".into(),
            head_sha: None,
            last_cloned_at: None,
            clone_error: None,
        });
        let cloned = RepoView::Bound(RepoProjection {
            remote: String::new(),
            branch: String::new(),
            clone_status: "cloned".into(),
            head_sha: None,
            last_cloned_at: None,
            clone_error: None,
        });
        // Our POST is still in flight: keep watching whatever the view says.
        assert!(poll_should_continue(true, None));
        assert!(poll_should_continue(true, Some(&cloned)));
        // POST answered (or was lost): the binding's own status decides.
        assert!(poll_should_continue(false, Some(&cloning)));
        assert!(!poll_should_continue(false, Some(&cloned)));
        assert!(!poll_should_continue(false, Some(&RepoView::Unbound)));
        assert!(!poll_should_continue(false, None));
    }

    // ---- presentation -------------------------------------------------------

    #[test]
    fn the_remote_reads_as_owner_slash_repo() {
        assert_eq!(
            remote_label("https://github.com/acme/site.git"),
            "acme/site"
        );
        assert_eq!(remote_label("git@github.com:acme/site.git"), "acme/site");
        assert_eq!(remote_label("https://github.com/acme/site"), "acme/site");
        // Something unrecognizable passes through rather than vanishing.
        assert_eq!(remote_label(""), "");
    }

    #[test]
    fn the_rail_line_says_what_stands() {
        let bound = RepoView::Bound(RepoProjection {
            remote: "https://github.com/acme/site.git".into(),
            branch: "main".into(),
            clone_status: "cloned".into(),
            head_sha: Some("0123456789abcdef".into()),
            last_cloned_at: None,
            clone_error: None,
        });
        assert_eq!(
            rail_line(&bound).as_deref(),
            Some("acme/site \u{b7} cloned @ 0123456789")
        );
        assert_eq!(
            rail_line(&RepoView::Unbound).as_deref(),
            Some("No repo bound.")
        );
        assert_eq!(rail_line(&RepoView::Unavailable), None);
    }

    /// A failed clone shows its status without pretending a sha it has not
    /// got; a pending one shows neither.
    #[test]
    fn the_rail_line_never_invents_a_sha() {
        let failed = RepoView::Bound(RepoProjection {
            remote: "https://github.com/acme/site.git".into(),
            branch: "main".into(),
            clone_status: "failed".into(),
            head_sha: Some("0123456789abcdef".into()),
            last_cloned_at: None,
            clone_error: None,
        });
        assert_eq!(
            rail_line(&failed).as_deref(),
            Some("acme/site \u{b7} failed")
        );
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
        assert!(room_is_federated(Some(&projection(
            RoomAccessState::Connecting
        ))));
        assert!(room_is_federated(Some(&projection(
            RoomAccessState::Revoked
        ))));
    }

    #[test]
    fn escape_closes_only_an_open_unclaimed_panel() {
        assert!(repo_escape_closes(true, false));
        assert!(!repo_escape_closes(false, false));
        assert!(!repo_escape_closes(true, true));
    }

    #[test]
    fn urls_assert_the_actor_and_encode_both_segments() {
        assert_eq!(
            repo_url("http://d", "team room", "user@host"),
            "http://d/v1/rooms/persistent/team%20room/workspace/repo?actor_id=user%40host"
        );
        assert!(clone_url("http://d", "k", "a").ends_with("/workspace/repo/clone?actor_id=a"));
        assert!(build_url("http://d", "k", "a").ends_with("/workspace/repo/build?actor_id=a"));
    }

    #[test]
    fn an_empty_script_is_refused_before_the_wire() {
        assert!(script_refusal("  ").is_some());
        assert!(script_refusal("").is_some());
        assert!(script_refusal("build").is_none());
    }

    #[test]
    fn clipping_is_char_safe() {
        assert_eq!(clip("abc", 5), "abc");
        assert_eq!(clip("abcdef", 3), "abc\u{2026}");
        // A multi-byte boundary must not split.
        assert_eq!(clip("é é é", 3), "é é\u{2026}");
    }

    // ---- the marker wake ----------------------------------------------------

    fn system_row(seq: u64, body: &str) -> RoomMessage {
        RoomMessage {
            seq,
            author_id: "system".into(),
            author_kind: crate::rooms::RoomParticipantKind::System,
            kind: crate::rooms::RoomMessageKind::System,
            body: body.into(),
            created_at: String::new(),
            federated: None,
            thread_parent_seq: None,
            attachment_id: None,
        }
    }

    /// Only a clone outcome wakes the binding read: both outcomes match,
    /// the other seven marker variants — which say nothing about the
    /// binding — do not, and neither does the initial load.
    #[test]
    fn only_a_clone_outcome_marker_wakes_the_binding_read() {
        assert!(is_repo_clone_marker(&system_row(
            1,
            "workspace repo cloned: 'main' @ 0123456789ab"
        )));
        assert!(is_repo_clone_marker(&system_row(
            2,
            "workspace repo clone failed: 'main' (exit 128)"
        )));
        assert!(!is_repo_clone_marker(&system_row(
            3,
            "workspace build 'build' succeeded (3.2s)"
        )));
        assert!(!is_repo_clone_marker(&system_row(4, "workspace flushed")));

        // Initial sight of a marker-laden history records, never wakes.
        let history = vec![system_row(1, "workspace repo cloned: 'main'")];
        let (watermark, wake) = marker_wake(None, 3, &history, is_repo_clone_marker);
        assert_eq!(watermark, Some((3, 1)));
        assert!(!wake);

        // A live clone outcome wakes; a build marker after it does not.
        let mut transcript = history;
        transcript.push(system_row(
            2,
            "workspace repo clone failed: 'main' (exit 1)",
        ));
        let (watermark, wake) = marker_wake(watermark, 3, &transcript, is_repo_clone_marker);
        assert_eq!(watermark, Some((3, 2)));
        assert!(wake);
        transcript.push(system_row(3, "workspace build 'build' succeeded (3.2s)"));
        let (_, wake) = marker_wake(watermark, 3, &transcript, is_repo_clone_marker);
        assert!(!wake);
    }
}
