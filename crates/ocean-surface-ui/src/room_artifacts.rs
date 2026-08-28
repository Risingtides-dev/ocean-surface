//! A room's artifacts — the browser half of the daemon's four artifact routes.
//!
//! A room produces things that outlive its transcript: a task somebody took, a
//! decision that must not be re-litigated, a piece of captured knowledge. The
//! daemon has stored all three durably since the rooms work landed, and until
//! now no browser could list, open, create or amend any of them. `room_summary`
//! reaches exactly ONE of these — the `room-summary` singleton — through its own
//! pair of routes; everything else in the room's store was unreachable.
//!
//!   GET  /v1/rooms/persistent/{key}/artifacts                   → all of them
//!   GET  /v1/rooms/persistent/{key}/artifacts/{id}              → one
//!   POST /v1/rooms/persistent/{key}/artifacts                   → create
//!   POST /v1/rooms/persistent/{key}/artifacts/{id}/amend        → amend (CAS)
//!
//! Four properties of that wire contract shape everything below:
//!
//! 1. **Amend is compare-and-swap, and the refusal is the feature.** A stale
//!    write comes back 409 `artifact_version_conflict` carrying BOTH the version
//!    that was presented and the version that actually stands. The daemon is
//!    deliberately handing back where to re-read from, so this side must say
//!    "someone moved this to v{actual}" and offer that re-read. Merging, or
//!    retrying with the newer version, is last-writer-wins — the exact bug the
//!    store's `version` column exists to refuse.
//! 2. **`expected_version` is the version the editor OPENED against**, not the
//!    latest one this module has seen. Those differ the moment a background
//!    list refresh lands mid-edit, and sending the fresher one would quietly
//!    overwrite a change the editor never showed its author.
//! 3. **Both request bodies are `deny_unknown_fields`.** One extra key is a 400
//!    the operator could do nothing about, so exactly the named fields go out.
//! 4. **`author_id` is caller-asserted and gated on create.** An id resolving to
//!    an Agent or System participant comes back 403 `forged_artifact_author` —
//!    an agent's artifact is written by the daemon, never by a client claiming
//!    its identity. Note the daemon applies that gate on CREATE only; amend is
//!    roster-checked in the store but carries no forged-author arm, so this
//!    module must not promise a guard the wire does not have.
//!
//! One thing this module deliberately does NOT do is filter. The room's summary
//! is a real artifact with a real id and it appears in this list like any other,
//! even though the rail renders it separately. A list that quietly omits rows is
//! a list that stops matching the room it claims to describe.
//!
//! Everything that turns a reply into what the operator sees is a free function
//! below, unit-testable natively without a browser or a daemon.

use std::collections::HashSet;

use gloo_net::http::Request;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen_futures::spawn_local;

use crate::room_summary::RoomArtifact;
use crate::rooms::{encode, Rooms};

/// The kinds the daemon's `RoomArtifactKind` accepts, in the order the create
/// form offers them. Snake_case strings rather than a mirrored enum for the
/// same reason `RoomArtifact` keeps `kind` as a `String`: this is the wire
/// alphabet, and an enum here would only add a second place to update.
const ARTIFACT_KINDS: [&str; 3] = ["task", "decision", "note"];

/// The lifecycle values `RoomArtifactState` accepts. An amend may only ever
/// send one of these — anything else is a deserialize error at the daemon.
const ARTIFACT_STATES: [&str; 3] = ["open", "done", "dropped"];

// ---- Wire types -------------------------------------------------------------

/// `POST .../artifacts`. Five fields, because `CreateArtifactRequest` is
/// `deny_unknown_fields` on the daemon and `state` is NOT one of them — a new
/// artifact is always born `open`, and asking for otherwise is a 400.
#[derive(Debug, Serialize)]
struct CreateRequest<'a> {
    id: &'a str,
    kind: &'a str,
    title: &'a str,
    body: &'a str,
    author_id: &'a str,
}

/// `POST .../artifacts/{id}/amend`. `title`/`body`/`state` are each optional on
/// the daemon and omitted here when unchanged, so a lifecycle-only amend does
/// not re-send prose it is not editing — the store compares the resolved values
/// against what stands and refuses an amend that would change nothing.
#[derive(Debug, Serialize)]
struct AmendRequest<'a> {
    expected_version: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<&'a str>,
    author_id: &'a str,
}

#[derive(Debug, Deserialize)]
struct ListBody {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    artifacts: Vec<RoomArtifact>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// The reply shape shared by the single read and both writes. `expected_version`
/// and `actual_version` are present only on the 409, which is the one refusal
/// this module can act on rather than merely report.
#[derive(Debug, Deserialize)]
struct ArtifactBody {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    artifact: Option<RoomArtifact>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    expected_version: Option<u64>,
    #[serde(default)]
    actual_version: Option<u64>,
}

// ---- Pure helpers -----------------------------------------------------------

fn list_url(base: &str, key: &str) -> String {
    format!("{base}/v1/rooms/persistent/{}/artifacts", encode(key))
}

fn artifact_url(base: &str, key: &str, artifact_id: &str) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/artifacts/{}",
        encode(key),
        encode(artifact_id),
    )
}

fn amend_url(base: &str, key: &str, artifact_id: &str) -> String {
    format!("{}/amend", artifact_url(base, key, artifact_id))
}

/// What a write reply means.
#[derive(Debug, PartialEq, Eq)]
enum WriteOutcome {
    /// The artifact as it now stands. Both create and a successful amend answer
    /// with the whole row, so neither needs a follow-up read. Boxed because the
    /// row dwarfs the two refusal variants, and every caller moves this enum
    /// across an await point.
    Wrote(Box<RoomArtifact>),
    /// The compare-and-swap refused: the caller presented `expected` and the
    /// store is at `actual`. Carried as numbers rather than a rendered sentence
    /// because the recovery — re-read at `actual` — is driven by them.
    Conflict { expected: u64, actual: u64 },
    /// A refusal, in words an operator can act on.
    Failure(String),
}

/// Map a write reply onto what the room should show.
///
/// The conflict arm keys off the `code`, not the 409 status: the daemon answers
/// 409 for a duplicate artifact id too (`ArtifactAlreadyExists`), and that one
/// carries no versions to re-read from. Treating it as a CAS conflict would
/// offer a re-read of an artifact the caller never had open.
fn classify_write(status: u16, body: ArtifactBody) -> WriteOutcome {
    if body.ok {
        return match body.artifact {
            Some(artifact) => WriteOutcome::Wrote(Box::new(artifact)),
            // A shape the daemon does not emit. An "ok" carrying nothing must
            // never be allowed to stand in for the row that was written.
            None => WriteOutcome::Failure("The room saved it but sent nothing back.".to_string()),
        };
    }
    if body.code.as_deref() == Some("artifact_version_conflict") {
        if let (Some(expected), Some(actual)) = (body.expected_version, body.actual_version) {
            return WriteOutcome::Conflict { expected, actual };
        }
    }
    WriteOutcome::Failure(artifact_failure_message(
        status,
        body.code.as_deref(),
        body.error.as_deref(),
    ))
}

/// What reading one artifact back told us.
///
/// `Ok(None)` is the case that matters: `unknown_artifact` means the row is
/// gone, which is an ANSWER. Re-reading after a conflict is the one place this
/// can legitimately happen — the other writer may have been amending toward a
/// room whose artifact was since removed.
fn classify_read(status: u16, body: ArtifactBody) -> Result<Option<RoomArtifact>, String> {
    if body.ok {
        return Ok(body.artifact);
    }
    // The code, not the status. `unknown_artifact` is the only coded 404 on this
    // route; the unknown ROOM answers 404 with no `code` at all, and reading
    // that as a missing artifact would say a room that is GONE merely lost one
    // row.
    if status == 404 && body.code.as_deref() == Some("unknown_artifact") {
        return Ok(None);
    }
    Err(artifact_failure_message(
        status,
        body.code.as_deref(),
        body.error.as_deref(),
    ))
}

fn classify_list(status: u16, body: ListBody) -> Result<Vec<RoomArtifact>, String> {
    if body.ok {
        return Ok(body.artifacts);
    }
    Err(artifact_failure_message(
        status,
        body.code.as_deref(),
        body.error.as_deref(),
    ))
}

/// Turn a refusal into something an operator can act on.
///
/// Only four of these refusals carry a typed `code`; the store's own errors
/// (duplicate id, author off the roster, unknown room, a no-op amend) reach the
/// browser through `room_store_error_response`, which sends `error` prose and NO
/// `code` at all. So the fallback arm is not a formality here — it is the arm
/// most real failures land in, and it must read as a sentence.
fn artifact_failure_message(status: u16, code: Option<&str>, error: Option<&str>) -> String {
    match code {
        // The daemon gates create on the roster kind: an agent's artifact is
        // daemon-authored. Said about an artifact rather than a summary, but in
        // the same voice `room_summary` uses for the identical refusal.
        Some("forged_artifact_author") => {
            "An artifact is attributed to a person on the roster \u{2014} an agent's is written \
             by the daemon."
                .to_string()
        }
        Some("unknown_artifact") => "That artifact is no longer in this room.".to_string(),
        Some("artifact_version_conflict") => {
            "Someone else changed this artifact. Re-read it and try again.".to_string()
        }
        Some("unknown_room") => "That room is no longer open.".to_string(),
        _ => match error {
            // `invalid_request` is the daemon's whole error string on a create
            // this side should have refused first, and echoing the token at an
            // operator explains nothing.
            Some("invalid_request") => "The room refused that artifact.".to_string(),
            Some(text) if !text.is_empty() => format!("Artifact write failed: {text}"),
            _ => format!("Artifact write failed ({status})."),
        },
    }
}

/// The sentence a compare-and-swap refusal earns.
///
/// It names the version that actually stands because that number is the whole
/// recovery: re-read at `actual`, look at what changed, decide again.
fn conflict_message(expected: u64, actual: u64) -> String {
    format!(
        "Someone else moved this to v{actual} while you were editing v{expected}. \
         Re-read it to see their version."
    )
}

/// A lifecycle mark worth showing. `open` is the ordinary case and says nothing;
/// `done` and `dropped` are the two states a reader scanning the list needs.
fn lifecycle_mark(state: &str) -> Option<&'static str> {
    match state {
        "done" => Some("done"),
        "dropped" => Some("dropped"),
        _ => None,
    }
}

/// The one-line provenance under an artifact: kind, how far it has moved, who
/// last moved it, and its lifecycle if that is not the default.
///
/// `version` is the load-bearing half — it is the number an editor must present
/// back to amend, so showing it is what makes the compare-and-swap legible
/// rather than mysterious.
fn artifact_meta(artifact: &RoomArtifact) -> String {
    let mut meta = format!("{} \u{b7} v{}", artifact.kind, artifact.version);
    if !artifact.updated_by.is_empty() {
        meta.push_str(" \u{b7} ");
        meta.push_str(&artifact.updated_by);
    }
    if let Some(mark) = lifecycle_mark(&artifact.state) {
        meta.push_str(" \u{b7} ");
        meta.push_str(mark);
    }
    meta
}

/// What the rail shows for one row: the title, or the id when a title somehow
/// came back blank. Never an empty span — a row a reader cannot name is a row
/// they cannot open.
fn artifact_label(artifact: &RoomArtifact) -> String {
    let title = artifact.title.trim();
    if title.is_empty() {
        artifact.id.clone()
    } else {
        title.to_string()
    }
}

/// Mint an artifact id from the title the author typed.
///
/// The daemon requires an id that is non-empty and already trimmed, and it is
/// the addressing segment of two routes — so a human should not have to invent
/// one, and what they typed must not be able to produce a bad segment. Lowercase
/// alphanumerics and single hyphens only; anything else collapses to a hyphen.
///
/// An empty result is possible (a title of pure punctuation) and is returned as
/// such rather than papered over: `create_refusal` is the single place that
/// decides a create cannot go out.
fn slug_id(title: &str) -> String {
    let mut out = String::new();
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    // Long ids are legal but unreadable in a URL and in the transcript marker
    // the store writes; 48 chars is a title's worth without being a paragraph.
    // Trimming AFTER the truncation matters: the cut can land on a separator,
    // and a trailing hyphen is an ugly id nobody typed.
    let clipped: String = out.chars().take(48).collect();
    clipped.trim_end_matches('-').to_string()
}

/// Refuse a create this side already knows the daemon will reject, in the
/// daemon's own terms.
///
/// Mirroring the handler's pre-store validation is not duplication for its own
/// sake: those checks answer a bare `invalid_request` with no `code`, so a
/// create that reaches the wire malformed comes back as prose this module would
/// have to guess at. Refusing here says WHICH field is wrong.
fn create_refusal(id: &str, title: &str) -> Option<String> {
    if title.trim().is_empty() {
        return Some("An artifact needs a title.".to_string());
    }
    if id.trim().is_empty() || id != id.trim() {
        return Some(
            "That title has no letters or numbers to make an id from \u{2014} add some."
                .to_string(),
        );
    }
    None
}

/// Which fields an amend should actually send.
///
/// `None` for a field means "leave it alone", and the store treats an amend
/// whose resolved values all match what stands as `ArtifactUnchanged` — a 400,
/// deliberately, because bumping the version on a no-op writes a transcript line
/// claiming somebody changed something they did not. So a submit that changes
/// nothing must be caught HERE, before it becomes a refusal the author cannot
/// read a cause into.
fn amend_delta<'a>(
    current: &RoomArtifact,
    title: &'a str,
    body: &'a str,
    state: &'a str,
) -> Option<(Option<&'a str>, Option<&'a str>, Option<&'a str>)> {
    let title = (title.trim() != current.title).then_some(title.trim());
    let body = (body != current.body).then_some(body);
    let state = (state != current.state).then_some(state);
    if title.is_none() && body.is_none() && state.is_none() {
        return None;
    }
    Some((title, body, state))
}

/// Latest-wins admission for an overlapping read. Extracted for the same reason
/// `attachments.rs` and `room_summary.rs` extract theirs: an older completion
/// publishing over a newer one is what put a premature empty state on screen in
/// three previous features (TASK-104/106/107), and a guard no test can reach is
/// a guard the next edit deletes in silence.
fn read_is_current(ticket: u64, current: u64) -> bool {
    ticket == current
}

// ---- State ------------------------------------------------------------------

/// What the artifacts panel is showing inside itself.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Focus {
    /// The full list, at a reading measure the rail cannot offer.
    List,
    /// One artifact, open for reading and amending.
    Open(String),
    /// The create form.
    New,
}

/// Everything an amend puts on the wire, decided before anything is spawned so
/// the decision is readable from a test. `None` on a field means "leave it
/// alone"; `expected_version` is the whole compare-and-swap.
#[derive(Debug, PartialEq, Eq)]
struct AmendPlan {
    expected_version: u64,
    title: Option<String>,
    body: Option<String>,
    state: Option<String>,
}

/// Does a live compare-and-swap refusal need a plain sentence of its own here?
///
/// The open editor renders the full refusal with its re-read control, so it
/// needs no second copy. Everywhere else — the list, the create form, and the
/// rail with no panel open at all — would otherwise show NOTHING about a write
/// the store refused: `begin_write` cleared `error` and never re-set it, so an
/// author who stepped back from the editor mid-write reads a refusal as a save.
fn conflict_needs_its_own_line(focus: Option<&Focus>) -> bool {
    !matches!(focus, Some(Focus::Open(_)))
}

/// Escape owned by the artifacts panel.
///
/// It is the topmost overlay in the rooms surface — `z-index: 445`, above the
/// members drawer's 430 and its backdrop's 425 — so it consumes the key before
/// anything under it, and `rooms_workspace`'s ladder asks this first. Without
/// that branch the key would close the drawer UNDER an open modal, or fall
/// through to the app rail and tear down the whole rooms surface with an
/// unsaved draft inside it. A predicate for the same reason
/// `members_escape_closes` is one: a ladder rung no test can reach is a rung
/// the next edit deletes in silence.
pub fn artifacts_escape_closes(panel_open: bool, default_prevented: bool) -> bool {
    panel_open && !default_prevented
}

/// Reactive handle for one room's artifacts.
///
/// Constructed at `RoomsWorkspace` component scope, never inside a rail closure:
/// those closures re-run on every `rooms.access` SSE update, and an in-flight
/// flag rebuilt mid-request would re-enable the save control during its own
/// write — a second create for an id the first one is already taking, or a
/// second amend against a version the first one is already consuming.
#[derive(Clone, Copy)]
pub struct RoomArtifactsState {
    /// Daemon base URL, shared with `Daemon::url` through `Rooms::url` — read
    /// live at request time because bootstrap resolves the origin
    /// asynchronously (a phone via the tunnel resolves it late).
    pub url: RwSignal<String>,
    /// The open room's artifacts, in the daemon's order (most recently updated
    /// first). Not re-sorted here: a second ordering is a second thing to keep
    /// true.
    pub items: RwSignal<Vec<RoomArtifact>>,
    /// Whether a list request has SUCCEEDED for the room now open. Starts false
    /// and returns to false on every room change, so the empty state can never
    /// assert "no artifacts" about a room that has not answered yet.
    pub loaded: RwSignal<bool>,
    /// A list request is in flight.
    pub loading: RwSignal<bool>,
    /// The most recent failure, read or write.
    pub error: RwSignal<Option<String>>,
    /// A write is in flight — blocks re-submit and drives the button label.
    pub saving: RwSignal<bool>,
    /// Whether the panel is open, and what it is showing.
    panel: RwSignal<Option<Focus>>,
    /// The rail control that opens the panel, so closing it can hand focus
    /// back — the workspace's Escape ladder does the same for the members
    /// chip, and a key that leaves focus on a removed node strands a reader
    /// who is not using a mouse.
    open_ref: NodeRef<leptos::html::Button>,
    /// The version the open editor was loaded against. This, NOT the version on
    /// the latest list row, is what an amend presents — see the module note on
    /// `expected_version`.
    base_version: RwSignal<u64>,
    /// A live compare-and-swap refusal: the version the editor holds and the
    /// version that now stands. Kept apart from `error` because it is the one
    /// refusal with a recovery attached, and the panel renders that recovery.
    conflict: RwSignal<Option<(u64, u64)>>,
    /// Editor fields, shared by the create form and the amend form.
    draft_title: RwSignal<String>,
    draft_body: RwSignal<String>,
    draft_kind: RwSignal<String>,
    draft_state: RwSignal<String>,
    /// Monotonic ticket; only the latest overlapping read may publish.
    ticket: RwSignal<u64>,
}

impl RoomArtifactsState {
    pub fn new(rooms: &Rooms) -> Self {
        Self {
            url: rooms.url,
            items: RwSignal::new(Vec::new()),
            loaded: RwSignal::new(false),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            saving: RwSignal::new(false),
            panel: RwSignal::new(None),
            open_ref: NodeRef::new(),
            base_version: RwSignal::new(0),
            conflict: RwSignal::new(None),
            draft_title: RwSignal::new(String::new()),
            draft_body: RwSignal::new(String::new()),
            draft_kind: RwSignal::new(ARTIFACT_KINDS[0].to_string()),
            draft_state: RwSignal::new(ARTIFACT_STATES[0].to_string()),
            ticket: RwSignal::new(0),
        }
    }

    /// Whether the panel is on screen. Public because the Escape ladder that
    /// owns the key lives in `rooms_workspace`, not here.
    pub fn panel_is_open(&self) -> bool {
        self.panel.get_untracked().is_some()
    }

    /// Close the panel and hand focus back to the control that opened it. The
    /// conflict goes with it: a stale-version banner belongs to the editor that
    /// earned it, and one restored over a later visit describes a write nobody
    /// on screen issued.
    pub fn close_panel(&self) {
        self.panel.set(None);
        self.conflict.set(None);
        if let Some(open) = self.open_ref.get_untracked() {
            let _ = open.focus();
        }
    }

    fn base(&self) -> String {
        self.url.get_untracked().trim_end_matches('/').to_string()
    }

    /// Retire whatever is on screen and whatever is in flight.
    ///
    /// The ticket bump is the load-bearing half: without it the previous room's
    /// list could still land and be read under this room's name. The panel goes
    /// with it — an editor left open across a room change would submit the old
    /// room's artifact id against the new room's key, and `saving` would leave
    /// the new room's save control disabled over a write it never issued.
    fn reset(&self) {
        self.ticket
            .update(|ticket| *ticket = ticket.wrapping_add(1));
        self.items.set(Vec::new());
        self.loaded.set(false);
        self.loading.set(false);
        self.error.set(None);
        self.saving.set(false);
        self.panel.set(None);
        self.conflict.set(None);
        self.clear_draft();
    }

    fn clear_draft(&self) {
        self.base_version.set(0);
        self.draft_title.set(String::new());
        self.draft_body.set(String::new());
        self.draft_kind.set(ARTIFACT_KINDS[0].to_string());
        self.draft_state.set(ARTIFACT_STATES[0].to_string());
    }

    /// Load the open room's artifact list.
    pub fn fetch(&self, key: String) {
        let base = self.base();
        let me = *self;
        let ticket = self.ticket.get_untracked().wrapping_add(1);
        self.ticket.set(ticket);
        self.loading.set(true);
        self.error.set(None);
        spawn_local(async move {
            let url = list_url(&base, &key);
            let result = match Request::get(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    match resp.json::<ListBody>().await {
                        Ok(body) => classify_list(status, body),
                        Err(err) => Err(format!("Artifacts decode error: {err}")),
                    }
                }
                Err(err) => Err(format!("Artifacts request failed: {err}")),
            };
            let current = read_is_current(ticket, me.ticket.get_untracked());
            me.publish_list(result, current);
        });
    }

    /// Publish a completed list read — but only the latest one.
    ///
    /// `is_current` is the caller's ticket check, taken as an argument so the
    /// refusal itself is reachable from a test with no browser runtime.
    fn publish_list(&self, result: Result<Vec<RoomArtifact>, String>, is_current: bool) {
        if !is_current {
            return;
        }
        self.loading.set(false);
        match result {
            // Only a SUCCESS may declare the list known. A failed read that
            // flipped this would replace an honest error with the false claim
            // that the room has no artifacts.
            Ok(items) => {
                self.items.set(items);
                self.loaded.set(true);
            }
            Err(error) => self.error.set(Some(error)),
        }
    }

    /// Re-read one artifact after a conflict and reload the editor onto it.
    ///
    /// This is the other half of the daemon's 409 contract. Without it the only
    /// recovery from a stale version is re-listing the whole room; with it the
    /// conflict → re-read → retry loop is one round trip, which is exactly why
    /// the single-artifact route exists.
    ///
    /// It overwrites the draft, so it is only ever reached from a control the
    /// author pressed after being told what it will cost them.
    fn reread(&self, rooms: Rooms, key: String, artifact_id: String) {
        let base = self.base();
        let me = *self;
        let generation = rooms.generation_snapshot();
        self.saving.set(true);
        self.error.set(None);
        spawn_local(async move {
            let url = artifact_url(&base, &key, &artifact_id);
            let read = match Request::get(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    match resp.json::<ArtifactBody>().await {
                        Ok(body) => classify_read(status, body),
                        Err(err) => Err(format!("Artifact decode error: {err}")),
                    }
                }
                Err(err) => Err(format!("Artifact request failed: {err}")),
            };
            me.publish_reread(read, rooms.room_is_current(generation, &key));
        });
    }

    /// Publish a completed re-read — but only into the room that started it.
    fn publish_reread(&self, read: Result<Option<RoomArtifact>, String>, room_is_current: bool) {
        if !room_is_current {
            return;
        }
        self.saving.set(false);
        match read {
            Ok(Some(artifact)) => {
                self.conflict.set(None);
                // Keep the list honest about the row the editor now holds
                // without paying for a second round trip.
                self.upsert(&artifact);
                self.load_editor(&artifact);
            }
            // The row is gone. Say so and close the editor rather than leave an
            // amend form pointed at an id the room no longer has.
            Ok(None) => {
                self.conflict.set(None);
                self.panel.set(Some(Focus::List));
                self.clear_draft();
                self.error
                    .set(Some("That artifact is no longer in this room.".to_string()));
            }
            // The conflict SURVIVES a failed re-read. Clearing it would leave an
            // author who cannot reach the other version looking at an editor
            // that appears ready to save — over a write the store will refuse.
            Err(error) => self.error.set(Some(error)),
        }
    }

    /// Fold a freshly written row into the list without waiting for a re-list.
    ///
    /// The panel renders an open artifact's prose FROM the list, so a create
    /// whose row only arrives with the next `fetch` would leave the author
    /// looking at the empty half of the thing they just wrote. A new row goes to
    /// the front because the store orders by `updated_at DESC` and nothing is
    /// more recently updated than this; the re-list confirms it.
    fn upsert(&self, artifact: &RoomArtifact) {
        self.items.update(
            |items| match items.iter_mut().find(|item| item.id == artifact.id) {
                Some(slot) => *slot = artifact.clone(),
                None => items.insert(0, artifact.clone()),
            },
        );
    }

    /// Point the editor at an artifact as it now stands.
    fn load_editor(&self, artifact: &RoomArtifact) {
        self.panel.set(Some(Focus::Open(artifact.id.clone())));
        self.base_version.set(artifact.version);
        self.draft_title.set(artifact.title.clone());
        self.draft_body.set(artifact.body.clone());
        self.draft_state.set(artifact.state.clone());
    }

    /// Create one artifact and publish what came back.
    ///
    /// `rooms` is taken so the completion can re-validate the `(generation, key)`
    /// pair it started with — the list ticket cannot stand in for it, because
    /// the success arm calls `fetch`, which mints a FRESH ticket that would then
    /// admit the wrong room's answer.
    fn create(&self, rooms: Rooms, key: String, author_id: String) {
        let title = self.draft_title.get_untracked();
        let id = slug_id(&title);
        if let Some(refusal) = create_refusal(&id, &title) {
            self.error.set(Some(refusal));
            return;
        }
        let body = self.draft_body.get_untracked();
        let kind = self.draft_kind.get_untracked();
        let base = self.base();
        let me = *self;
        let generation = rooms.generation_snapshot();
        self.begin_write();
        spawn_local(async move {
            let url = list_url(&base, &key);
            let payload = CreateRequest {
                id: &id,
                kind: &kind,
                title: title.trim(),
                body: &body,
                author_id: &author_id,
            };
            let outcome = post_artifact(&url, &payload).await;
            let is_current = rooms.room_is_current(generation, &key);
            if me.publish_write(outcome, is_current) {
                me.fetch(key);
            }
        });
    }

    /// Decide what an amend of `artifact_id` would put on the wire, or refuse it
    /// in the words its author reads.
    ///
    /// Separated from `amend` for the same reason `publish_write` takes its room
    /// check as an argument: `amend` ends in `spawn_local`, a wasm-only import
    /// no native test can drive, and the version this presents is the one thing
    /// in the module a wrong answer would lose somebody's work over.
    fn amend_plan(&self, artifact_id: &str) -> Result<AmendPlan, String> {
        // The row the delta is computed against. `upsert` keeps the list ahead
        // of the editor on every write, so this is present whenever an editor is
        // open — but a silent return here would be an unexplained dead control,
        // and this panel's whole job is explaining refusals.
        let Some(current) = self
            .items
            .get_untracked()
            .into_iter()
            .find(|item| item.id == artifact_id)
        else {
            return Err("That artifact is no longer in this room.".to_string());
        };
        let title = self.draft_title.get_untracked();
        let body = self.draft_body.get_untracked();
        let state = self.draft_state.get_untracked();
        if title.trim().is_empty() {
            return Err("An artifact needs a title.".to_string());
        }
        // A no-op amend is a 400 at the store, deliberately, and unreadable
        // prose here. Caught against the row that stands.
        let Some((title, body, state)) = amend_delta(&current, &title, &body, &state) else {
            return Err("Nothing changed.".to_string());
        };
        Ok(AmendPlan {
            // `base_version`, NOT `current.version`. The editor's version is the
            // one its author actually read; the list's may have moved under them
            // since, and presenting THAT is a write the compare-and-swap admits
            // and nobody ever saw.
            expected_version: self.base_version.get_untracked(),
            title: title.map(str::to_string),
            body: body.map(str::to_string),
            state: state.map(str::to_string),
        })
    }

    /// Amend one artifact under compare-and-swap.
    fn amend(&self, rooms: Rooms, key: String, artifact_id: String, author_id: String) {
        let plan = match self.amend_plan(&artifact_id) {
            Ok(plan) => plan,
            Err(refusal) => {
                self.error.set(Some(refusal));
                return;
            }
        };
        let base = self.base();
        let me = *self;
        let generation = rooms.generation_snapshot();
        self.begin_write();
        spawn_local(async move {
            let url = amend_url(&base, &key, &artifact_id);
            let payload = AmendRequest {
                expected_version: plan.expected_version,
                title: plan.title.as_deref(),
                body: plan.body.as_deref(),
                state: plan.state.as_deref(),
                author_id: &author_id,
            };
            let outcome = post_artifact(&url, &payload).await;
            let is_current = rooms.room_is_current(generation, &key);
            if me.publish_write(outcome, is_current) {
                me.fetch(key);
            }
        });
    }

    /// Take the state into a write. The conflict is retired with the error: a
    /// stale-version banner left standing over a fresh submit tells its author
    /// the write they are watching already failed.
    fn begin_write(&self) {
        self.saving.set(true);
        self.error.set(None);
        self.conflict.set(None);
    }

    /// Publish a completed write — but only into the room that started it.
    ///
    /// `room_is_current` is the caller's `(generation, key)` re-validation, taken
    /// as an argument rather than recomputed here so the refusal is reachable
    /// from a test with no live `Rooms` and no browser runtime. When it is false
    /// there is nothing to write: `reset` has already cleared this state for
    /// whoever is on screen now, and the artifact still landed in the room it
    /// was meant for — reopening that room lists it.
    ///
    /// Returns whether the caller should re-list. The `fetch` is deliberately
    /// NOT issued from here: `spawn_local` is a wasm-only import, so a publish
    /// that spawned could not be driven from a native test at all — and this is
    /// the function holding the conflict and stale-room refusals that most need
    /// one.
    #[must_use]
    fn publish_write(&self, outcome: WriteOutcome, is_current: bool) -> bool {
        if !is_current {
            return false;
        }
        self.saving.set(false);
        match outcome {
            WriteOutcome::Wrote(artifact) => {
                // The reply carries the whole row, so the editor is re-pointed
                // at it without a round trip; the list is still re-read because
                // a create changes the room's ordering and an amend moves its
                // row to the front of it. `load_editor` runs BEFORE that
                // re-list so the fresh `base_version` is the one this write
                // produced — the list reply is a second answer to the same
                // question and must not be what the next amend presents.
                self.upsert(&artifact);
                self.load_editor(&artifact);
                true
            }
            // Never a silent overwrite and never a bare "409": the author is
            // told which version now stands, and the re-read that gets them
            // there is one control away.
            WriteOutcome::Conflict { expected, actual } => {
                self.conflict.set(Some((expected, actual)));
                false
            }
            WriteOutcome::Failure(error) => {
                self.error.set(Some(error));
                false
            }
        }
    }
}

/// Issue one artifact write and classify the reply.
///
/// Shared by create and amend because the reply envelope is identical and the
/// three failure seams — encode, transport, decode — must read the same way for
/// both. A decode failure is a real transport fault: every body on these routes
/// is JSON the daemon shapes itself.
async fn post_artifact<T: Serialize>(url: &str, payload: &T) -> WriteOutcome {
    match Request::post(url)
        .header("content-type", "application/json")
        .json(payload)
    {
        Ok(request) => match request.send().await {
            Ok(resp) => {
                let status = resp.status();
                match resp.json::<ArtifactBody>().await {
                    Ok(body) => classify_write(status, body),
                    Err(err) => WriteOutcome::Failure(format!("Artifact decode error: {err}")),
                }
            }
            Err(err) => WriteOutcome::Failure(format!("Artifact request failed: {err}")),
        },
        Err(err) => WriteOutcome::Failure(format!("Artifact encode error: {err}")),
    }
}

// ---- Component --------------------------------------------------------------

/// The open room's artifacts: a compact rail list, and a panel where they are
/// actually read and written.
///
/// The rail deliberately holds only the list. The right rail is 220px wide and
/// already carries the roster, the summary and the files; a create form and a
/// body editor squeezed in beside them would be a fifth thing nobody can read.
/// Everything that needs a reading measure — the prose, the version, the
/// compare-and-swap story — lives in the panel.
///
/// `writes_allowed` is supplied by the workspace rather than recomputed here so
/// this control and the composer can never disagree about the same room's access
/// projection. `members` is the same roster memo the transcript renders against,
/// so an `@id` means the same thing in an artifact as in a message.
#[component]
pub fn RoomArtifacts(
    rooms: Rooms,
    state: RoomArtifactsState,
    writes_allowed: Signal<bool>,
    members: Memo<HashSet<String>>,
) -> impl IntoView {
    // Follow the open room. Clearing FIRST is what stops the previous room's
    // artifacts from being read, however briefly, under this room's name.
    Effect::new(move |_| match rooms.open_key.get() {
        Some(key) if !key.is_empty() => {
            state.reset();
            state.fetch(key);
        }
        _ => state.reset(),
    });

    // Identity is deliberately NOT part of this predicate. `identity_resolved`
    // reads its signals untracked, so a control disabled on it would never
    // re-enable when bootstrap answers; the composer gates on access here and
    // refuses on identity at the action, and these controls do the same.
    let can_write = move || {
        writes_allowed.get()
            && !state.saving.get()
            && rooms.open_key.get().is_some_and(|key| !key.is_empty())
    };

    // The one place an action resolves the room key and the author id together.
    // Both refusals are the composer's, in the composer's words.
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

    view! {
        <div class="rooms-workspace__artifacts">
            <div class="rooms-workspace__artifacts-head">
                <span class="rooms-workspace__artifacts-title">"Artifacts"</span>
                <button
                    class="rooms-workspace__artifacts-open"
                    type="button"
                    node_ref=state.open_ref
                    title="Open this room's tasks, decisions and notes"
                    disabled=move || {
                        rooms.open_key.get().is_none_or(|key| key.is_empty())
                    }
                    on:click=move |_| {
                        state.error.set(None);
                        state.panel.set(Some(Focus::List));
                    }
                >
                    "open"
                </button>
            </div>

            // Also rendered in the panel, and deliberately in both places: a
            // list that failed while the panel was closed would otherwise show
            // as a rail holding nothing at all, which reads as a room that
            // produced nothing rather than a read that did not answer.
            {move || {
                state.error.get().map(|error| view! {
                    <div class="rooms-workspace__artifacts-error" role="alert">{error}</div>
                })
            }}

            // The compare-and-swap refusal follows its author out of the editor.
            // A 409 that lands after they stepped back to the list renders
            // nowhere otherwise: `begin_write` cleared `error` and never re-set
            // it, and the recovery block below is inside the editor they left.
            // A write the store REFUSED reading as a write that saved is the one
            // outcome this whole panel exists to prevent.
            {move || {
                let focus = state.panel.get();
                state
                    .conflict
                    .get()
                    .filter(|_| conflict_needs_its_own_line(focus.as_ref()))
                    .map(|(expected, actual)| view! {
                        <div class="rooms-workspace__artifacts-error" role="alert">
                            {conflict_message(expected, actual)}
                        </div>
                    })
            }}

            {move || {
                // Order matters: in-flight and never-answered both outrank the
                // empty state, which may only speak for a room that replied.
                if state.loading.get() {
                    return view! {
                        <div class="rooms-workspace__artifacts-note">"Loading artifacts\u{2026}"</div>
                    }.into_any();
                }
                if !state.loaded.get() {
                    return ().into_any();
                }
                let items = state.items.get();
                if items.is_empty() {
                    return view! {
                        <div class="rooms-workspace__artifacts-note">"No artifacts yet."</div>
                    }.into_any();
                }
                let rows = items
                    .into_iter()
                    .map(|artifact| {
                        let dropped = artifact.state == "dropped";
                        let label = artifact_label(&artifact);
                        let meta = artifact_meta(&artifact);
                        let title = format!("{} \u{2014} {meta}", artifact.id);
                        let opened = artifact.clone();
                        view! {
                            <button
                                class="rooms-workspace__artifact"
                                class:rooms-workspace__artifact--dropped=dropped
                                type="button"
                                title=title
                                on:click=move |_| {
                                    state.error.set(None);
                                    state.conflict.set(None);
                                    state.load_editor(&opened);
                                }
                            >
                                <span class="rooms-workspace__artifact-name">{label}</span>
                                <span class="rooms-workspace__artifact-meta">{meta}</span>
                            </button>
                        }
                    })
                    .collect::<Vec<_>>();
                view! {
                    <div
                        class="rooms-workspace__artifacts-list"
                        role="list"
                        aria-label="Room artifacts"
                    >
                        {rows}
                    </div>
                }.into_any()
            }}

            {move || {
                let Some(focus) = state.panel.get() else {
                    return ().into_any();
                };
                // The panel is a fixed modal over its own rail, so the rail's
                // copy of the sentence is behind the scrim while this is open.
                let stranded = conflict_needs_its_own_line(Some(&focus));
                view! {
                    <div class="rooms-workspace__artifacts-scrim" on:click=move |_| {
                        state.close_panel();
                    }></div>
                    // `aria-modal` because the scrim is only paint: without it a
                    // screen reader still walks the rail and the transcript
                    // behind a dialog a sighted reader cannot reach.
                    <div
                        class="rooms-workspace__artifacts-panel"
                        role="dialog"
                        aria-modal="true"
                        aria-label="Room artifacts"
                    >
                        <div class="rooms-workspace__artifacts-panel-head">
                            <span class="rooms-workspace__artifacts-panel-title">
                                {match &focus {
                                    Focus::List => "Artifacts".to_string(),
                                    Focus::New => "New artifact".to_string(),
                                    Focus::Open(id) => id.clone(),
                                }}
                            </span>
                            <button
                                class="rooms-workspace__artifacts-close"
                                type="button"
                                aria-label="Close artifacts"
                                on:click=move |_| state.close_panel()
                            >
                                "\u{d7}"
                            </button>
                        </div>

                        {move || {
                            state.error.get().map(|error| view! {
                                <div class="rooms-workspace__artifacts-error" role="alert">
                                    {error}
                                </div>
                            })
                        }}

                        {move || {
                            state.conflict.get().filter(|_| stranded).map(
                                |(expected, actual)| view! {
                                    <div
                                        class="rooms-workspace__artifacts-error"
                                        role="alert"
                                    >
                                        {conflict_message(expected, actual)}
                                    </div>
                                },
                            )
                        }}

                        <div class="rooms-workspace__artifacts-panel-body">
                            {match focus {
                                Focus::List => panel_list(state, writes_allowed).into_any(),
                                Focus::New => {
                                    panel_new(state, actor, rooms, can_write).into_any()
                                }
                                Focus::Open(id) => {
                                    panel_open(state, actor, rooms, can_write, members, id)
                                        .into_any()
                                }
                            }}
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}

/// The panel's list view: every artifact at a measure the rail cannot give it.
fn panel_list(state: RoomArtifactsState, writes_allowed: Signal<bool>) -> impl IntoView {
    view! {
        <button
            class="rooms-workspace__artifacts-new"
            type="button"
            disabled=move || !writes_allowed.get()
            on:click=move |_| {
                state.error.set(None);
                state.clear_draft();
                state.panel.set(Some(Focus::New));
            }
        >
            "+ new artifact"
        </button>
        {move || {
            let items = state.items.get();
            if items.is_empty() {
                // `loaded` gates the rail's empty state; by the time the panel
                // is open the list has answered or the rail is showing why not.
                return view! {
                    <div class="rooms-workspace__artifacts-note">"No artifacts yet."</div>
                }.into_any();
            }
            items
                .into_iter()
                .map(|artifact| {
                    let dropped = artifact.state == "dropped";
                    let label = artifact_label(&artifact);
                    let meta = artifact_meta(&artifact);
                    let opened = artifact.clone();
                    view! {
                        <button
                            class="rooms-workspace__artifacts-row"
                            class:rooms-workspace__artifacts-row--dropped=dropped
                            type="button"
                            on:click=move |_| {
                                state.error.set(None);
                                state.conflict.set(None);
                                state.load_editor(&opened);
                            }
                        >
                            <span class="rooms-workspace__artifacts-row-name">{label}</span>
                            <span class="rooms-workspace__artifacts-row-meta">{meta}</span>
                        </button>
                    }
                })
                .collect::<Vec<_>>()
                .into_any()
        }}
    }
}

/// The create form. `kind` is fixed at creation because the daemon has no amend
/// path for it — a task that turns out to be a decision is a new artifact.
fn panel_new(
    state: RoomArtifactsState,
    actor: impl Fn() -> Option<(String, String)> + Copy + Send + Sync + 'static,
    rooms: Rooms,
    can_write: impl Fn() -> bool + Copy + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        <label class="rooms-workspace__artifacts-label" for="artifact-kind">"Kind"</label>
        <select
            class="rooms-workspace__artifacts-select"
            id="artifact-kind"
            on:change=move |ev| state.draft_kind.set(event_target_value(&ev))
        >
            {ARTIFACT_KINDS
                .iter()
                .map(|kind| view! {
                    <option
                        value=*kind
                        selected=move || state.draft_kind.get() == *kind
                    >
                        {*kind}
                    </option>
                })
                .collect::<Vec<_>>()}
        </select>

        <label class="rooms-workspace__artifacts-label" for="artifact-title">"Title"</label>
        <input
            class="rooms-workspace__artifacts-input"
            id="artifact-title"
            type="text"
            prop:value=move || state.draft_title.get()
            on:input=move |ev| state.draft_title.set(event_target_value(&ev))
        />
        // The id is derived, not typed: it is the addressing segment of two
        // routes and the daemon refuses an untrimmed one. Showing it is what
        // makes a duplicate-id refusal legible when it arrives.
        <div class="rooms-workspace__artifacts-hint">
            {move || {
                let id = slug_id(&state.draft_title.get());
                if id.is_empty() { String::new() } else { format!("id: {id}") }
            }}
        </div>

        <label class="rooms-workspace__artifacts-label" for="artifact-body">"Body"</label>
        <textarea
            class="rooms-workspace__artifacts-textarea"
            id="artifact-body"
            rows="8"
            prop:value=move || state.draft_body.get()
            on:input=move |ev| state.draft_body.set(event_target_value(&ev))
        ></textarea>

        <div class="rooms-workspace__artifacts-actions">
            <button
                class="rooms-workspace__artifacts-cancel"
                type="button"
                on:click=move |_| {
                    state.error.set(None);
                    state.clear_draft();
                    state.panel.set(Some(Focus::List));
                }
            >
                "cancel"
            </button>
            <button
                class="rooms-workspace__artifacts-save"
                type="button"
                disabled=move || !can_write()
                on:click=move |_| {
                    let Some((key, author)) = actor() else { return };
                    state.create(rooms, key, author);
                }
            >
                {move || if state.saving.get() { "saving\u{2026}" } else { "create" }}
            </button>
        </div>
    }
}

/// One artifact, open: what it says now, and the amend that presents the version
/// it was read at.
fn panel_open(
    state: RoomArtifactsState,
    actor: impl Fn() -> Option<(String, String)> + Copy + Send + Sync + 'static,
    rooms: Rooms,
    can_write: impl Fn() -> bool + Copy + Send + Sync + 'static,
    members: Memo<HashSet<String>>,
    artifact_id: String,
) -> impl IntoView {
    let conflict_id = artifact_id.clone();
    let amend_id = artifact_id.clone();

    view! {
        {move || {
            let artifact_id = artifact_id.clone();
            state
                .items
                .get()
                .into_iter()
                .find(move |item| item.id == artifact_id)
                .map(|artifact| {
                let meta = artifact_meta(&artifact);
                let detail = format!(
                    "created {} by {} \u{b7} updated {} by {}",
                    artifact.created_at,
                    artifact.created_by,
                    artifact.updated_at,
                    artifact.updated_by,
                );
                view! {
                    <div class="rooms-workspace__artifacts-meta" title=detail>{meta}</div>
                    <div class="rooms-workspace__artifacts-body">
                        // Structural rendering only: `body_view` emits Leptos
                        // text nodes inside a fixed element set with no
                        // innerHTML path, so model- and human-written prose
                        // cannot become markup on this origin.
                        {crate::room_markdown::body_view(artifact.body.clone(), members)}
                    </div>
                }
                })
        }}

        {move || {
            let conflict_id = conflict_id.clone();
            state.conflict.get().map(move |(expected, actual)| {
                let conflict_id = conflict_id.clone();
                view! {
                    <div class="rooms-workspace__artifacts-conflict" role="alert">
                        <div>{conflict_message(expected, actual)}</div>
                        <button
                            class="rooms-workspace__artifacts-reread"
                            type="button"
                            title="Load their version \u{2014} this replaces what you typed"
                            disabled=move || state.saving.get()
                            on:click=move |_| {
                                let Some((key, _)) = actor() else { return };
                                state.reread(rooms, key, conflict_id.clone());
                            }
                        >
                            "re-read"
                        </button>
                    </div>
                }
            })
        }}

        <label class="rooms-workspace__artifacts-label" for="artifact-edit-title">"Title"</label>
        <input
            class="rooms-workspace__artifacts-input"
            id="artifact-edit-title"
            type="text"
            prop:value=move || state.draft_title.get()
            on:input=move |ev| state.draft_title.set(event_target_value(&ev))
        />

        <label class="rooms-workspace__artifacts-label" for="artifact-edit-state">"State"</label>
        <select
            class="rooms-workspace__artifacts-select"
            id="artifact-edit-state"
            on:change=move |ev| state.draft_state.set(event_target_value(&ev))
        >
            {ARTIFACT_STATES
                .iter()
                .map(|lifecycle| view! {
                    <option
                        value=*lifecycle
                        selected=move || state.draft_state.get() == *lifecycle
                    >
                        {*lifecycle}
                    </option>
                })
                .collect::<Vec<_>>()}
        </select>

        <label class="rooms-workspace__artifacts-label" for="artifact-edit-body">"Body"</label>
        <textarea
            class="rooms-workspace__artifacts-textarea"
            id="artifact-edit-body"
            rows="8"
            prop:value=move || state.draft_body.get()
            on:input=move |ev| state.draft_body.set(event_target_value(&ev))
        ></textarea>

        <div class="rooms-workspace__artifacts-actions">
            // The version this amend will present. Shown because it is the whole
            // compare-and-swap contract: if it is not the version that stands,
            // the write is refused rather than merged.
            <span class="rooms-workspace__artifacts-version">
                {move || format!("editing v{}", state.base_version.get())}
            </span>
            <button
                class="rooms-workspace__artifacts-cancel"
                type="button"
                on:click=move |_| {
                    state.error.set(None);
                    state.conflict.set(None);
                    state.clear_draft();
                    state.panel.set(Some(Focus::List));
                }
            >
                "back"
            </button>
            <button
                class="rooms-workspace__artifacts-save"
                type="button"
                disabled=move || !can_write()
                on:click={
                    let amend_id = amend_id.clone();
                    move |_| {
                        let Some((key, author)) = actor() else { return };
                        state.amend(rooms, key, amend_id.clone(), author);
                    }
                }
            >
                // `saving` also covers the re-read that recovers from a conflict,
            // and this button is on screen throughout it. One word that is true
            // of both beats a label that lies during one of them.
            {move || if state.saving.get() { "working\u{2026}" } else { "save" }}
            </button>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state as `new` leaves it, for the tests that drive one directly.
    fn fresh_state() -> RoomArtifactsState {
        RoomArtifactsState {
            url: RwSignal::new("http://d".to_string()),
            items: RwSignal::new(Vec::new()),
            loaded: RwSignal::new(false),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            saving: RwSignal::new(false),
            panel: RwSignal::new(None),
            open_ref: NodeRef::new(),
            base_version: RwSignal::new(0),
            conflict: RwSignal::new(None),
            draft_title: RwSignal::new(String::new()),
            draft_body: RwSignal::new(String::new()),
            draft_kind: RwSignal::new(ARTIFACT_KINDS[0].to_string()),
            draft_state: RwSignal::new(ARTIFACT_STATES[0].to_string()),
            ticket: RwSignal::new(0),
        }
    }

    /// The daemon's own serialization of `ocean_core::RoomArtifact`, field for
    /// field: `kind` and `state` are snake_case strings, `version` starts at 1,
    /// and `on_behalf_of` is absent when a human authored directly
    /// (`skip_serializing_if = "Option::is_none"`).
    fn artifact_json(id: &str, kind: &str, state: &str, version: u64) -> String {
        format!(
            r#"{{"id":"{id}","kind":"{kind}","title":"Ship the proxy",
                "body":"Land it before Friday.","state":"{state}",
                "created_by":"smaths","created_at":"2026-08-01T09:00:00+00:00",
                "updated_by":"smaths","updated_at":"2026-08-25T10:00:00+00:00",
                "version":{version}}}"#
        )
    }

    fn list_body(json: &str) -> ListBody {
        serde_json::from_str(json).expect("decode")
    }

    fn artifact_body(json: &str) -> ArtifactBody {
        serde_json::from_str(json).expect("decode")
    }

    fn decoded(id: &str, kind: &str, state: &str, version: u64) -> RoomArtifact {
        serde_json::from_str(&artifact_json(id, kind, state, version)).expect("decode")
    }

    #[test]
    fn every_route_segment_is_encoded_once() {
        // A key or an id with a slash must never become two segments and reach
        // a different daemon route.
        assert_eq!(
            list_url("http://d", "call:xyz"),
            "http://d/v1/rooms/persistent/call%3Axyz/artifacts"
        );
        assert_eq!(
            artifact_url("http://d", "a/b", "task/one"),
            "http://d/v1/rooms/persistent/a%2Fb/artifacts/task%2Fone"
        );
        assert_eq!(
            amend_url("http://d", "a/b", "task/one"),
            "http://d/v1/rooms/persistent/a%2Fb/artifacts/task%2Fone/amend"
        );
    }

    #[test]
    fn the_create_body_carries_exactly_the_five_fields_the_daemon_names() {
        // `CreateArtifactRequest` is `deny_unknown_fields`, so one extra key is
        // a 400 the operator could do nothing about — and `state` is NOT one of
        // its fields, however natural it looks to send.
        let json = serde_json::to_string(&CreateRequest {
            id: "ship-the-proxy",
            kind: "task",
            title: "Ship the proxy",
            body: "Land it before Friday.",
            author_id: "smaths",
        })
        .expect("encode");
        assert_eq!(
            json,
            r#"{"id":"ship-the-proxy","kind":"task","title":"Ship the proxy","body":"Land it before Friday.","author_id":"smaths"}"#
        );
    }

    #[test]
    fn a_lifecycle_only_amend_sends_no_prose_it_is_not_editing() {
        // `title`/`body`/`state` are each `Option` on the daemon and each
        // resolves to what stands when omitted. Sending the unchanged body back
        // would work, but it also means a concurrent body edit this editor never
        // saw gets overwritten by the text it happens to be holding.
        let json = serde_json::to_string(&AmendRequest {
            expected_version: 3,
            title: None,
            body: None,
            state: Some("done"),
            author_id: "smaths",
        })
        .expect("encode");
        assert_eq!(
            json,
            r#"{"expected_version":3,"state":"done","author_id":"smaths"}"#
        );
    }

    #[test]
    fn the_list_is_published_in_the_daemons_order_and_nothing_is_filtered_out() {
        // `ORDER BY updated_at DESC, artifact_id` is the store's ordering and
        // this side does not re-derive it. The room's summary is a real artifact
        // with a real id: the rail renders it separately, but a list that
        // quietly omits rows stops matching the room it describes.
        let json = format!(
            r#"{{"ok":true,"artifacts":[{},{},{}]}}"#,
            artifact_json("ship-the-proxy", "task", "open", 2),
            artifact_json("room-summary", "note", "open", 7),
            artifact_json("use-sqlite", "decision", "done", 1),
        );
        let items = classify_list(200, list_body(&json)).expect("a list");
        let ids: Vec<&str> = items.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["ship-the-proxy", "room-summary", "use-sqlite"]);
        assert_eq!(items[2].kind, "decision");
        assert_eq!(items[2].state, "done");
    }

    #[test]
    fn a_stale_amend_surfaces_the_version_that_stands_instead_of_overwriting() {
        // The whole point of the compare-and-swap. The daemon hands back BOTH
        // versions precisely so the recovery is "re-read at `actual`", and a
        // client that merged, or retried at `actual`, would be last-writer-wins
        // — the bug the `version` column exists to refuse.
        let body = artifact_body(
            r#"{"ok":false,"code":"artifact_version_conflict",
                "expected_version":3,"actual_version":5,
                "error":"artifact is at version 5, not 3; re-read and retry"}"#,
        );
        let outcome = classify_write(409, body);
        assert_eq!(
            outcome,
            WriteOutcome::Conflict {
                expected: 3,
                actual: 5
            }
        );
        let WriteOutcome::Conflict { expected, actual } = outcome else {
            unreachable!()
        };
        // And the sentence names the version to re-read from, not the status.
        let sentence = conflict_message(expected, actual);
        assert!(sentence.contains("v5"), "{sentence}");
        assert!(!sentence.contains("409"), "{sentence}");
    }

    #[test]
    fn a_duplicate_id_409_is_not_mistaken_for_a_version_conflict() {
        // `ArtifactAlreadyExists` is also a 409, and it carries no versions.
        // Reading it as a CAS conflict would offer a re-read of an artifact the
        // caller never had open.
        let body = artifact_body(
            r#"{"ok":false,"error":"room 'r' already has an artifact 'ship-the-proxy'"}"#,
        );
        match classify_write(409, body) {
            WriteOutcome::Failure(message) => {
                assert!(message.contains("already has an artifact"), "{message}")
            }
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn a_forged_agent_author_is_explained_rather_than_echoed() {
        // The daemon gates create on the roster kind: an agent's artifact is
        // written by the daemon, never by a client claiming its identity. The
        // control that fires this is held by a permitted identity, so the
        // sentence has to say what actually happened or it reads as a bug.
        let body = artifact_body(
            r#"{"ok":false,"code":"forged_artifact_author",
                "error":"an agent's artifact is authored by the daemon, not by a client claiming its identity"}"#,
        );
        match classify_write(403, body) {
            WriteOutcome::Failure(message) => {
                assert!(message.contains("written by the daemon"), "{message}");
                assert!(!message.contains("forged"), "{message}");
            }
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn a_bare_invalid_request_is_not_shown_to_an_operator_as_a_token() {
        // `invalid_request_response` sends `{"ok":false,"error":"invalid_request"}`
        // with no `code` at all, so the fallback arm is what an operator reads.
        let body = artifact_body(r#"{"ok":false,"error":"invalid_request"}"#);
        match classify_write(400, body) {
            WriteOutcome::Failure(message) => {
                assert_eq!(message, "The room refused that artifact.")
            }
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_artifact_404_is_an_answer_but_an_unknown_room_404_is_not() {
        // `unknown_artifact` is the daemon's ONLY coded 404 on this route. The
        // unknown ROOM answers 404 through `room_store_error_response` with no
        // `code`, and reading that as a missing row would say a room that is
        // GONE merely lost one artifact.
        let gone = artifact_body(
            r#"{"ok":false,"code":"unknown_artifact","error":"room 'r' has no artifact 'x'"}"#,
        );
        assert_eq!(classify_read(404, gone), Ok(None));

        let no_room = artifact_body(r#"{"ok":false,"error":"unknown room 'r'"}"#);
        assert!(classify_read(404, no_room).is_err());
    }

    #[test]
    fn an_ok_write_carrying_no_artifact_is_refused_rather_than_believed() {
        // A shape the daemon does not emit. Believing it would let an "ok" with
        // nothing in it stand in for the row that was written.
        let body = artifact_body(r#"{"ok":true}"#);
        match classify_write(201, body) {
            WriteOutcome::Failure(message) => {
                assert!(message.contains("sent nothing"), "{message}")
            }
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn an_amend_that_changes_nothing_never_reaches_the_wire() {
        // The store refuses a no-op amend with a 400, deliberately: bumping the
        // version on a no-op writes a transcript line claiming somebody changed
        // something they did not, and invalidates every other writer's
        // `expected_version`. Caught here, the author is told why.
        let current = decoded("ship-the-proxy", "task", "open", 2);
        assert_eq!(
            amend_delta(&current, "Ship the proxy", "Land it before Friday.", "open"),
            None
        );
        // A surrounding-whitespace-only title edit is also a no-op: the daemon
        // stores what the create trimmed, so an editor pre-filled from it and
        // re-submitted with a stray space must not look like a change.
        assert_eq!(
            amend_delta(
                &current,
                "  Ship the proxy  ",
                "Land it before Friday.",
                "open"
            ),
            None
        );
        // But a lifecycle move is a real change, and only that field goes out.
        assert_eq!(
            amend_delta(&current, "Ship the proxy", "Land it before Friday.", "done"),
            Some((None, None, Some("done")))
        );
    }

    #[test]
    fn an_amend_presents_the_version_the_editor_opened_against() {
        // The bug this refuses: a background list refresh lands mid-edit
        // carrying v5, the editor still shows v2's prose, and an amend that
        // presented v5 would be ADMITTED by the compare-and-swap and silently
        // overwrite the change its author never saw. The plan is read rather
        // than the wire because `amend` ends in `spawn_local`, which no native
        // test can drive — the payload decision is extracted for exactly that.
        let state = fresh_state();
        let opened = decoded("ship-the-proxy", "task", "open", 2);
        state.items.set(vec![opened.clone()]);
        state.load_editor(&opened);
        assert_eq!(state.base_version.get_untracked(), 2);

        // Someone else amends; the list refresh publishes their v5 row under
        // the open editor. The author then moves the lifecycle and saves.
        state
            .items
            .set(vec![decoded("ship-the-proxy", "task", "open", 5)]);
        state.draft_state.set("done".to_string());

        let plan = state
            .amend_plan("ship-the-proxy")
            .expect("a lifecycle move is a real change");
        assert_eq!(
            plan.expected_version, 2,
            "the version the editor read goes out, not the one the list moved to"
        );
        assert_eq!(plan.state.as_deref(), Some("done"));
        assert_eq!(plan.title, None, "an unchanged field is left alone");
        assert_eq!(plan.body, None);
    }

    #[test]
    fn an_amend_the_store_would_refuse_is_refused_here_with_a_reason() {
        // Each of these is a 400 or a dead control at the wire. Refused before
        // the write so the author reads a cause instead of a status code.
        let state = fresh_state();
        let opened = decoded("ship-the-proxy", "task", "open", 2);
        state.items.set(vec![opened.clone()]);
        state.load_editor(&opened);

        assert_eq!(
            state.amend_plan("gone-from-the-room"),
            Err("That artifact is no longer in this room.".to_string())
        );

        state.draft_title.set("   ".to_string());
        assert_eq!(
            state.amend_plan("ship-the-proxy"),
            Err("An artifact needs a title.".to_string())
        );

        state.draft_title.set(opened.title.clone());
        assert_eq!(
            state.amend_plan("ship-the-proxy"),
            Err("Nothing changed.".to_string())
        );
    }

    #[test]
    fn a_refused_write_is_shown_wherever_its_author_is_standing() {
        // The swallow this refuses: submit an amend, step back to the list, and
        // the 409 lands into a state nothing renders — `begin_write` cleared
        // `error` and never re-set it, and the recovery block lives inside the
        // editor that was left. An un-erroring UI over a write the store
        // REFUSED is a refusal that reads as a save.
        let state = fresh_state();
        let opened = decoded("ship-the-proxy", "task", "open", 2);
        state.items.set(vec![opened.clone()]);
        state.load_editor(&opened);
        state.begin_write();
        // Back to the list, mid-write.
        state.panel.set(Some(Focus::List));

        assert!(!state.publish_write(
            WriteOutcome::Conflict {
                expected: 2,
                actual: 5
            },
            true
        ));
        assert_eq!(
            state.error.get_untracked(),
            None,
            "a conflict is not an error"
        );
        assert_eq!(state.conflict.get_untracked(), Some((2, 5)));
        assert!(
            conflict_needs_its_own_line(state.panel.get_untracked().as_ref()),
            "the list has no conflict banner of its own, so it must carry the sentence"
        );

        // Closed entirely — the rail is then the only thing on screen, and it
        // says it too.
        state.panel.set(None);
        assert!(conflict_needs_its_own_line(None));
        // The create form is no better a place to hide it.
        assert!(conflict_needs_its_own_line(Some(&Focus::New)));
        // Only the open editor is exempt: it renders the full refusal WITH the
        // re-read that recovers from it, and two copies would read as two.
        assert!(!conflict_needs_its_own_line(Some(&Focus::Open(
            "ship-the-proxy".to_string()
        ))));
    }

    #[test]
    fn the_panel_consumes_escape_before_anything_underneath_it() {
        // The panel is `position: fixed` at z-index 445, above the members
        // drawer (430) and its backdrop (425). Left out of the workspace's
        // ladder, Escape would close the drawer UNDER an open modal or fall
        // through to the app rail and tear down the rooms surface with an
        // unsaved draft inside it.
        assert!(artifacts_escape_closes(true, false));
        // Nothing to close.
        assert!(!artifacts_escape_closes(false, false));
        // Something nearer the key already answered it.
        assert!(!artifacts_escape_closes(true, true));
    }

    #[test]
    fn closing_the_panel_retires_the_conflict_it_was_showing() {
        // A banner restored over a later visit describes a write nobody on
        // screen issued.
        let state = fresh_state();
        state.panel.set(Some(Focus::List));
        state.conflict.set(Some((2, 5)));
        assert!(state.panel_is_open());

        state.close_panel();
        assert!(!state.panel_is_open());
        assert_eq!(state.conflict.get_untracked(), None);
    }

    #[test]
    fn only_the_latest_list_read_may_publish() {
        // An older completion publishing over a newer one is what put a
        // premature empty state on screen in three previous features. The
        // predicate is taken as an argument so this refusal is reachable
        // without a browser.
        let state = fresh_state();
        state
            .items
            .set(vec![decoded("ship-the-proxy", "task", "open", 2)]);
        state.loaded.set(true);

        state.publish_list(Ok(Vec::new()), read_is_current(3, 4));
        assert_eq!(
            state.items.get_untracked().len(),
            1,
            "a stale read must not blank the list it lost the race to"
        );

        state.publish_list(Ok(Vec::new()), read_is_current(4, 4));
        assert!(state.items.get_untracked().is_empty());
    }

    #[test]
    fn a_failed_list_never_claims_the_room_has_no_artifacts() {
        // Only a SUCCESS may declare the list known. A failed read that flipped
        // `loaded` would replace an honest error with the false claim that the
        // room produced nothing.
        let state = fresh_state();
        state.publish_list(Err("Artifacts request failed".to_string()), true);
        assert!(!state.loaded.get_untracked());
        assert!(state.error.get_untracked().is_some());
    }

    #[test]
    fn a_write_that_lands_after_a_room_change_publishes_nothing() {
        // `reset` has already cleared this state for whoever is on screen now,
        // and the artifact still landed in the room it was meant for. Publishing
        // it here would put one room's artifact under another room's name.
        let state = fresh_state();
        let artifact = decoded("ship-the-proxy", "task", "open", 1);
        state.saving.set(true);

        assert!(
            !state.publish_write(WriteOutcome::Wrote(Box::new(artifact)), false),
            "a refused publish must not ask for a re-list under the new room's key"
        );
        assert!(state.items.get_untracked().is_empty());
        assert!(state.panel.get_untracked().is_none());
        assert!(
            state.saving.get_untracked(),
            "a refused publish must not touch the flag the live room owns"
        );
    }

    #[test]
    fn a_created_artifact_is_readable_before_the_re_list_lands() {
        // The panel renders an open artifact's prose FROM the list, so a create
        // whose row only arrived with the next `fetch` would leave the author
        // looking at the empty half of the thing they just wrote — and an amend
        // issued in that window would find no row to compute a delta against.
        let state = fresh_state();
        state
            .items
            .set(vec![decoded("use-sqlite", "decision", "done", 1)]);
        let made = decoded("ship-the-proxy", "task", "open", 1);

        assert!(state.publish_write(WriteOutcome::Wrote(Box::new(made)), true));

        let items = state.items.get_untracked();
        assert_eq!(
            items.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            ["ship-the-proxy", "use-sqlite"],
            "the new row leads, as `updated_at DESC` will confirm"
        );
        assert_eq!(state.base_version.get_untracked(), 1);
        assert_eq!(
            state.panel.get_untracked(),
            Some(Focus::Open("ship-the-proxy".to_string()))
        );
    }

    #[test]
    fn an_amended_artifact_replaces_its_row_rather_than_doubling_it() {
        let state = fresh_state();
        state
            .items
            .set(vec![decoded("ship-the-proxy", "task", "open", 2)]);

        let amended = decoded("ship-the-proxy", "task", "done", 3);
        assert!(state.publish_write(WriteOutcome::Wrote(Box::new(amended)), true));

        let items = state.items.get_untracked();
        assert_eq!(items.len(), 1, "one artifact, moved forward, not two");
        assert_eq!(items[0].version, 3);
        assert_eq!(items[0].state, "done");
        // And the editor is re-pointed at the version this write produced, so
        // the next amend presents v3 rather than the v2 it opened against.
        assert_eq!(state.base_version.get_untracked(), 3);
    }

    #[test]
    fn a_conflict_is_held_apart_from_an_error_because_it_has_a_recovery() {
        let state = fresh_state();
        assert!(
            !state.publish_write(
                WriteOutcome::Conflict {
                    expected: 3,
                    actual: 5,
                },
                true,
            ),
            "a refused write must not re-list: nothing moved"
        );
        assert_eq!(state.conflict.get_untracked(), Some((3, 5)));
        assert!(
            state.error.get_untracked().is_none(),
            "a conflict is not an error: it is the one refusal with a control attached"
        );
        assert!(!state.saving.get_untracked());
    }

    #[test]
    fn a_new_write_retires_the_conflict_banner_it_is_answering() {
        // A stale-version banner left standing over a fresh submit tells its
        // author the write they are watching already failed.
        let state = fresh_state();
        state.conflict.set(Some((3, 5)));
        state.error.set(Some("something older".to_string()));
        state.begin_write();
        assert!(state.conflict.get_untracked().is_none());
        assert!(state.error.get_untracked().is_none());
        assert!(state.saving.get_untracked());
    }

    #[test]
    fn a_room_change_retires_the_open_editor_and_its_in_flight_write() {
        // An editor left open across a room change would submit the old room's
        // artifact id against the new room's key, and `saving` carried across
        // would leave the new room's save control disabled over a write it never
        // issued.
        let state = fresh_state();
        let opened = decoded("ship-the-proxy", "task", "open", 2);
        state.items.set(vec![opened.clone()]);
        state.load_editor(&opened);
        state.saving.set(true);
        state.conflict.set(Some((2, 3)));
        state.loaded.set(true);

        state.reset();
        assert!(state.panel.get_untracked().is_none());
        assert!(!state.saving.get_untracked());
        assert!(state.conflict.get_untracked().is_none());
        assert!(!state.loaded.get_untracked());
        assert_eq!(state.base_version.get_untracked(), 0);
        assert!(state.draft_title.get_untracked().is_empty());
    }

    #[test]
    fn the_minted_id_is_a_single_safe_path_segment() {
        // The id addresses two routes and the daemon refuses an untrimmed one,
        // so what a human typed must not be able to produce a bad segment.
        assert_eq!(slug_id("Ship the proxy"), "ship-the-proxy");
        assert_eq!(slug_id("  Ship / the  proxy!! "), "ship-the-proxy");
        assert_eq!(slug_id("Ship\u{2014}v2"), "ship-v2");
        let long = slug_id(&"word ".repeat(30));
        assert!(long.len() <= 48, "{long}");
        assert!(!long.ends_with('-'), "{long}");
        // A title with nothing to slug yields an empty id, which `create_refusal`
        // — not this function — is what turns into a sentence.
        assert_eq!(slug_id("!!! ???"), "");
    }

    #[test]
    fn a_create_the_daemon_would_reject_is_refused_here_with_a_reason() {
        // The handler's pre-store checks answer a bare `invalid_request` with no
        // `code`, so a malformed create that reaches the wire comes back as
        // prose this module would have to guess at.
        assert!(create_refusal("ship-it", "Ship it").is_none());
        assert_eq!(
            create_refusal("", "   ").as_deref(),
            Some("An artifact needs a title.")
        );
        // A title that slugs to nothing: the title is real, the id is not.
        let refusal = create_refusal(&slug_id("!!!"), "!!!").expect("a refusal");
        assert!(refusal.contains("no letters or numbers"), "{refusal}");
        // The daemon refuses an id it would have to trim, so this side must
        // never mint one.
        assert!(create_refusal(" leading", "Leading").is_some());
    }

    #[test]
    fn the_lifecycle_mark_speaks_only_when_it_is_not_the_ordinary_case() {
        assert_eq!(lifecycle_mark("open"), None);
        assert_eq!(lifecycle_mark("done"), Some("done"));
        assert_eq!(lifecycle_mark("dropped"), Some("dropped"));
        // A state the daemon adds later must not become a mark this side
        // invented a meaning for.
        assert_eq!(lifecycle_mark("archived"), None);
    }

    #[test]
    fn the_meta_line_carries_the_version_an_amend_has_to_present() {
        let artifact = decoded("use-sqlite", "decision", "done", 4);
        let meta = artifact_meta(&artifact);
        assert!(meta.contains("decision"), "{meta}");
        assert!(meta.contains("v4"), "{meta}");
        assert!(meta.contains("smaths"), "{meta}");
        assert!(meta.contains("done"), "{meta}");
    }

    #[test]
    fn a_row_is_never_nameless() {
        // A blank title is not a shape the daemon creates — it refuses one — but
        // a row a reader cannot name is a row they cannot open, and the id is
        // always there.
        let mut artifact = decoded("use-sqlite", "decision", "open", 1);
        assert_eq!(artifact_label(&artifact), "Ship the proxy");
        artifact.title = "   ".to_string();
        assert_eq!(artifact_label(&artifact), "use-sqlite");
    }

    /// Source assertion against the real stylesheet, in the style of
    /// `slash_menu.rs`'s.
    ///
    /// The right rail is `flex: 0 0 220px`, and a PROPORTIONAL section there
    /// is not a feature, it is another thing nobody can read — so every
    /// section takes a fixed sliver and does its reading in a panel on the
    /// overlay tier, leaving the roster's `flex: 1` the rest of the column.
    /// That is a layout decision an innocent-looking edit can undo, in a rail
    /// four features share.
    #[test]
    fn the_rail_section_takes_a_fixed_sliver_and_never_a_share_of_the_column() {
        let css_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../styles/rooms-workspace.css"
        );
        let css =
            std::fs::read_to_string(css_path).unwrap_or_else(|e| panic!("read {css_path}: {e}"));

        let section = rule_body(&css, ".rooms-workspace__artifacts {");
        assert!(
            !section.contains('%'),
            "the artifacts rail section must not claim a percentage of the right \
             rail's height; it competes with the summary and the files: {section}"
        );
        assert!(
            section.contains("flex: 0 0 auto"),
            "the section must not grow into the roster's space: {section}"
        );

        let list = rule_body(&css, ".rooms-workspace__artifacts-list {");
        assert!(
            list.contains("max-height") && list.contains("px") && !list.contains('%'),
            "the rail list is capped in PIXELS so it takes a fixed sliver: {list}"
        );
        assert!(
            list.contains("overflow-y: auto"),
            "a capped list that cannot scroll hides rows: {list}"
        );

        // The neighbours hold the same line. The summary and the files once
        // claimed 38% and 40% of the column between them — the roster kept
        // about a fifth of its own rail — and a percentage reappearing on any
        // section is that regression starting over.
        for selector in [
            ".rooms-workspace__summary {",
            ".rooms-workspace__files {",
            ".rooms-workspace__repo {",
        ] {
            let body = rule_body(&css, selector);
            assert!(
                !body.contains('%'),
                "`{selector}` must not claim a share of the right rail: {body}"
            );
            assert!(
                body.contains("flex: 0 0 auto"),
                "`{selector}` must not grow into the roster's space: {body}"
            );
        }
    }

    /// The declarations of one rule, found by its exact opening line.
    fn rule_body<'a>(css: &'a str, selector: &str) -> &'a str {
        let at = css
            .find(selector)
            .unwrap_or_else(|| panic!("rooms-workspace.css must define `{selector}`"));
        let open = at + selector.len();
        let close = css[open..]
            .find('}')
            .unwrap_or_else(|| panic!("`{selector}` is unterminated"));
        &css[open..open + close]
    }
}
