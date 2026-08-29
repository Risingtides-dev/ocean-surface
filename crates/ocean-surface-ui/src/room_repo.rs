//! The room's bound repo — the browser half of the daemon's workspace lane.
//!
//! A federated room can have a git repo bound to its Bedrock workspace, cloned
//! into its container, and built there. The daemon exposes exactly what a room
//! MEMBER may do with that binding (`room_workspace_proxy.rs`'s allowlist):
//!
//!   GET  /v1/rooms/persistent/{key}/workspace/repo         → the binding
//!   POST /v1/rooms/persistent/{key}/workspace/repo/clone   → clone it
//!   POST /v1/rooms/persistent/{key}/workspace/repo/build   → run a script
//!   GET  /v1/rooms/persistent/{key}/workspace/repo/ci      → recorded CI state
//!   POST /v1/rooms/persistent/{key}/workspace/repo/ci      → pull CI via gh
//!
//! Choosing WHAT the room builds — bind and unbind — was deliberately absent
//! until the daemon grew an identity map (ocean-os#390): its POST leaves
//! `repo/bind` and `repo/unbind` become Bedrock's PUT and DELETE, forwarded
//! only when the asserted actor resolves to the credential's own principal,
//! and whether that principal owns the room stays Bedrock's call. This panel
//! cannot run that map, so the controls render for every member and the
//! typed refusal answers — never a client-side guess at authority.
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
//!    A bind sends exactly the keys `validateRepoBinding` admits — `remote`,
//!    and `branch`/`dir` only when the operator gave one, so the upstream
//!    defaults stay upstream's. An unbind posts `{}` too: the daemon's lane
//!    demands a JSON object even though Bedrock's DELETE reads none. And an
//!    unbind DELETES THE CHECKOUT with the binding, so its control confirms
//!    once, and the reply's `checkout_removed` is surfaced honestly.
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
//! CI rides the same lane with one more property: a pull's reply is ALWAYS
//! HTTP 200 once gh ran — a nonzero exit (unauthenticated gh, a non-GitHub
//! remote, a rate limit) is outcome `failed` with gh's stderr guidance as
//! THE answer, rendered as a legible failed state, never a transport fault.
//! The recorded read is served from Bedrock's dedupe table with no container
//! run, so CI state is readable on open even while the container is absent,
//! and another member's pull reaches this panel as a "workspace CI" marker
//! on the same transcript tail the clone markers ride.
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

/// The opening of the CI marker ("workspace CI on 'main': 2 new results…").
/// Another member's pull lands new rows in Bedrock's table; this marker is
/// how the recorded view here hears it moved.
const REPO_CI_MARKER_PREFIX: &str = "workspace CI";

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

/// The `ci` object of a pull reply. Always under HTTP 200 once gh ran —
/// outcome `failed` or `timed_out` is gh's own report, not a fault.
/// `message` carries Bedrock's projection refusal (`ci_output_rejected`
/// family) when gh exited 0 but the answer could not be vouched for.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CiReport {
    #[serde(default)]
    pub outcome: String,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub checks_total: Option<u64>,
    #[serde(default)]
    pub checks_new: Option<u64>,
    #[serde(default)]
    pub message: Option<String>,
}

/// One CI check. The identity pair is Bedrock's only promise; every
/// descriptive field is lenient because the two reply shapes differ — a
/// pull's rows carry `new`, the recorded read's rows carry `first_seen_at`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CiCheck {
    #[serde(default)]
    pub check_run_id: String,
    #[serde(default)]
    pub head_sha: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub conclusion: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub first_seen_at: Option<String>,
    #[serde(default)]
    pub new: bool,
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
    ci: Option<CiReport>,
    /// A pull reply's full current list, and the recorded read's whole
    /// answer — presence here is what a `GET repo/ci` success means.
    #[serde(default)]
    checks: Option<Vec<CiCheck>>,
    /// Top-level on a pull reply: gh's guidance when the outcome is a
    /// failure. Distinct from the `details.stderr` of a thrown 502.
    #[serde(default)]
    stderr: Option<String>,
    /// The unbind reply (`handleRepoUnbind`): presence of `unbound` is what
    /// its success means, and `checkout_removed` says whether the checkout
    /// actually left the container — false comes with a reason.
    #[serde(default)]
    unbound: Option<bool>,
    #[serde(default)]
    checkout_removed: Option<bool>,
    #[serde(default)]
    checkout_removed_reason: Option<String>,
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

/// One URL for both CI verbs: GET reads the recorded state, POST pulls. The
/// daemon forwards only `limit` upstream on the read and this side never
/// passes one — Bedrock's default is plenty for a panel.
fn ci_url(base: &str, key: &str, actor: &str) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/workspace/repo/ci?actor_id={}",
        encode(key),
        encode(actor),
    )
}

/// The owner verbs ride daemon-side POST leaves — the daemon maps them to
/// Bedrock's PUT and DELETE itself, because the CORS preflight only admits
/// the methods the lane advertises.
fn bind_url(base: &str, key: &str, actor: &str) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/workspace/repo/bind?actor_id={}",
        encode(key),
        encode(actor),
    )
}

fn unbind_url(base: &str, key: &str, actor: &str) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/workspace/repo/unbind?actor_id={}",
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

/// The commands a member can run. One at a time — Bedrock holds a
/// mutual-exclusion lock over the checkout for clone and build, and while a
/// CI pull takes no claim upstream, one submit at a time keeps this panel's
/// answer legible: `note` and `error` hold one command's outcome, not a race.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepoCommand {
    Clone,
    Build,
    Ci,
    Bind,
    Unbind,
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
    /// The CI pull ran and gh answered cleanly: the report, and the full
    /// current list (with `new` flags) that replaces the recorded view
    /// wholesale. A gh that ran and FAILED is a `Failure` carrying its
    /// stderr guidance — that guidance is the answer, not a fault.
    Checked(CiReport, Vec<CiCheck>),
    /// The bind landed; the reply carries the fresh projection. The view is
    /// NOT set from it — the caller re-reads through `GET repo`, so what the
    /// panel renders is always what a read would answer.
    Bound(Box<RepoProjection>),
    /// The unbind landed, and the reply says truthfully whether the checkout
    /// left the container with it.
    Unbound {
        checkout_removed: bool,
        reason: Option<String>,
    },
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
        "repo_unbound" => "No repo is bound to this room yet.",
        // The daemon's owner gate answering a non-principal actor. For the
        // people it refuses this is how the room is shaped, not a fault.
        "workspace_not_owner_principal" => "Only the room owner can change the repo binding.",
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
        "workspace_actor_unmapped" => "Your identity doesn't map to this room's compute service.",
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

/// What the recorded-CI read answered. Kept apart from `RepoView` because
/// the table outlives the container: the binding can be mid-churn while the
/// recorded checks still read fine, and a daemon can serve the repo lane
/// while predating this one.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ChecksView {
    /// The recorded rows, as Bedrock's table serves them.
    Recorded(Vec<CiCheck>),
    /// This deployment does not serve the read. Quiet — the panel around it
    /// still works, and the pull control answers honestly at the click.
    Unavailable,
    /// The read failed and nothing recorded stands to show instead.
    Unread(String),
}

/// Map a `GET repo/ci` reply onto the recorded view. Same 404 posture as
/// `classify_status`: a deployment that predates the lane is an answer, not
/// a fault.
fn classify_checks(status: u16, body: Option<RepoBody>) -> Result<ChecksView, String> {
    let Some(body) = body else {
        if status == 404 {
            return Ok(ChecksView::Unavailable);
        }
        return Err(format!(
            "The recorded CI reply could not be read ({status})."
        ));
    };
    if let Some(checks) = body.checks {
        return Ok(ChecksView::Recorded(checks));
    }
    match body.refusal_code() {
        Some("workspace_route_not_allowed") => Ok(ChecksView::Unavailable),
        Some(code) => Err(failure_sentence(code)
            .or_else(|| state_sentence(code))
            .or_else(|| body.error.clone())
            .unwrap_or_else(|| format!("Reading recorded CI failed ({status})."))),
        None if status == 404 => Ok(ChecksView::Unavailable),
        None => Err(body
            .error
            .filter(|error| !error.is_empty())
            .map(|error| format!("Reading recorded CI failed: {error}"))
            .unwrap_or_else(|| format!("Reading recorded CI failed ({status})."))),
    }
}

/// Map a command reply onto what the panel should show.
fn classify_command(command: RepoCommand, status: u16, body: Option<RepoBody>) -> CommandOutcome {
    let noun = match command {
        RepoCommand::Clone => "clone",
        RepoCommand::Build => "build",
        RepoCommand::Ci => "CI check",
        RepoCommand::Bind => "bind",
        RepoCommand::Unbind => "unbind",
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
        RepoCommand::Ci => {
            if let Some(ci) = body.ci {
                if ci.outcome == "checked" {
                    return CommandOutcome::Checked(ci, body.checks.unwrap_or_default());
                }
                // gh ran and reported — 200 with a failed outcome, and its
                // stderr guidance is the whole answer.
                return CommandOutcome::Failure(ci_failure_sentence(&ci, body.stderr.as_deref()));
            }
        }
        RepoCommand::Bind => {
            if let Some(repo) = body.repo {
                return CommandOutcome::Bound(Box::new(repo));
            }
        }
        RepoCommand::Unbind => {
            if body.unbound == Some(true) {
                return CommandOutcome::Unbound {
                    checkout_removed: body.checkout_removed == Some(true),
                    reason: body.checkout_removed_reason,
                };
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

/// The sentence a landed bind earns. The clone hint is the next act: a fresh
/// binding is `pending` until someone clones it in.
fn bound_sentence(repo: &RepoProjection) -> String {
    let label = remote_label(&repo.remote);
    if label.is_empty() {
        "Repo bound \u{2014} clone it to bring it into the workspace.".to_string()
    } else {
        format!("Bound {label} \u{2014} clone it to bring it into the workspace.")
    }
}

/// The sentence a landed unbind earns — honest about the checkout, because
/// Bedrock deletes it with the binding and reports whether that worked. A
/// checkout it could not remove stops being excluded from the flush, so the
/// leftover matters enough to say.
fn unbind_sentence(checkout_removed: bool, reason: Option<&str>) -> String {
    if checkout_removed {
        return "Repo unbound \u{2014} the workspace checkout was removed.".to_string();
    }
    match reason {
        Some("no_container") => {
            "Repo unbound. No container was live, so there was no checkout to remove.".to_string()
        }
        Some("rm_failed") => {
            "Repo unbound, but the checkout could not be removed \u{2014} its files will flush \
             as ordinary room files."
                .to_string()
        }
        _ => "Repo unbound.".to_string(),
    }
}

/// The sentence a clean CI pull earns. The counts are the news; the
/// recorded list below the note carries the conclusions.
fn ci_sentence(report: &CiReport) -> String {
    let took = report
        .duration_ms
        .map(|ms| format!(" in {}s", ms.div_ceil(1000)))
        .unwrap_or_default();
    match (report.checks_new, report.checks_total) {
        (Some(0), Some(total)) => {
            format!("CI checked{took} \u{2014} no new results ({total} recorded).")
        }
        (Some(new), Some(total)) => {
            let noun = if new == 1 { "result" } else { "results" };
            format!("CI checked{took}: {new} new {noun} ({total} total).")
        }
        _ => format!("CI checked{took}."),
    }
}

/// A failed or timed-out pull, with gh's stderr as the detail — an
/// unauthenticated gh or a non-GitHub remote explains itself there, and
/// that guidance is what the operator acts on. Bedrock's projection
/// refusals carry their reason in `message` instead.
fn ci_failure_sentence(report: &CiReport, stderr: Option<&str>) -> String {
    let verb = if report.outcome == "timed_out" {
        "timed out"
    } else {
        "failed"
    };
    let guidance = stderr
        .map(str::trim)
        .filter(|guidance| !guidance.is_empty())
        .or_else(|| {
            report
                .message
                .as_deref()
                .map(str::trim)
                .filter(|message| !message.is_empty())
        });
    match guidance {
        Some(guidance) => format!("The CI check {verb}: {}", clip(guidance, 400)),
        None => format!("The CI check {verb}."),
    }
}

/// The word a check row is judged on: the conclusion when the run has one,
/// the status otherwise, "unknown" when gh said neither.
fn check_verdict(check: &CiCheck) -> &str {
    check
        .conclusion
        .as_deref()
        .map(str::trim)
        .filter(|word| !word.is_empty())
        .or_else(|| {
            check
                .status
                .as_deref()
                .map(str::trim)
                .filter(|word| !word.is_empty())
        })
        .unwrap_or("unknown")
}

/// The color a verdict earns: the one word that means done-and-well, the
/// family that means it is not, and neutral for everything else ("skipped",
/// "neutral", a bare status).
fn conclusion_tone(verdict: &str) -> &'static str {
    match verdict {
        "success" => "good",
        "failure" | "timed_out" | "startup_failure" | "action_required" | "cancelled" => "bad",
        _ => "",
    }
}

/// One recorded check as a line the eye can scan: name, verdict, and the
/// commit it judged.
fn check_line(check: &CiCheck) -> String {
    let name = check
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("(unnamed)");
    let mut line = format!("{name}: {}", check_verdict(check));
    if !check.head_sha.is_empty() {
        line.push_str(" @ ");
        line.push_str(&short_sha(&check.head_sha));
    }
    line
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

/// Whether a row is a CI marker — another member's pull changed the
/// recorded view this section reads back.
fn is_repo_ci_marker(row: &RoomMessage) -> bool {
    is_workspace_marker(row) && row.body.starts_with(REPO_CI_MARKER_PREFIX)
}

/// The union the wake Effect watches. One watermark covers both kinds
/// because a wake simply re-reads this section's two lanes: refreshing both
/// on either marker costs one extra silent GET, where a second watermark
/// would cost a second Effect that can drift from this one.
fn is_repo_wake_marker(row: &RoomMessage) -> bool {
    is_repo_clone_marker(row) || is_repo_ci_marker(row)
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

/// `script_refusal`'s rule for the one field a bind requires. Everything
/// else — scheme, host allowlist, branch grammar — is `validateRepoBinding`'s
/// judgment, relayed in its own words.
fn bind_refusal(remote: &str) -> Option<String> {
    if remote.trim().is_empty() {
        return Some("A binding names a remote URL.".to_string());
    }
    None
}

/// Exactly the keys `validateRepoBinding` admits, and only when given —
/// the payload is strict deny-extra upstream, and an omitted `branch` or
/// `dir` is how the upstream defaults stay upstream's.
fn bind_payload(remote: &str, branch: &str, dir: &str) -> serde_json::Value {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "remote".to_string(),
        serde_json::Value::String(remote.trim().to_string()),
    );
    let branch = branch.trim();
    if !branch.is_empty() {
        payload.insert(
            "branch".to_string(),
            serde_json::Value::String(branch.to_string()),
        );
    }
    let dir = dir.trim();
    if !dir.is_empty() {
        payload.insert(
            "dir".to_string(),
            serde_json::Value::String(dir.to_string()),
        );
    }
    serde_json::Value::Object(payload)
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
    /// The bind form's three fields. Room-scoped like the script; empty
    /// `branch`/`dir` mean "let upstream default", so empty is the reset.
    bind_remote: RwSignal<String>,
    bind_branch: RwSignal<String>,
    bind_dir: RwSignal<String>,
    /// Whether the unbind control is one click from firing. Unbind deletes
    /// the checkout, so the first click only arms this.
    confirm_unbind: RwSignal<bool>,
    /// The recorded CI view: read on open from Bedrock's table (no
    /// container run), replaced wholesale by a pull's reply.
    checks: RwSignal<Option<ChecksView>>,
    /// Monotonic ticket; only the latest overlapping read may publish.
    ticket: RwSignal<u64>,
    /// The recorded-CI read's own admission — separate from `ticket` so a
    /// binding read and a checks read can overlap without retiring each
    /// other.
    checks_ticket: RwSignal<u64>,
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
            bind_remote: RwSignal::new(String::new()),
            bind_branch: RwSignal::new(String::new()),
            bind_dir: RwSignal::new(String::new()),
            confirm_unbind: RwSignal::new(false),
            checks: RwSignal::new(None),
            ticket: RwSignal::new(0),
            checks_ticket: RwSignal::new(0),
            poll_epoch: RwSignal::new(0),
        }
    }

    /// Whether the panel is on screen. Public because the Escape ladder that
    /// owns the key lives in `rooms_workspace`, not here.
    pub fn panel_is_open(&self) -> bool {
        self.panel.get_untracked()
    }

    /// Close the panel and hand focus back to the control that opened it.
    /// A reopened panel must not resume a primed unbind confirm.
    pub fn close_panel(&self) {
        self.panel.set(false);
        self.confirm_unbind.set(false);
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
        self.checks_ticket
            .update(|ticket| *ticket = ticket.wrapping_add(1));
        self.poll_epoch
            .update(|epoch| *epoch = epoch.wrapping_add(1));
        self.view.set(None);
        self.checks.set(None);
        self.loading.set(false);
        self.error.set(None);
        self.marker_seen.set(None);
        self.note.set(None);
        self.working.set(None);
        self.panel.set(false);
        self.build_script.set(DEFAULT_BUILD_SCRIPT.to_string());
        self.bind_remote.set(String::new());
        self.bind_branch.set(String::new());
        self.bind_dir.set(String::new());
        self.confirm_unbind.set(false);
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

    /// Read the recorded CI state, silently — Bedrock answers from its own
    /// table, so this burns no container run and rides along with every
    /// binding read.
    fn fetch_checks(&self, key: String, actor: String) {
        let base = self.base();
        let me = *self;
        let ticket = self.checks_ticket.get_untracked().wrapping_add(1);
        self.checks_ticket.set(ticket);
        spawn_local(async move {
            let result = read_checks(&base, &key, &actor).await;
            me.publish_checks(
                result,
                read_is_current(ticket, me.checks_ticket.get_untracked()),
            );
        });
    }

    /// Publish a recorded-CI read. `publish_status`'s rule, applied twice:
    /// a failed read never blanks a standing list, and this background lane
    /// never touches `error` — a blip here must not stomp a command answer
    /// the operator is reading.
    fn publish_checks(&self, result: Result<ChecksView, String>, is_current: bool) {
        if !is_current {
            return;
        }
        match result {
            Ok(view) => self.checks.set(Some(view)),
            Err(sentence) => {
                let keep = self
                    .checks
                    .with_untracked(|slot| matches!(slot, Some(ChecksView::Recorded(_))));
                if !keep {
                    self.checks.set(Some(ChecksView::Unread(sentence)));
                }
            }
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

    /// Pull CI through the workspace's gh. No poller and no readback: the
    /// reply is always the answer — a checked one carries the full current
    /// list, a failed one carries gh's guidance — and another member's pull
    /// reaches this panel by marker.
    fn check_ci(&self, rooms: Rooms, key: String, actor: String) {
        let base = self.base();
        let me = *self;
        let generation = rooms.generation_snapshot();
        self.working.set(Some(RepoCommand::Ci));
        self.error.set(None);
        self.note.set(None);
        spawn_local(async move {
            let url = ci_url(&base, &key, &actor);
            let outcome = post_command(RepoCommand::Ci, &url, &serde_json::json!({})).await;
            me.publish_command(outcome, rooms.room_is_current(generation, &key));
        });
    }

    /// Bind a remote. The reply carries the fresh projection, but the view
    /// is re-read through `GET repo` instead — the mutation's word never
    /// becomes the panel's, so bind renders exactly what any reload would.
    fn bind_repo(&self, rooms: Rooms, key: String, actor: String) {
        let remote = self.bind_remote.get_untracked();
        if let Some(refusal) = bind_refusal(&remote) {
            self.error.set(Some((RepoErrorSource::Command, refusal)));
            return;
        }
        let payload = bind_payload(
            &remote,
            &self.bind_branch.get_untracked(),
            &self.bind_dir.get_untracked(),
        );
        let base = self.base();
        let me = *self;
        let generation = rooms.generation_snapshot();
        self.working.set(Some(RepoCommand::Bind));
        self.error.set(None);
        self.note.set(None);
        spawn_local(async move {
            let url = bind_url(&base, &key, &actor);
            let outcome = post_command(RepoCommand::Bind, &url, &payload).await;
            let current = rooms.room_is_current(generation, &key);
            let landed = matches!(outcome, CommandOutcome::Bound(_));
            me.publish_command(outcome, current);
            if current && landed {
                me.fetch(rooms, key, actor);
            }
        });
    }

    /// Unbind the repo. `{}` because the lane demands a JSON object even
    /// though the upstream DELETE reads none; the readback re-proves the
    /// unbound state the same way bind re-proves the binding.
    fn unbind_repo(&self, rooms: Rooms, key: String, actor: String) {
        let base = self.base();
        let me = *self;
        let generation = rooms.generation_snapshot();
        self.working.set(Some(RepoCommand::Unbind));
        self.error.set(None);
        self.note.set(None);
        spawn_local(async move {
            let url = unbind_url(&base, &key, &actor);
            let outcome = post_command(RepoCommand::Unbind, &url, &serde_json::json!({})).await;
            let current = rooms.room_is_current(generation, &key);
            let landed = matches!(outcome, CommandOutcome::Unbound { .. });
            me.publish_command(outcome, current);
            if current && landed {
                me.fetch(rooms, key, actor);
            }
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
            // The view deliberately stays as it was: the caller's readback
            // is what moves it, so the panel never renders a state only a
            // mutation reply has claimed.
            CommandOutcome::Bound(repo) => self.note.set(Some(bound_sentence(&repo))),
            CommandOutcome::Unbound {
                checkout_removed,
                reason,
            } => self
                .note
                .set(Some(unbind_sentence(checkout_removed, reason.as_deref()))),
            CommandOutcome::Checked(report, checks) => {
                self.note.set(Some(ci_sentence(&report)));
                self.checks.set(Some(ChecksView::Recorded(checks)));
            }
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

/// One recorded-CI read: transport, decode, classify. Same undecodable-404
/// posture as `read_status`, for the same reason.
async fn read_checks(base: &str, key: &str, actor: &str) -> Result<ChecksView, String> {
    let url = ci_url(base, key, actor);
    match Request::get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.json::<RepoBody>().await.ok();
            classify_checks(status, body)
        }
        Err(err) => Err(format!("The recorded CI request failed: {err}")),
    }
}

/// One command POST. The body is exactly what the daemon's strict lane
/// expects: `{}` for clone and CI, `{script}` for build — `actor_member_id`
/// is the daemon's to assert, never this side's.
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
    // binding from being read, however briefly, under this room's name. The
    // recorded CI read rides along: Bedrock answers it from its own table,
    // so being readable on open costs no container run.
    Effect::new(move |_| match read_target.get() {
        Some((key, actor)) => {
            state.reset();
            state.fetch_checks(key.clone(), actor.clone());
            state.fetch(rooms, key, actor);
        }
        None => state.reset(),
    });

    // The wake path: a clone finishing anywhere — another member, another
    // session — lands on the transcript as a System marker, and a CI pull
    // that found news does too, so watching the SSE-fed transcript closes
    // the gap between "the room heard" and "this panel shows it". The
    // watermark starts over with reset() AND carries the room generation,
    // so hydration reads as the initial load, never as news, whichever
    // Effect runs first on a room switch.
    Effect::new(move |_| {
        let (watermark, wake) = rooms.transcript.with(|transcript| {
            marker_wake(
                state.marker_seen.get_untracked(),
                rooms.generation_snapshot(),
                transcript,
                is_repo_wake_marker,
            )
        });
        state.marker_seen.set(watermark);
        if !wake {
            return;
        }
        let Some((key, actor)) = read_target.get_untracked() else {
            return;
        };
        state.refresh_on_marker(rooms, key.clone(), actor.clone());
        state.fetch_checks(key, actor);
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
                                        Some(RepoView::Unbound) => {
                                            panel_unbound(state, actor, rooms, can_run)
                                                .into_any()
                                        }
                                        _ => ().into_any(),
                                    }}
                                    // Owner-gated on the wire, not here:
                                    // this surface cannot run the identity
                                    // map, so the controls render for every
                                    // member and the typed refusal answers.
                                    <div class="rooms-workspace__repo-footnote">
                                        "Binding and unbinding are owner acts \u{2014} \
                                         the daemon refuses them for anyone else."
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

/// The panel when no repo is bound: the answer, and the owner's bind form.
/// Rendered for every member — the daemon's owner gate is the authority,
/// and its typed refusal reads as a calm sentence here, not a failure.
fn panel_unbound(
    state: RoomRepoState,
    actor: impl Fn() -> Option<(String, String)> + Copy + Send + Sync + 'static,
    rooms: Rooms,
    can_run: impl Fn() -> bool + Copy + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        <div class="rooms-workspace__repo-note">
            "No repo is bound to this room yet."
        </div>
        <div class="rooms-workspace__repo-bind">
            <input
                class="rooms-workspace__repo-input"
                type="text"
                aria-label="Remote URL to bind"
                placeholder="https://github.com/org/repo.git"
                prop:value=move || state.bind_remote.get()
                on:input=move |ev| state.bind_remote.set(event_target_value(&ev))
            />
            // Empty means upstream's defaults — `main`, and Bedrock's
            // checkout dir — so the placeholders name them instead of
            // this side pre-filling values it would then have to send.
            <div class="rooms-workspace__repo-bind-pair">
                <input
                    class="rooms-workspace__repo-input"
                    type="text"
                    aria-label="Branch (defaults to main)"
                    placeholder="main"
                    prop:value=move || state.bind_branch.get()
                    on:input=move |ev| state.bind_branch.set(event_target_value(&ev))
                />
                <input
                    class="rooms-workspace__repo-input"
                    type="text"
                    aria-label="Checkout directory (defaults to repo)"
                    placeholder="repo"
                    prop:value=move || state.bind_dir.get()
                    on:input=move |ev| state.bind_dir.set(event_target_value(&ev))
                />
            </div>
            <div class="rooms-workspace__repo-actions">
                <button
                    class="rooms-workspace__repo-run"
                    type="button"
                    title="Bind this remote as the room's repo"
                    disabled=move || !can_run()
                    on:click=move |_| {
                        let Some((key, actor_id)) = actor() else { return };
                        state.bind_repo(rooms, key, actor_id);
                    }
                >
                    {move || {
                        if state.working.get() == Some(RepoCommand::Bind) {
                            "binding\u{2026}"
                        } else {
                            "bind"
                        }
                    }}
                </button>
            </div>
        </div>
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
            // Cloned-only like the build row: the pull runs gh in the
            // checkout, and Bedrock answers 409 until one exists.
            <div class="rooms-workspace__repo-actions">
                <button
                    class="rooms-workspace__repo-run"
                    type="button"
                    title="Pull CI results for the bound branch through the workspace's gh"
                    disabled=move || !can_run()
                    on:click=move |_| {
                        let Some((key, actor_id)) = actor() else { return };
                        state.check_ci(rooms, key, actor_id);
                    }
                >
                    {move || {
                        if state.working.get() == Some(RepoCommand::Ci) {
                            "checking\u{2026}"
                        } else {
                            "check CI"
                        }
                    }}
                </button>
            </div>
        })}

        // The recorded CI state — Bedrock's table, no container run.
        // Rendered for any standing binding, cloned or not: the rows
        // outlive the checkout, and a member rejoining after a container
        // churn still deserves the room's CI history on open.
        {move || {
            let Some(checks_view) = state.checks.get() else {
                return ().into_any();
            };
            match checks_view {
                ChecksView::Unavailable => ().into_any(),
                ChecksView::Unread(sentence) => view! {
                    <div class="rooms-workspace__repo-ci">
                        <div class="rooms-workspace__repo-ci-title">"recorded CI"</div>
                        <div class="rooms-workspace__repo-note">{sentence}</div>
                    </div>
                }
                .into_any(),
                ChecksView::Recorded(checks) if checks.is_empty() => view! {
                    <div class="rooms-workspace__repo-ci">
                        <div class="rooms-workspace__repo-ci-title">"recorded CI"</div>
                        <div class="rooms-workspace__repo-note">
                            "No CI results recorded for this room yet."
                        </div>
                    </div>
                }
                .into_any(),
                ChecksView::Recorded(checks) => {
                    let rows = checks.into_iter().map(check_row).collect_view();
                    view! {
                        <div class="rooms-workspace__repo-ci">
                            <div class="rooms-workspace__repo-ci-title">"recorded CI"</div>
                            <ul class="rooms-workspace__repo-ci-list">{rows}</ul>
                        </div>
                    }
                    .into_any()
                }
            }
        }}

        // Unbinding deletes the checkout with the binding, so the first
        // click only arms the confirm — a second, separate click fires.
        <div class="rooms-workspace__repo-unbind">
            {move || {
                if state.confirm_unbind.get() {
                    view! {
                        <span class="rooms-workspace__repo-unbind-warn">
                            "Unbinding deletes the workspace checkout."
                        </span>
                        <button
                            class="rooms-workspace__repo-run rooms-workspace__repo-run--danger"
                            type="button"
                            disabled=move || !can_run()
                            on:click=move |_| {
                                let Some((key, actor_id)) = actor() else { return };
                                state.confirm_unbind.set(false);
                                state.unbind_repo(rooms, key, actor_id);
                            }
                        >
                            "unbind"
                        </button>
                        <button
                            class="rooms-workspace__repo-run"
                            type="button"
                            on:click=move |_| state.confirm_unbind.set(false)
                        >
                            "keep"
                        </button>
                    }.into_any()
                } else {
                    view! {
                        <button
                            class="rooms-workspace__repo-run rooms-workspace__repo-run--danger"
                            type="button"
                            title="Unbind this room's repo \u{2014} deletes the workspace checkout"
                            disabled=move || !can_run()
                            on:click=move |_| state.confirm_unbind.set(true)
                        >
                            {move || {
                                if state.working.get() == Some(RepoCommand::Unbind) {
                                    "unbinding\u{2026}"
                                } else {
                                    "unbind\u{2026}"
                                }
                            }}
                        </button>
                    }.into_any()
                }
            }}
        </div>
    }
}

/// The href a recorded check may carry: gh's `url`, but only when it is a
/// well-formed http(s) URL. The field is gh stdout read inside the room
/// container — the same container the bound repo's build script runs in — so
/// it is the container's word, not GitHub's, and rendering it unvetted would
/// hand that container a clickable `javascript:` anchor in the surface
/// origin. Same allowlist as room markdown links; anything else stays text.
fn check_href(check: &CiCheck) -> Option<String> {
    check
        .url
        .clone()
        .filter(|url| crate::room_markdown::scheme_allowed(url))
}

/// One recorded check row: the line, linked when gh gave an http(s) URL, the
/// pull reply's `new` flag, and when this room first saw the result.
fn check_row(check: CiCheck) -> impl IntoView {
    let tone = conclusion_tone(check_verdict(&check));
    let line_class = if tone.is_empty() {
        "rooms-workspace__repo-check-line".to_string()
    } else {
        format!("rooms-workspace__repo-check-line rooms-workspace__repo-check-line--{tone}")
    };
    let line = check_line(&check);
    let url = check_href(&check);
    let seen = check.first_seen_at.clone().filter(|at| !at.is_empty());
    view! {
        <li class="rooms-workspace__repo-check">
            {check.new.then(|| view! {
                <span class="rooms-workspace__repo-check-new">"new"</span>
            })}
            {match url {
                Some(url) => view! {
                    <a class=line_class href=url target="_blank" rel="noopener noreferrer">{line}</a>
                }
                .into_any(),
                None => view! { <span class=line_class>{line}</span> }.into_any(),
            }}
            {seen.map(|at| view! {
                <span class="rooms-workspace__repo-check-seen">{at}</span>
            })}
        </li>
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
            bind_remote: RwSignal::new(String::new()),
            bind_branch: RwSignal::new(String::new()),
            bind_dir: RwSignal::new(String::new()),
            confirm_unbind: RwSignal::new(false),
            checks: RwSignal::new(None),
            ticket: RwSignal::new(0),
            checks_ticket: RwSignal::new(0),
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

    // ---- the CI pull --------------------------------------------------------

    /// The checked reply, field for field as Bedrock composes it: the
    /// report's counts, and the full current list with `new` flagging what
    /// this room had not recorded.
    #[test]
    fn a_checked_ci_carries_the_results() {
        let checked = body(
            r#"{"ci": {"outcome": "checked", "branch": "main", "repo_dir": "site",
                       "exit_code": 0, "duration_ms": 5200, "checks_total": 5, "checks_new": 2},
                "checks": [
                  {"check_run_id": "17296035001",
                   "head_sha": "0123456789012345678901234567890123456789",
                   "name": "lint", "title": "CI", "status": "completed",
                   "conclusion": "failure", "event": "push",
                   "url": "https://github.com/acme/site/actions/runs/17296035001",
                   "created_at": "2026-08-27T10:00:00Z", "updated_at": "2026-08-27T10:05:00Z",
                   "new": true},
                  {"check_run_id": "17296035000",
                   "head_sha": "0123456789012345678901234567890123456789",
                   "name": "build", "status": "completed", "conclusion": "success",
                   "new": false}
                ],
                "exec": {"id": "exec-9"}, "stderr": ""}"#,
        );
        let outcome = classify_command(RepoCommand::Ci, 200, Some(checked));
        let CommandOutcome::Checked(report, checks) = outcome else {
            panic!("expected Checked, got {outcome:?}");
        };
        assert_eq!(checks.len(), 2);
        assert!(checks[0].new);
        assert!(!checks[1].new);
        let sentence = ci_sentence(&report);
        assert!(
            sentence.contains("2 new results (5 total)"),
            "got: {sentence}"
        );
        assert!(sentence.contains("6s"), "got: {sentence}");
    }

    /// gh ran and exited nonzero — unauthenticated, a non-GitHub remote, a
    /// rate limit. The reply is 200, the stderr guidance is THE answer, and
    /// it must survive into the sentence like the clone failure's does.
    #[test]
    fn a_failed_ci_shows_the_gh_guidance() {
        let failed = body(
            r#"{"ci": {"outcome": "failed", "branch": "main", "repo_dir": "site",
                       "exit_code": 4, "duration_ms": 800},
                "exec": {"id": "exec-10"},
                "stderr": "To get started with GitHub CLI, please run:  gh auth login"}"#,
        );
        let outcome = classify_command(RepoCommand::Ci, 200, Some(failed));
        let CommandOutcome::Failure(sentence) = outcome else {
            panic!("expected Failure, got {outcome:?}");
        };
        assert!(sentence.contains("gh auth login"), "got: {sentence}");
        assert!(sentence.contains("failed"), "got: {sentence}");
    }

    #[test]
    fn a_timed_out_ci_says_so() {
        let stalled = body(
            r#"{"ci": {"outcome": "timed_out", "branch": "main", "repo_dir": "site",
                       "exit_code": null, "duration_ms": 600000},
                "exec": {"id": "exec-11"}, "stderr": ""}"#,
        );
        let outcome = classify_command(RepoCommand::Ci, 200, Some(stalled));
        assert_eq!(
            outcome,
            CommandOutcome::Failure("The CI check timed out.".to_string())
        );
    }

    /// gh exited 0 but Bedrock refused to vouch for the output — the coded
    /// refusal's `message` is the human half, and stderr (empty here) must
    /// not shadow it.
    #[test]
    fn a_rejected_ci_projection_carries_its_message() {
        let rejected = body(
            r#"{"ci": {"outcome": "failed", "error": "ci_output_rejected",
                       "message": "gh did not return JSON.", "branch": "main",
                       "repo_dir": "site", "exit_code": 0, "duration_ms": 900},
                "exec": {"id": "exec-12"}, "stderr": ""}"#,
        );
        let outcome = classify_command(RepoCommand::Ci, 200, Some(rejected));
        let CommandOutcome::Failure(sentence) = outcome else {
            panic!("expected Failure, got {outcome:?}");
        };
        assert!(sentence.contains("did not return JSON"), "got: {sentence}");
    }

    /// Bedrock's two typed refusals for a pull, both thrown (code under
    /// `details`): a clone in flight, and a repo not yet cloned.
    #[test]
    fn a_ci_pull_against_an_unready_checkout_is_a_state() {
        let cloning = body(
            r#"{"ok": false, "error": "A clone is running for this room; wait for it to finish.",
                "details": {"code": "repo_cloning"}}"#,
        );
        let outcome = classify_command(RepoCommand::Ci, 409, Some(cloning));
        let CommandOutcome::State(sentence) = outcome else {
            panic!("expected State, got {outcome:?}");
        };
        assert!(sentence.contains("already running"), "got: {sentence}");

        let uncloned = body(
            r#"{"ok": false, "error": "This room's repo has not been cloned into the workspace.",
                "details": {"code": "repo_not_cloned", "clone_status": "pending"}}"#,
        );
        let outcome = classify_command(RepoCommand::Ci, 409, Some(uncloned));
        let CommandOutcome::State(sentence) = outcome else {
            panic!("expected State, got {outcome:?}");
        };
        assert!(sentence.contains("clone it first"), "got: {sentence}");
    }

    /// A daemon predating the CI lane refuses the POST with its own coded
    /// 404 — a failure in words at the click, not a state and not a crash.
    #[test]
    fn a_route_less_daemon_refuses_the_ci_pull_in_words() {
        let refused = body(
            r#"{"ok": false, "code": "workspace_route_not_allowed",
                "error": "this workspace route is not allowed"}"#,
        );
        let outcome = classify_command(RepoCommand::Ci, 404, Some(refused));
        assert_eq!(
            outcome,
            CommandOutcome::Failure(
                "This Ocean deployment doesn't expose that workspace route.".to_string()
            )
        );
    }

    // ---- the owner verbs ----------------------------------------------------

    /// The bind payload is strict deny-extra upstream: exactly the admitted
    /// keys, trimmed, and an empty `branch`/`dir` is OMITTED so the upstream
    /// defaults (`main`, Bedrock's checkout dir) stay upstream's.
    #[test]
    fn a_bind_payload_carries_exactly_what_was_given() {
        let full = bind_payload(" https://github.com/acme/site.git ", "trunk", "site");
        assert_eq!(
            full,
            serde_json::json!({
                "remote": "https://github.com/acme/site.git",
                "branch": "trunk",
                "dir": "site"
            })
        );
        let sparse = bind_payload("https://github.com/acme/site.git", "  ", "");
        assert_eq!(
            sparse,
            serde_json::json!({"remote": "https://github.com/acme/site.git"})
        );
    }

    #[test]
    fn an_empty_remote_is_refused_before_the_wire() {
        assert!(bind_refusal("  ").is_some());
        assert!(bind_refusal("").is_some());
        assert!(bind_refusal("https://github.com/acme/site.git").is_none());
    }

    /// A landed bind answers with the projection (200 on re-bind, 201 on
    /// first bind — both are the same answer here), and the sentence points
    /// at the next act.
    #[test]
    fn a_landed_bind_is_bound_and_points_at_the_clone() {
        let outcome = classify_command(RepoCommand::Bind, 201, Some(body(bound_json())));
        let CommandOutcome::Bound(repo) = outcome else {
            panic!("expected Bound, got {outcome:?}");
        };
        let sentence = bound_sentence(&repo);
        assert!(sentence.contains("acme/site"), "got: {sentence}");
        assert!(sentence.contains("clone"), "got: {sentence}");
    }

    /// The daemon's owner gate answering a non-principal actor is how the
    /// room is shaped for that caller — a state in a calm voice, never a
    /// failure alert.
    #[test]
    fn a_non_principal_bind_is_a_state_not_a_failure() {
        let gated = body(
            r#"{"ok": false, "code": "workspace_not_owner_principal",
                "error": "an owner verb forwards only for the principal this room's credential speaks for"}"#,
        );
        let outcome = classify_command(RepoCommand::Bind, 403, Some(gated));
        let CommandOutcome::State(sentence) = outcome else {
            panic!("expected State, got {outcome:?}");
        };
        assert!(sentence.contains("room owner"), "got: {sentence}");
    }

    /// Bedrock's own owner check throws WITHOUT a code — prose only — and
    /// that prose must reach the operator through the fallthrough.
    #[test]
    fn bedrocks_owner_refusal_reads_in_its_own_words() {
        let refused =
            body(r#"{"error": "Only the room owner may change the shape of the room workspace."}"#);
        let outcome = classify_command(RepoCommand::Unbind, 403, Some(refused));
        let CommandOutcome::Failure(sentence) = outcome else {
            panic!("expected Failure, got {outcome:?}");
        };
        assert!(sentence.contains("room owner"), "got: {sentence}");
    }

    /// `validateRepoBinding`'s judgments arrive as thrown 400s whose message
    /// is the guidance — an ssh remote, a flag-shaped branch. Relayed, not
    /// rewritten.
    #[test]
    fn a_rejected_binding_reads_in_bedrocks_words() {
        let rejected = body(
            r#"{"ok": false,
                "error": "remote must use https://. ssh, git, http, and file remotes are refused.",
                "details": {"code": "repo_remote_rejected"}}"#,
        );
        let outcome = classify_command(RepoCommand::Bind, 400, Some(rejected));
        let CommandOutcome::Failure(sentence) = outcome else {
            panic!("expected Failure, got {outcome:?}");
        };
        assert!(sentence.contains("https://"), "got: {sentence}");
    }

    /// The unbind reply's `checkout_removed` is the honest half of the
    /// answer: gone, nothing to remove, or left behind where the next flush
    /// will treat it as room files.
    #[test]
    fn an_unbind_reports_the_checkout_honestly() {
        let gone = body(r#"{"unbound": true, "checkout_removed": true}"#);
        let outcome = classify_command(RepoCommand::Unbind, 200, Some(gone));
        assert_eq!(
            outcome,
            CommandOutcome::Unbound {
                checkout_removed: true,
                reason: None
            }
        );
        assert!(unbind_sentence(true, None).contains("checkout was removed"));

        let dark = body(
            r#"{"unbound": true, "checkout_removed": false,
                "checkout_removed_reason": "no_container"}"#,
        );
        let CommandOutcome::Unbound {
            checkout_removed,
            reason,
        } = classify_command(RepoCommand::Unbind, 200, Some(dark))
        else {
            panic!("expected Unbound");
        };
        assert!(!checkout_removed);
        let sentence = unbind_sentence(checkout_removed, reason.as_deref());
        assert!(sentence.contains("No container"), "got: {sentence}");

        let stuck = unbind_sentence(false, Some("rm_failed"));
        assert!(stuck.contains("could not be removed"), "got: {stuck}");
        assert!(stuck.contains("room files"), "got: {stuck}");
    }

    /// Losing an unbind race is Bedrock's thrown `repo_unbound` — already an
    /// answer ("nothing is bound"), so it renders as the state it is.
    #[test]
    fn an_unbind_race_is_a_state() {
        let raced = body(
            r#"{"ok": false, "error": "This room has no repo bound to it.",
                "details": {"code": "repo_unbound"}}"#,
        );
        let outcome = classify_command(RepoCommand::Unbind, 409, Some(raced));
        let CommandOutcome::State(sentence) = outcome else {
            panic!("expected State, got {outcome:?}");
        };
        assert!(sentence.contains("No repo is bound"), "got: {sentence}");
    }

    #[test]
    fn an_unmapped_actor_is_a_failure_in_words() {
        let unmapped = body(
            r#"{"ok": false, "code": "workspace_actor_unmapped",
                "error": "the asserted actor resolves to no Bedrock member id on this daemon"}"#,
        );
        let outcome = classify_command(RepoCommand::Bind, 403, Some(unmapped));
        assert_eq!(
            outcome,
            CommandOutcome::Failure(
                "Your identity doesn't map to this room's compute service.".to_string()
            )
        );
    }

    /// Neither owner verb trusts its mutation reply into the view: the note
    /// carries the outcome and the caller's readback moves the panel, so
    /// what renders is always what a fresh read would answer.
    #[test]
    fn owner_verbs_note_but_never_move_the_view() {
        let state = fresh_state();
        state.working.set(Some(RepoCommand::Bind));
        let repo = RepoProjection {
            remote: "https://github.com/acme/site.git".into(),
            branch: "main".into(),
            clone_status: "pending".into(),
            head_sha: None,
            last_cloned_at: None,
            clone_error: None,
        };
        state.publish_command(CommandOutcome::Bound(Box::new(repo)), true);
        assert_eq!(state.working.get_untracked(), None);
        assert_eq!(state.view.get_untracked(), None);
        let note = state.note.get_untracked().unwrap();
        assert!(note.contains("acme/site"), "got: {note}");

        state.working.set(Some(RepoCommand::Unbind));
        state.publish_command(
            CommandOutcome::Unbound {
                checkout_removed: true,
                reason: None,
            },
            true,
        );
        assert_eq!(state.working.get_untracked(), None);
        assert_eq!(state.view.get_untracked(), None);
        let note = state.note.get_untracked().unwrap();
        assert!(note.contains("unbound"), "got: {note}");
    }

    // ---- the recorded CI read -----------------------------------------------

    /// The GET's answer: rows from Bedrock's table, `first_seen_at` and all,
    /// with no `new` flag — that is the pull reply's word, not the table's.
    #[test]
    fn recorded_checks_read_back() {
        let recorded = body(
            r#"{"checks": [
                  {"check_run_id": "17296035001",
                   "head_sha": "0123456789012345678901234567890123456789",
                   "name": "lint", "status": "completed", "conclusion": "failure",
                   "url": "https://github.com/acme/site/actions/runs/17296035001",
                   "first_seen_at": "2026-08-27T10:06:00.000Z"}
                ]}"#,
        );
        let view = classify_checks(200, Some(recorded)).unwrap();
        let ChecksView::Recorded(checks) = view else {
            panic!("expected Recorded, got {view:?}");
        };
        assert_eq!(checks.len(), 1);
        assert_eq!(
            checks[0].first_seen_at.as_deref(),
            Some("2026-08-27T10:06:00.000Z")
        );
        assert!(!checks[0].new);
    }

    /// An empty history is an answer — the panel says "none recorded yet",
    /// never nothing.
    #[test]
    fn an_empty_recorded_history_is_an_answer() {
        let empty = body(r#"{"checks": []}"#);
        assert_eq!(
            classify_checks(200, Some(empty)),
            Ok(ChecksView::Recorded(Vec::new()))
        );
    }

    /// A deployment predating the lane — the daemon's coded 404 or an
    /// undecodable one — reads as quiet unavailability: the repo panel
    /// around it still works, and the POST says so in words if clicked.
    #[test]
    fn a_route_less_deployment_reads_ci_as_unavailable() {
        assert_eq!(classify_checks(404, None), Ok(ChecksView::Unavailable));
        let coded = body(
            r#"{"ok": false, "code": "workspace_route_not_allowed",
                "error": "this workspace route is not allowed"}"#,
        );
        assert_eq!(
            classify_checks(404, Some(coded)),
            Ok(ChecksView::Unavailable)
        );
    }

    #[test]
    fn an_unreachable_bedrock_fails_the_ci_read_in_words() {
        let relay = body(
            r#"{"ok": false, "code": "workspace_unavailable",
                "error": "the room workspace could not be reached"}"#,
        );
        let err = classify_checks(503, Some(relay)).unwrap_err();
        assert!(err.contains("can't be reached"), "got: {err}");
        let opaque = classify_checks(500, None).unwrap_err();
        assert!(opaque.contains("500"), "got: {opaque}");
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

    fn recorded_check(name: &str, conclusion: &str) -> CiCheck {
        CiCheck {
            check_run_id: "1".into(),
            head_sha: "0123456789012345678901234567890123456789".into(),
            name: Some(name.into()),
            status: Some("completed".into()),
            conclusion: Some(conclusion.into()),
            url: None,
            first_seen_at: None,
            new: false,
        }
    }

    /// A checked pull replaces the recorded view wholesale — the reply IS
    /// the table's current answer — and the note says the news.
    #[test]
    fn a_checked_pull_replaces_the_recorded_view_and_says_the_news() {
        let state = fresh_state();
        state.working.set(Some(RepoCommand::Ci));
        let report = CiReport {
            outcome: "checked".into(),
            exit_code: Some(0),
            duration_ms: Some(2100),
            checks_total: Some(3),
            checks_new: Some(1),
            message: None,
        };
        let pulled = vec![recorded_check("lint", "success")];
        state.publish_command(CommandOutcome::Checked(report, pulled.clone()), true);
        assert_eq!(state.working.get_untracked(), None);
        assert_eq!(
            state.checks.get_untracked(),
            Some(ChecksView::Recorded(pulled))
        );
        let note = state.note.get_untracked().unwrap();
        assert!(note.contains("1 new result (3 total)"), "got: {note}");
    }

    #[test]
    fn a_stale_checks_read_publishes_nothing() {
        let state = fresh_state();
        state.publish_checks(Ok(ChecksView::Recorded(Vec::new())), false);
        assert_eq!(state.checks.get_untracked(), None);
    }

    /// `publish_status`'s rule, held on this lane too: a blipped background
    /// read neither blanks a standing recorded list nor touches the shared
    /// error signal a command answer may be occupying.
    #[test]
    fn a_failed_checks_read_never_blanks_a_standing_list() {
        let state = fresh_state();
        state.publish_checks(Err("net blip".to_string()), true);
        assert_eq!(
            state.checks.get_untracked(),
            Some(ChecksView::Unread("net blip".to_string()))
        );

        let standing = vec![recorded_check("lint", "success")];
        state.publish_checks(Ok(ChecksView::Recorded(standing.clone())), true);
        state.error.set(Some((
            RepoErrorSource::Command,
            "The clone failed.".to_string(),
        )));
        state.publish_checks(Err("net blip".to_string()), true);
        assert_eq!(
            state.checks.get_untracked(),
            Some(ChecksView::Recorded(standing))
        );
        assert_eq!(
            state.error.get_untracked(),
            Some((RepoErrorSource::Command, "The clone failed.".to_string())),
            "a checks read must never stomp a standing command answer"
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

    /// A check row degrades honestly: the verdict falls from conclusion to
    /// status to "unknown", and the tone colors only words with a meaning.
    #[test]
    fn a_check_reads_as_name_verdict_and_sha() {
        let check = recorded_check("lint", "failure");
        assert_eq!(check_line(&check), "lint: failure @ 0123456789");
        assert_eq!(conclusion_tone(check_verdict(&check)), "bad");

        let unfinished = CiCheck {
            conclusion: None,
            ..recorded_check("build", "")
        };
        assert_eq!(check_verdict(&unfinished), "completed");
        assert_eq!(conclusion_tone(check_verdict(&unfinished)), "");

        let bare = CiCheck {
            name: None,
            status: None,
            conclusion: None,
            head_sha: String::new(),
            ..recorded_check("", "")
        };
        assert_eq!(check_line(&bare), "(unnamed): unknown");
        assert_eq!(conclusion_tone("success"), "good");
    }

    /// The check URL is container-influenced gh output, not GitHub's word:
    /// only a well-formed http(s) URL survives to become an anchor, the same
    /// posture the markdown renderers hold against `javascript:` hrefs.
    #[test]
    fn a_check_href_is_gated_to_http_schemes() {
        let with_url = |url: &str| CiCheck {
            url: Some(url.into()),
            ..recorded_check("lint", "success")
        };

        let run = "https://github.com/acme/site/actions/runs/1/job/2";
        assert_eq!(check_href(&with_url(run)).as_deref(), Some(run));

        for hostile in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "java\tscript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "vbscript:x",
            "",
        ] {
            assert_eq!(check_href(&with_url(hostile)), None, "{hostile:?} linked");
        }
        assert_eq!(check_href(&recorded_check("lint", "success")), None);
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
        assert!(bind_url("http://d", "k", "a").ends_with("/workspace/repo/bind?actor_id=a"));
        assert!(unbind_url("http://d", "k", "a").ends_with("/workspace/repo/unbind?actor_id=a"));
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

    /// The wake union: a CI marker refreshes the recorded view another
    /// member's pull just changed, clone markers keep waking the binding
    /// read, and the other marker variants still wake nothing here.
    #[test]
    fn a_ci_marker_wakes_the_recorded_view() {
        let ci = system_row(
            2,
            "workspace CI on 'main': 2 new results (5 total) \u{2014} lint: failure, build: success",
        );
        assert!(is_repo_ci_marker(&ci));
        assert!(!is_repo_ci_marker(&system_row(
            3,
            "workspace repo cloned: 'main' @ 0123456789ab"
        )));
        assert!(is_repo_wake_marker(&ci));
        assert!(is_repo_wake_marker(&system_row(
            3,
            "workspace repo cloned: 'main' @ 0123456789ab"
        )));
        assert!(!is_repo_wake_marker(&system_row(
            4,
            "workspace build 'build' succeeded (3.2s)"
        )));

        // Through the shared wake mechanics: recorded history stays silent,
        // a live CI marker wakes.
        let history = vec![system_row(1, "workspace hydrated (12 files)")];
        let (watermark, wake) = marker_wake(None, 7, &history, is_repo_wake_marker);
        assert!(!wake);
        let mut transcript = history;
        transcript.push(ci);
        let (_, wake) = marker_wake(watermark, 7, &transcript, is_repo_wake_marker);
        assert!(wake);
    }
}
