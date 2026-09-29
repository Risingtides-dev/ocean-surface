//! The room's summary — the browser half of the daemon's summarize route.
//!
//! A long room is unreadable, and the daemon's answer is not another wall of
//! chat: it is one durable thing the room OWNS. `room_summary.rs` there reads a
//! bounded tail of the transcript, runs a single model turn, and folds the
//! result into the well-known `room-summary` artifact. That has been true since
//! the summarize route landed; until now no browser could run it or read what
//! it wrote.
//!
//!   POST /v1/rooms/persistent/{key}/summarize                 → run one
//!   GET  /v1/rooms/persistent/{key}/artifacts/room-summary    → read the one
//!
//! Four properties of that wire contract shape everything below:
//!
//! 1. **The artifact is a SINGLETON with a fixed id.** Repeat runs amend it in
//!    place under the store's compare-and-swap; they never stack. So this side
//!    mints no ids and posts no artifacts — it addresses one well-known id and
//!    watches `version` move.
//! 2. **`summarized: false` is not a failure.** `unchanged` means the model
//!    looked at the same conversation and said the same thing, and the reply
//!    still carries the artifact that stands. Rendering that as an error would
//!    tell an operator their room broke when nothing did. `no_messages` and
//!    `empty_summary` are equally truthful 200s. All three are notes; only a
//!    non-`ok` body is a failure.
//! 3. **The request body is `deny_unknown_fields`** with `requested_by`
//!    required. One extra field is a 400, so exactly one field goes out.
//! 4. **`requested_by` is caller-asserted and gated.** An id resolving to an
//!    Agent or System participant comes back 403 `forged_artifact_author` — an
//!    agent's artifact is written by the daemon, never by a client claiming its
//!    identity. The control is therefore gated on the access projection, with
//!    identity refused at the action — the composer's SHAPE, but not the
//!    composer's gate: a run lands in this daemon's store, so it stays offered
//!    while the link is coming back (see `local_store_write_gate`).
//!
//! Everything that turns a reply into what the operator sees is a free function
//! below, unit-testable natively without a browser or a daemon.

use std::collections::HashSet;

use gloo_net::http::Request;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen_futures::spawn_local;

use crate::rooms::{encode, Rooms};

/// The one artifact id every summarize call writes to, mirroring the daemon's
/// `ROOM_SUMMARY_ARTIFACT_ID`. A constant rather than anything this side
/// chooses: "the room's summary" is singular, and a client that minted its own
/// id would be asking the daemon to accumulate the near-duplicates it has
/// deliberately refused to accumulate.
pub const ROOM_SUMMARY_ARTIFACT_ID: &str = "room-summary";

// ---- Wire types -------------------------------------------------------------

/// One room artifact. Mirrors `ocean_core::RoomArtifact`.
///
/// `kind` and `state` arrive as the daemon's snake_case strings rather than
/// mirrored enums: this struct is the wire shape, not a view model, and a
/// client that refuses to decode a variant the server later adds is a client
/// that breaks on a server it was supposed to tolerate.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RoomArtifact {
    pub id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub created_by: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_by: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub on_behalf_of: Option<String>,
    #[serde(default)]
    pub version: u64,
}

/// The whole request. One field, because the daemon's `SummarizeRequest` is
/// `deny_unknown_fields` — `limit` and `after_seq` exist there, but pinning a
/// window is an operator decision this control does not have to make, and
/// sending a field this side does not mean is how a 400 gets invented.
#[derive(Debug, Serialize)]
struct SummarizeRequest<'a> {
    requested_by: &'a str,
}

#[derive(Debug, Deserialize)]
struct SummarizeBody {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    summarized: bool,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    artifact: Option<RoomArtifact>,
    #[serde(default)]
    error: Option<String>,
}

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
}

// ---- Pure helpers -----------------------------------------------------------

fn summarize_url(base: &str, key: &str) -> String {
    format!("{base}/v1/rooms/persistent/{}/summarize", encode(key))
}

fn artifact_url(base: &str, key: &str) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/artifacts/{}",
        encode(key),
        encode(ROOM_SUMMARY_ARTIFACT_ID),
    )
}

/// What a summarize reply means.
#[derive(Debug, PartialEq, Eq)]
enum SummarizeOutcome {
    /// A summary was written or amended. Render it.
    Wrote(RoomArtifact),
    /// The store refused a no-op amend, which is correct. Nothing moved, and
    /// the artifact that already stands came back with the refusal.
    Unchanged(RoomArtifact),
    /// A truthful 200 with nothing to show.
    Note(String),
    /// A refusal, in words an operator can act on.
    Failure(String),
}

/// Map a summarize reply onto what the room should show.
///
/// Presence of an artifact, not the `code` string, decides whether there is
/// something to render: a body carrying an artifact always carries prose the
/// room owns, whatever the daemon called the situation.
fn classify_summarize(status: u16, body: SummarizeBody) -> SummarizeOutcome {
    if !body.ok {
        return SummarizeOutcome::Failure(summary_failure_message(
            status,
            body.code.as_deref(),
            body.error.as_deref(),
        ));
    }
    match (body.summarized, body.artifact) {
        (true, Some(artifact)) => SummarizeOutcome::Wrote(artifact),
        (false, Some(artifact)) => SummarizeOutcome::Unchanged(artifact),
        (false, None) => SummarizeOutcome::Note(summary_note(body.code.as_deref())),
        // A shape the daemon does not emit. Refusing it is the only honest
        // answer: an "ok" with nothing in it must never be allowed to blank the
        // summary that already stands.
        (true, None) => {
            SummarizeOutcome::Failure("The room summarized but sent nothing back.".to_string())
        }
    }
}

/// What reading the artifact back told us.
///
/// `Ok(None)` is the case that matters: the daemon's `unknown_artifact` 404
/// means this room has never been summarized, which is an ANSWER and not a
/// failure — it is the only thing that earns the empty state the right to
/// speak. A `Result` rather than a named three-variant enum because every arm
/// but one would be a `String` next to a 248-byte artifact.
type SummaryRead = Result<Option<RoomArtifact>, String>;

fn classify_read(status: u16, body: ArtifactBody) -> SummaryRead {
    if body.ok {
        return Ok(body.artifact);
    }
    // The code, not the status. `unknown_artifact` is the daemon's ONLY coded
    // 404 on this route; its other one is the unknown room, which
    // `room_store_error_response` sends with no `code` at all. Reading that as
    // an empty summary would tell an operator a room that is GONE merely has
    // nothing to say yet.
    if status == 404 && body.code.as_deref() == Some("unknown_artifact") {
        return Ok(None);
    }
    Err(summary_failure_message(
        status,
        body.code.as_deref(),
        body.error.as_deref(),
    ))
}

/// The truthful 200s. None of these is a fault, and none of them may be dressed
/// up as one — an operator who reads "failed" goes looking for a broken room.
fn summary_note(code: Option<&str>) -> String {
    match code {
        Some("unchanged") => "Unchanged \u{2014} nothing new since the last summary.".to_string(),
        Some("no_messages") => "Nothing to summarize yet.".to_string(),
        Some("empty_summary") => "The summary came back empty. Try again.".to_string(),
        _ => "Nothing to summarize.".to_string(),
    }
}

/// Turn a refusal into something an operator can act on.
///
/// The daemon's typed `code` is the input, not its prose, which is written for
/// a log reader. Two of these have to say what actually happened or they read
/// as bugs: `at_capacity` is a busy daemon and is worth retrying, while
/// `forged_artifact_author` fires on a live control held by a permitted
/// identity — the id it carried simply belongs to an agent.
fn summary_failure_message(status: u16, code: Option<&str>, error: Option<&str>) -> String {
    match code {
        Some("at_capacity") => {
            "The daemon is at its concurrent-turn limit. Try again shortly.".to_string()
        }
        Some("forged_artifact_author") => {
            "A summary is attributed to a person on the roster \u{2014} an agent's is written by \
             the daemon."
                .to_string()
        }
        Some("summary_provider_error") => "The summary model call failed.".to_string(),
        Some("summary_timeout") => {
            "The summary model took too long. Try again \u{2014} a shorter room answers faster."
                .to_string()
        }
        Some("unknown_room") => "That room is no longer open.".to_string(),
        Some("invalid_request") => "The room refused that summarize request.".to_string(),
        _ => match error {
            Some(text) if !text.is_empty() => format!("Summarize failed: {text}"),
            _ => format!("Summarize failed ({status})."),
        },
    }
}

/// A lifecycle mark worth showing. The daemon's summarize path leaves `state`
/// alone on every amend precisely so a room that marked its summary done or
/// dropped keeps that; `open` is the ordinary case and says nothing.
fn lifecycle_label(state: &str) -> Option<&'static str> {
    match state {
        "done" => Some("done"),
        "dropped" => Some("dropped"),
        _ => None,
    }
}

/// The provenance line under the summary: which artifact this is, how far it
/// has moved, and who last moved it.
///
/// `version` is the load-bearing half. It is the visible proof that a repeat
/// run amended one artifact rather than stacking a second one — the id never
/// changes, the number does.
fn summary_meta(artifact: &RoomArtifact) -> String {
    let name = if artifact.title.trim().is_empty() {
        artifact.id.as_str()
    } else {
        artifact.title.trim()
    };
    let mut meta = format!("{name} \u{b7} v{}", artifact.version);
    if !artifact.updated_by.is_empty() {
        meta.push_str(" \u{b7} ");
        meta.push_str(&artifact.updated_by);
    }
    if let Some(mark) = lifecycle_label(&artifact.state) {
        meta.push_str(" \u{b7} ");
        meta.push_str(mark);
    }
    meta
}

/// Latest-wins admission for an overlapping read. The same predicate
/// `attachments.rs` extracts for its list ticket, and here for the same reason:
/// an older completion publishing over a newer one is what put a premature
/// empty state on screen in three previous features (TASK-104/106/107).
fn summary_read_is_current(ticket: u64, current: u64) -> bool {
    ticket == current
}

/// Escape owned by the summary panel. Same contract as
/// `artifacts_escape_closes`: the panel is a fixed modal on the rooms
/// surface's overlay tier, so it consumes the key before the drawers under
/// it. A predicate for the same reason that one is — a ladder rung no test
/// can reach is a rung the next edit deletes in silence.
pub fn summary_escape_closes(panel_open: bool, default_prevented: bool) -> bool {
    panel_open && !default_prevented
}

// ---- State ------------------------------------------------------------------

/// Reactive handle for one room's summary.
///
/// Constructed at `RoomsWorkspace` component scope, never inside a rail
/// closure: those closures re-run on every `rooms.access` SSE update, and an
/// in-flight flag rebuilt mid-request would re-enable the control during its
/// own summarize — a second provider turn for a room already spending one of
/// the daemon's turn permits.
#[derive(Clone, Copy)]
pub struct RoomSummaryState {
    /// Daemon base URL, shared with `Daemon::url` through `Rooms::url` — read
    /// live at request time because bootstrap resolves the origin
    /// asynchronously (a phone via the tunnel resolves it late).
    pub url: RwSignal<String>,
    /// The open room's summary artifact, if it has one.
    pub artifact: RwSignal<Option<RoomArtifact>>,
    /// Whether a read has ANSWERED for the room now open — including the 404
    /// that means "never summarized". Starts false and returns to false on
    /// every room change, so the empty state can never claim a room has no
    /// summary before that room has said anything.
    pub loaded: RwSignal<bool>,
    /// A read is in flight.
    pub loading: RwSignal<bool>,
    /// The most recent failure, read or run.
    pub error: RwSignal<Option<String>>,
    /// The most recent truthful non-result. Deliberately not `error`: an
    /// unchanged summary and a room with nothing to say are both correct
    /// outcomes, and colouring them like faults is a lie about the room.
    pub note: RwSignal<Option<String>>,
    /// A summarize run is in flight — blocks re-submit and drives the label.
    pub summarizing: RwSignal<bool>,
    /// Whether the reading-measure panel is open.
    panel: RwSignal<bool>,
    /// The rail control that opens the panel, so closing hands focus back.
    open_ref: NodeRef<leptos::html::Button>,
    /// Monotonic ticket; only the latest overlapping read may publish.
    ticket: RwSignal<u64>,
}

impl RoomSummaryState {
    pub fn new(rooms: &Rooms) -> Self {
        Self {
            url: rooms.url,
            artifact: RwSignal::new(None),
            loaded: RwSignal::new(false),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            note: RwSignal::new(None),
            summarizing: RwSignal::new(false),
            panel: RwSignal::new(false),
            open_ref: NodeRef::new(),
            ticket: RwSignal::new(0),
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

    /// Retire whatever is on screen and whatever is in flight.
    ///
    /// The ticket bump is the load-bearing half: without it the previous room's
    /// summary could still land and be read under this room's name — and a
    /// summary is prose ABOUT a conversation, so showing the wrong room's is
    /// worse than showing none. `summarizing` is retired here as well: a run
    /// belongs to the room that started it, so a flag carried across a room
    /// change disables the new room's control over work it never asked for,
    /// permanently if that request never resolves. The panel goes with them:
    /// one left open across a room change would present the next room's
    /// summary inside a dialog the operator opened for this one.
    fn reset(&self) {
        self.ticket
            .update(|ticket| *ticket = ticket.wrapping_add(1));
        self.artifact.set(None);
        self.loaded.set(false);
        self.loading.set(false);
        self.error.set(None);
        self.note.set(None);
        self.summarizing.set(false);
        self.panel.set(false);
    }

    /// Read back the room's standing summary.
    ///
    /// This is the half that makes the feature exist for someone who did not
    /// press the button: opening a room summarized last week shows what it says,
    /// without spending a model turn.
    pub fn fetch(&self, key: String) {
        let base = self.base();
        let me = *self;
        let ticket = self.ticket.get_untracked().wrapping_add(1);
        self.ticket.set(ticket);
        self.loading.set(true);
        self.error.set(None);
        spawn_local(async move {
            let url = artifact_url(&base, &key);
            let read = match Request::get(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    match resp.json::<ArtifactBody>().await {
                        Ok(body) => classify_read(status, body),
                        // A 404 body the daemon shapes itself always decodes;
                        // anything that does not is a real transport fault.
                        Err(err) => Err(format!("Summary decode error: {err}")),
                    }
                }
                Err(err) => Err(format!("Summary request failed: {err}")),
            };
            let current = summary_read_is_current(ticket, me.ticket.get_untracked());
            me.publish_read(read, current);
        });
    }

    /// Publish a completed read — but only the latest one.
    ///
    /// `is_current` is the caller's ticket check, taken as an argument for the
    /// same reason `publish_run` takes its verdict: a guard no test can reach is
    /// a guard the next edit deletes in silence.
    fn publish_read(&self, read: SummaryRead, is_current: bool) {
        if !is_current {
            return;
        }
        self.loading.set(false);
        match read {
            // Only an ANSWER may declare the summary known — including the 404
            // that answers "never summarized". A failed read that flipped this
            // would replace an honest error with the false claim that the room
            // has no summary.
            Ok(artifact) => {
                self.artifact.set(artifact);
                self.loaded.set(true);
            }
            Err(error) => self.error.set(Some(error)),
        }
    }

    /// Run one summarize turn and publish what came back.
    ///
    /// The reply carries the artifact, so there is no follow-up read: the POST
    /// is the whole round trip. `rooms` is taken so the completion can
    /// re-validate the `(generation, key)` pair it started with — the read
    /// ticket cannot stand in for it, because a room change bumps that ticket
    /// for the NEW room's read, which would then admit this room's answer.
    fn summarize(&self, rooms: Rooms, key: String, requested_by: String) {
        let base = self.base();
        let me = *self;
        let generation = rooms.generation_snapshot();
        self.begin_run();
        spawn_local(async move {
            let url = summarize_url(&base, &key);
            let payload = SummarizeRequest {
                requested_by: &requested_by,
            };
            let outcome = match Request::post(&url)
                .header("content-type", "application/json")
                .json(&payload)
            {
                Ok(request) => match request.send().await {
                    Ok(resp) => {
                        let status = resp.status();
                        match resp.json::<SummarizeBody>().await {
                            Ok(body) => classify_summarize(status, body),
                            Err(err) => {
                                SummarizeOutcome::Failure(format!("Summarize decode error: {err}"))
                            }
                        }
                    }
                    Err(err) => {
                        SummarizeOutcome::Failure(format!("Summarize request failed: {err}"))
                    }
                },
                Err(err) => SummarizeOutcome::Failure(format!("Summarize encode error: {err}")),
            };
            me.publish_run(outcome, rooms.room_is_current(generation, &key));
        });
    }

    /// Take the state into a run.
    ///
    /// Retiring the read is the half that is easy to miss. `can_summarize`
    /// deliberately does not wait on `loading`, so the control is live from
    /// first paint while the room-open GET is still out — and that GET is
    /// holding the artifact as it stood BEFORE this run. Landing last it would
    /// publish the pre-run prose and its older `v{n}` over the summary the
    /// operator just paid a model turn for, which is the one number this module
    /// asks the reader to trust. `loading` goes with it: a read that can no
    /// longer publish must not keep a spinner over the summary that can. The
    /// cost is that a run which then FAILS leaves the panel with no standing
    /// summary until the next run or reopen — the run is the authority for the
    /// room it started in, and a read carrying pre-run state is not a fallback
    /// worth reinstating.
    fn begin_run(&self) {
        self.ticket
            .update(|ticket| *ticket = ticket.wrapping_add(1));
        self.loading.set(false);
        self.summarizing.set(true);
        self.error.set(None);
        self.note.set(None);
    }

    /// Publish a completed run — but only into the room that started it.
    ///
    /// `room_is_current` is the caller's `(generation, key)` re-validation,
    /// taken as an argument rather than recomputed here so the refusal itself is
    /// reachable from a test with no live `Rooms` and no browser runtime. When
    /// it is false there is nothing to write: `reset` has already cleared this
    /// state for whoever is on screen now, and the artifact still landed in the
    /// room it was meant for — reopening that room reads the summary back.
    fn publish_run(&self, outcome: SummarizeOutcome, room_is_current: bool) {
        if !room_is_current {
            return;
        }
        self.summarizing.set(false);
        match outcome {
            SummarizeOutcome::Wrote(artifact) => {
                self.artifact.set(Some(artifact));
                self.loaded.set(true);
            }
            SummarizeOutcome::Unchanged(artifact) => {
                self.note.set(Some(summary_note(Some("unchanged"))));
                self.artifact.set(Some(artifact));
                self.loaded.set(true);
            }
            // A note never clears the artifact: `no_messages` is about the
            // transcript, not about the summary that already stands.
            SummarizeOutcome::Note(note) => self.note.set(Some(note)),
            SummarizeOutcome::Failure(error) => self.error.set(Some(error)),
        }
    }
}

// ---- Component --------------------------------------------------------------

/// The open room's summary: a compact rail row, and a panel where the prose is
/// actually read and the run control lives.
///
/// The rail deliberately holds one line — whether a summary exists and how
/// current it is. The right rail is 220px wide and prose is unreadable there;
/// everything at a reading measure happens in the panel, exactly as
/// `room_artifacts` and `room_repo` do it.
///
/// `writes_allowed` is supplied by the workspace rather than recomputed here so
/// one place holds the ruling for every rail. It is deliberately NOT the
/// composer's gate: a run summarizes this room's own transcript into this
/// daemon's store and is never enqueued for a peer, so a link that is down or
/// coming back does not hold it (see `local_store_write_gate`). `members` is
/// the same roster memo the transcript renders against, so an `@id` means the
/// same thing in a summary as in a message.
#[component]
pub fn RoomSummary(
    rooms: Rooms,
    state: RoomSummaryState,
    writes_allowed: Signal<bool>,
    members: Memo<HashSet<String>>,
) -> impl IntoView {
    // Follow the open room. Clearing FIRST is what stops the previous room's
    // summary from being read, however briefly, under this room's name.
    Effect::new(move |_| match rooms.open_key.get() {
        Some(key) if !key.is_empty() => {
            state.reset();
            state.fetch(key);
        }
        _ => state.reset(),
    });

    // Identity is deliberately NOT part of this predicate. `identity_resolved`
    // reads its signals untracked, so a control disabled on it would never
    // re-enable when bootstrap answers; the composer gates on access at the
    // control and refuses on identity at the action, and this control does the
    // same — with its own access gate, not the composer's.
    let can_summarize = move || {
        writes_allowed.get()
            && !state.summarizing.get()
            && rooms.open_key.get().is_some_and(|key| !key.is_empty())
    };

    view! {
        <div class="rooms-workspace__summary">
            <div class="rooms-workspace__summary-head">
                <span class="rooms-workspace__summary-title">"Summary"</span>
                <button
                    class="rooms-workspace__summary-open"
                    type="button"
                    node_ref=state.open_ref
                    title="Open this room's summary"
                    disabled=move || {
                        rooms.open_key.get().is_none_or(|key| key.is_empty())
                    }
                    on:click=move |_| {
                        state.error.set(None);
                        state.panel.set(true);
                    }
                >
                    "open"
                </button>
            </div>

            // Rendered in the rail AND the panel, like the artifacts error: a
            // failure while the panel is closed must not read as a room with
            // nothing to say.
            {move || {
                state.error.get().map(|error| view! {
                    <div class="rooms-workspace__summary-error" role="alert">{error}</div>
                })
            }}

            // The collapsed row's one line: whether a summary exists and how
            // current it is. The prose itself lives in the panel.
            {move || {
                if state.loading.get() {
                    return view! {
                        <div class="rooms-workspace__summary-note">"Loading summary\u{2026}"</div>
                    }.into_any();
                }
                if let Some(artifact) = state.artifact.get() {
                    let detail = format!(
                        "{} \u{2014} updated {} by {}",
                        artifact.id, artifact.updated_at, artifact.updated_by,
                    );
                    return view! {
                        <div class="rooms-workspace__summary-line" title=detail>
                            {summary_meta(&artifact)}
                        </div>
                    }.into_any();
                }
                if state.summarizing.get() {
                    return view! {
                        <div class="rooms-workspace__summary-note">
                            "Reading the transcript\u{2026}"
                        </div>
                    }.into_any();
                }
                // Never answered: say nothing rather than assert an absence.
                if !state.loaded.get() {
                    return ().into_any();
                }
                view! {
                    <div class="rooms-workspace__summary-note">"No summary yet."</div>
                }.into_any()
            }}

            {move || {
                if !state.panel.get() {
                    return ().into_any();
                }
                view! {
                    <div
                        class="rooms-workspace__summary-scrim"
                        on:click=move |_| state.close_panel()
                    ></div>
                    // `aria-modal` because the scrim is only paint: without it
                    // a screen reader still walks the rail and the transcript
                    // behind a dialog a sighted reader cannot reach.
                    <div
                        class="rooms-workspace__summary-panel"
                        role="dialog"
                        aria-modal="true"
                        aria-label="Room summary"
                    >
                        <div class="rooms-workspace__summary-panel-head">
                            <span class="rooms-workspace__summary-panel-title">"Summary"</span>
                            <button
                                class="rooms-workspace__summary-run"
                                type="button"
                                title="Summarize this room's transcript into the room's summary"
                                disabled=move || !can_summarize()
                                on:click=move |_| {
                                    let Some(key) = rooms
                                        .open_key
                                        .get_untracked()
                                        .filter(|key| !key.is_empty())
                                    else {
                                        return;
                                    };
                                    if !rooms.identity_resolved() {
                                        state.error.set(Some(
                                            "Still signing in \u{2014} try again in a moment."
                                                .to_string(),
                                        ));
                                        return;
                                    }
                                    state.summarize(
                                        rooms,
                                        key,
                                        rooms.identity_id.get_untracked(),
                                    );
                                }
                            >
                                {move || {
                                    if state.summarizing.get() {
                                        "summarizing\u{2026}"
                                    } else {
                                        "summarize"
                                    }
                                }}
                            </button>
                            <button
                                class="rooms-workspace__summary-close"
                                type="button"
                                aria-label="Close summary"
                                on:click=move |_| state.close_panel()
                            >
                                "\u{d7}"
                            </button>
                        </div>
                        <div class="rooms-workspace__summary-panel-body">
                            {move || {
                                state.error.get().map(|error| view! {
                                    <div class="rooms-workspace__summary-error" role="alert">
                                        {error}
                                    </div>
                                })
                            }}
                            {move || {
                                state.note.get().map(|note| view! {
                                    <div class="rooms-workspace__summary-note">{note}</div>
                                })
                            }}
                            {move || {
                                if state.loading.get() {
                                    return view! {
                                        <div class="rooms-workspace__summary-note">
                                            "Loading summary\u{2026}"
                                        </div>
                                    }.into_any();
                                }
                                // The standing summary stays readable through a
                                // re-run. Blanking it for the duration would
                                // remove the only thing on screen worth reading,
                                // and a run that comes back `unchanged` would
                                // have taken away nothing but the reader's place
                                // in the text.
                                if let Some(artifact) = state.artifact.get() {
                                    let dropped = artifact.state == "dropped";
                                    let detail = format!(
                                        "{} \u{2014} updated {} by {}",
                                        artifact.id, artifact.updated_at, artifact.updated_by,
                                    );
                                    return view! {
                                        <div
                                            class="rooms-workspace__summary-body"
                                            class:rooms-workspace__summary-body--dropped=dropped
                                        >
                                            // Structural rendering only:
                                            // `body_view` emits Leptos text nodes
                                            // inside a fixed element set with no
                                            // innerHTML path, so model-written
                                            // prose cannot become markup on this
                                            // origin.
                                            {crate::room_markdown::body_view(
                                                artifact.body.clone(),
                                                members,
                                            )}
                                        </div>
                                        <div class="rooms-workspace__summary-meta" title=detail>
                                            {summary_meta(&artifact)}
                                        </div>
                                    }.into_any();
                                }
                                if state.summarizing.get() {
                                    return view! {
                                        <div class="rooms-workspace__summary-note">
                                            "Reading the transcript\u{2026}"
                                        </div>
                                    }.into_any();
                                }
                                if !state.loaded.get() {
                                    return ().into_any();
                                }
                                view! {
                                    <div class="rooms-workspace__summary-note">
                                        "No summary yet."
                                    </div>
                                }.into_any()
                            }}
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state as `new` leaves it, for the tests that drive one directly.
    fn fresh_state() -> RoomSummaryState {
        RoomSummaryState {
            url: RwSignal::new("http://d".to_string()),
            artifact: RwSignal::new(None),
            loaded: RwSignal::new(false),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            note: RwSignal::new(None),
            summarizing: RwSignal::new(false),
            panel: RwSignal::new(false),
            open_ref: NodeRef::new(),
            ticket: RwSignal::new(0),
        }
    }

    /// The room's summary at a given version — the only field these tests read
    /// back, because it is what tells a repeat run's amend from a stale read.
    fn artifact_at(version: u64) -> RoomArtifact {
        RoomArtifact {
            id: ROOM_SUMMARY_ARTIFACT_ID.to_string(),
            kind: "note".to_string(),
            title: "Room summary".to_string(),
            body: "They agreed to ship on Friday.".to_string(),
            state: "open".to_string(),
            created_by: "smaths".to_string(),
            created_at: String::new(),
            updated_by: "smaths".to_string(),
            updated_at: String::new(),
            on_behalf_of: None,
            version,
        }
    }

    fn summarize_body(json: &str) -> SummarizeBody {
        serde_json::from_str(json).expect("decode")
    }

    fn artifact_body(json: &str) -> ArtifactBody {
        serde_json::from_str(json).expect("decode")
    }

    /// The daemon's `Wrote` shape, with `version` as the caller supplies it.
    fn wrote_json(version: u64) -> String {
        format!(
            r#"{{"ok":true,"summarized":true,"created":false,
                "artifact":{{
                    "id":"room-summary","kind":"note","title":"Room summary",
                    "body":"They agreed to ship on Friday.","state":"open",
                    "created_by":"smaths","created_at":"2026-08-01T09:00:00Z",
                    "updated_by":"smaths","updated_at":"2026-08-25T10:00:00Z",
                    "version":{version}
                }},
                "model":"haiku","messages_summarized":40,
                "from_seq":0,"to_seq":39,"has_more":false}}"#
        )
    }

    #[test]
    fn the_room_key_is_one_encoded_path_segment_on_both_routes() {
        // A key with a slash must never become two segments and reach a
        // different daemon route.
        assert_eq!(
            summarize_url("http://d", "a/b"),
            "http://d/v1/rooms/persistent/a%2Fb/summarize"
        );
        assert_eq!(
            artifact_url("http://d", "call:xyz"),
            "http://d/v1/rooms/persistent/call%3Axyz/artifacts/room-summary"
        );
    }

    #[test]
    fn the_request_carries_requested_by_and_nothing_else() {
        // `SummarizeRequest` is `deny_unknown_fields` on the daemon, so one
        // extra key here is a 400 the operator could do nothing about.
        let json = serde_json::to_string(&SummarizeRequest {
            requested_by: "smaths",
        })
        .expect("encode");
        assert_eq!(json, r#"{"requested_by":"smaths"}"#);
    }

    #[test]
    fn a_repeat_run_moves_one_artifact_forward_instead_of_stacking() {
        // The whole point of the singleton id: two runs, two replies, one
        // artifact. If this side ever started addressing a minted id, or the
        // daemon started answering with `summary-2`, the ids below would
        // diverge — and that is exactly the moment a second summary would
        // appear in the room.
        let first = match classify_summarize(200, summarize_body(&wrote_json(1))) {
            SummarizeOutcome::Wrote(artifact) => artifact,
            other => panic!("expected a write, got {other:?}"),
        };
        let second = match classify_summarize(200, summarize_body(&wrote_json(2))) {
            SummarizeOutcome::Wrote(artifact) => artifact,
            other => panic!("expected a write, got {other:?}"),
        };
        assert_eq!(first.id, second.id);
        assert!(
            second.version > first.version,
            "an amend advances the version: {} then {}",
            first.version,
            second.version
        );
        // And the version the operator reads is the one that moved.
        assert!(summary_meta(&second).contains("v2"));
        // The id the reader is shown is the id this side asked for, so the
        // route and the rendered artifact cannot drift apart.
        assert!(artifact_url("http://d", "r").ends_with(&second.id));
    }

    #[test]
    fn unchanged_renders_the_artifact_it_carries_rather_than_an_error() {
        // The store refused a no-op amend, which is correct. Treating this as a
        // failure would tell an operator their room broke when nothing did.
        let outcome = classify_summarize(
            200,
            summarize_body(
                r#"{"ok":true,"summarized":false,"code":"unchanged",
                    "artifact":{"id":"room-summary","kind":"note","title":"Room summary",
                    "body":"Still the same call.","state":"open","created_by":"smaths",
                    "created_at":"2026-08-01T09:00:00Z","updated_by":"smaths",
                    "updated_at":"2026-08-25T10:00:00Z","version":7}}"#,
            ),
        );
        match outcome {
            SummarizeOutcome::Unchanged(artifact) => {
                assert_eq!(artifact.body, "Still the same call.");
                assert_eq!(artifact.version, 7);
            }
            other => panic!("unchanged must render the artifact, got {other:?}"),
        }
    }

    #[test]
    fn a_room_with_nothing_to_say_is_a_note_not_a_failure() {
        assert_eq!(
            classify_summarize(
                200,
                summarize_body(r#"{"ok":true,"summarized":false,"code":"no_messages"}"#)
            ),
            SummarizeOutcome::Note("Nothing to summarize yet.".to_string()),
        );
        let empty = classify_summarize(
            200,
            summarize_body(r#"{"ok":true,"summarized":false,"code":"empty_summary"}"#),
        );
        assert!(matches!(empty, SummarizeOutcome::Note(_)));
    }

    #[test]
    fn an_ok_reply_with_no_artifact_to_show_never_blanks_the_standing_summary() {
        // Not a shape the daemon emits. Admitting it would set `artifact` to a
        // summary with an empty body, erasing the one the room actually has.
        let outcome = classify_summarize(200, summarize_body(r#"{"ok":true,"summarized":true}"#));
        assert!(matches!(outcome, SummarizeOutcome::Failure(_)));
    }

    #[test]
    fn every_refusal_says_something_an_operator_can_act_on() {
        let busy = summary_failure_message(429, Some("at_capacity"), None);
        assert!(busy.contains("Try again"), "retryable: {busy}");
        let forged = summary_failure_message(403, Some("forged_artifact_author"), None);
        assert!(forged.contains("daemon"), "{forged}");
        assert!(forged.contains("roster"), "{forged}");
        let provider = summary_failure_message(502, Some("summary_provider_error"), None);
        assert!(provider.contains("model"), "{provider}");
        // 504, not a 502 variant: the daemon's timeout arm is its own status.
        let timeout = summary_failure_message(504, Some("summary_timeout"), None);
        assert!(timeout.contains("Try again"), "{timeout}");
        // An untyped failure still says something, and never swallows the
        // server's own words when it has them.
        assert_eq!(
            summary_failure_message(500, None, Some("disk on fire")),
            "Summarize failed: disk on fire"
        );
        assert_eq!(
            summary_failure_message(500, None, None),
            "Summarize failed (500)."
        );
        assert_eq!(
            summary_failure_message(500, None, Some("")),
            "Summarize failed (500)."
        );
    }

    #[test]
    fn a_never_summarized_room_reads_as_an_answer_not_a_fault() {
        // The 404 is what earns the empty state the right to say "no summary
        // yet". Rendering it as an error would hide a working feature behind a
        // red line on every room nobody has summarized.
        assert_eq!(
            classify_read(
                404,
                artifact_body(
                    r#"{"ok":false,"code":"unknown_artifact",
                        "error":"room 'r' has no artifact 'room-summary'"}"#
                )
            ),
            Ok(None),
        );
        // A real fault still is one.
        assert!(
            classify_read(500, artifact_body(r#"{"ok":false,"error":"store fault"}"#)).is_err()
        );
        // A 404 the daemon did NOT code is its unknown-room refusal —
        // `room_store_error_response` sends that one with no `code` at all.
        // Answering it with the empty state would tell an operator that a room
        // which is GONE merely has nothing to say yet.
        assert!(classify_read(
            404,
            artifact_body(r#"{"ok":false,"error":"no room with key 'r'"}"#)
        )
        .is_err());
    }

    #[test]
    fn a_standing_summary_decodes_from_the_daemons_own_artifact_shape() {
        let read = classify_read(
            200,
            artifact_body(
                r#"{"ok":true,"artifact":{
                    "id":"room-summary","kind":"note","title":"Room summary",
                    "body":"They agreed to ship on Friday.","state":"open",
                    "created_by":"smaths","created_at":"2026-08-01T09:00:00Z",
                    "updated_by":"ari","updated_at":"2026-08-25T10:00:00Z","version":3}}"#,
            ),
        );
        let Ok(Some(artifact)) = read else {
            panic!("expected the artifact, got {read:?}");
        };
        assert_eq!(artifact.kind, "note");
        assert_eq!(artifact.created_by, "smaths");
        assert_eq!(artifact.created_at, "2026-08-01T09:00:00Z");
        // `on_behalf_of` is skipped when a human authored, and must not be
        // required to decode.
        assert_eq!(artifact.on_behalf_of, None);
        assert_eq!(summary_meta(&artifact), "Room summary \u{b7} v3 \u{b7} ari");
    }

    #[test]
    fn the_meta_line_carries_a_rename_and_a_lifecycle_mark() {
        // The daemon leaves title and state alone on every amend precisely so a
        // room that renamed or retired its summary keeps that. Dropping either
        // here would present a tombstone as the room's current word.
        let mut artifact = RoomArtifact {
            id: ROOM_SUMMARY_ARTIFACT_ID.to_string(),
            kind: "note".to_string(),
            title: "Where we landed".to_string(),
            body: "b".to_string(),
            state: "dropped".to_string(),
            created_by: "smaths".to_string(),
            created_at: String::new(),
            updated_by: "smaths".to_string(),
            updated_at: String::new(),
            on_behalf_of: None,
            version: 9,
        };
        assert_eq!(
            summary_meta(&artifact),
            "Where we landed \u{b7} v9 \u{b7} smaths \u{b7} dropped"
        );
        artifact.state = "open".to_string();
        assert_eq!(
            summary_meta(&artifact),
            "Where we landed \u{b7} v9 \u{b7} smaths"
        );
        // An artifact with no title falls back to the id rather than to a blank
        // separator that reads as a rendering bug.
        artifact.title = "   ".to_string();
        assert!(summary_meta(&artifact).starts_with(ROOM_SUMMARY_ARTIFACT_ID));
    }

    #[test]
    fn reset_retires_the_in_flight_run_not_just_the_read() {
        let state = RoomSummaryState {
            url: RwSignal::new("http://d".to_string()),
            artifact: RwSignal::new(Some(RoomArtifact {
                id: ROOM_SUMMARY_ARTIFACT_ID.to_string(),
                kind: "note".to_string(),
                title: "Room summary".to_string(),
                body: "b".to_string(),
                state: "open".to_string(),
                created_by: "smaths".to_string(),
                created_at: String::new(),
                updated_by: "smaths".to_string(),
                updated_at: String::new(),
                on_behalf_of: None,
                version: 2,
            })),
            loaded: RwSignal::new(true),
            loading: RwSignal::new(true),
            error: RwSignal::new(Some("boom".to_string())),
            note: RwSignal::new(Some("unchanged".to_string())),
            summarizing: RwSignal::new(true),
            panel: RwSignal::new(true),
            open_ref: NodeRef::new(),
            ticket: RwSignal::new(4),
        };

        state.reset();

        assert!(state.artifact.get_untracked().is_none());
        assert!(!state.loaded.get_untracked());
        assert!(!state.loading.get_untracked());
        assert!(state.error.get_untracked().is_none());
        assert!(state.note.get_untracked().is_none());
        // The room the run was for is gone; leaving this true renders the NEXT
        // room's control disabled and reading "summarizing…" for a turn it never
        // asked for, forever if that request never resolves.
        assert!(!state.summarizing.get_untracked());
        // The panel closes with the room it was opened for: left standing it
        // would present the next room's summary inside this room's dialog.
        assert!(!state.panel_is_open());
        // And the ticket still moves, so a read in flight cannot land either.
        assert_eq!(state.ticket.get_untracked(), 5);
    }

    #[test]
    fn escape_closes_only_an_open_unclaimed_panel() {
        assert!(summary_escape_closes(true, false));
        assert!(!summary_escape_closes(false, false));
        // A key someone under us already consumed is not ours to act on.
        assert!(!summary_escape_closes(true, true));
    }

    #[test]
    fn stale_reads_cannot_publish() {
        assert!(summary_read_is_current(7, 7));
        assert!(!summary_read_is_current(6, 7));
    }

    #[test]
    fn a_read_landing_after_a_newer_answer_writes_nothing() {
        // The state a run has just published into, with the room-open GET it
        // raced still outstanding and holding the PRE-run artifact.
        let state = fresh_state();
        state.artifact.set(Some(artifact_at(4)));
        state.loaded.set(true);

        state.publish_read(Ok(Some(artifact_at(3))), false);

        // v3 on screen would be the visible half of the lie: the operator paid
        // for v4 and the meta line would swear the room is still at v3.
        assert_eq!(
            state
                .artifact
                .get_untracked()
                .map(|artifact| artifact.version),
            Some(4)
        );
        // A stale failure is no more admissible than stale prose.
        state.publish_read(Err("Summarize failed (500).".to_string()), false);
        assert!(state.error.get_untracked().is_none());
    }

    #[test]
    fn only_an_answer_declares_the_summary_known() {
        // The 404 that means "never summarized" IS an answer, and it is the
        // only thing that earns the empty state the right to speak.
        let answered = fresh_state();
        answered.publish_read(Ok(None), true);
        assert!(answered.loaded.get_untracked());
        assert!(!answered.loading.get_untracked());

        // A failed read must not: flipping `loaded` here would replace an
        // honest error with the false claim that the room has no summary.
        let failed = fresh_state();
        failed.publish_read(Err("Summarize failed (500).".to_string()), true);
        assert!(!failed.loaded.get_untracked());
        assert_eq!(
            failed.error.get_untracked().as_deref(),
            Some("Summarize failed (500)."),
        );
    }

    #[test]
    fn starting_a_run_retires_the_room_open_read_it_races() {
        // The ordinary shape on a slow origin, because the control is live from
        // first paint: the room-open GET is still out — holding the artifact as
        // it stood BEFORE this run — when the operator presses summarize.
        let state = fresh_state();
        state.loading.set(true);
        state.error.set(Some("the last room's failure".to_string()));
        state.note.set(Some("the last room's note".to_string()));
        let read_in_flight = state.ticket.get_untracked();

        state.begin_run();

        // Landing last, that read would put the pre-run prose and its older
        // `v{n}` back over the summary this run is about to write.
        assert!(!summary_read_is_current(
            read_in_flight,
            state.ticket.get_untracked()
        ));
        // And it takes its spinner with it: a read that can no longer publish
        // must not keep "Loading summary…" over the summary that can.
        assert!(!state.loading.get_untracked());
        assert!(state.summarizing.get_untracked());
        assert!(state.error.get_untracked().is_none());
        assert!(state.note.get_untracked().is_none());
    }

    /// The read ticket cannot cover a summarize run: a room change bumps it for
    /// the NEW room's read, which would then admit the OLD room's answer. So the
    /// completion re-validates the `(generation, key)` pair it started with and
    /// hands the verdict to `publish_run`, which is where the refusal lives and
    /// where a test can reach it without a live `Rooms` or a browser runtime.
    #[test]
    fn a_run_landing_after_a_room_change_writes_nothing() {
        let state = fresh_state();
        state.summarizing.set(true);

        state.publish_run(SummarizeOutcome::Wrote(artifact_at(4)), false);

        // Publishing here would put the old room's conversation on screen as
        // this room's summary — prose about the wrong room, which is the worst
        // thing this control could render.
        assert!(state.artifact.get_untracked().is_none());
        assert!(!state.loaded.get_untracked());
        // Not even the flag: `reset` retired this run when the room changed,
        // and a late write to `summarizing` would disable the control that is
        // on screen now over a turn it never asked for.
        assert!(state.summarizing.get_untracked());
    }

    #[test]
    fn a_run_landing_in_its_own_room_publishes_and_no_later_reply_blanks_it() {
        let state = fresh_state();
        state.summarizing.set(true);

        state.publish_run(SummarizeOutcome::Wrote(artifact_at(4)), true);
        assert_eq!(
            state
                .artifact
                .get_untracked()
                .map(|artifact| artifact.version),
            Some(4)
        );
        assert!(state.loaded.get_untracked());
        assert!(!state.summarizing.get_untracked());

        // A note is about the transcript, not about the summary that stands.
        state.publish_run(
            SummarizeOutcome::Note(summary_note(Some("no_messages"))),
            true,
        );
        assert_eq!(
            state
                .artifact
                .get_untracked()
                .map(|artifact| artifact.version),
            Some(4)
        );
        assert_eq!(
            state.note.get_untracked().as_deref(),
            Some("Nothing to summarize yet."),
        );

        // And neither does a refusal: the room's last word stays readable while
        // the operator reads why the re-run did not land.
        state.publish_run(SummarizeOutcome::Failure("boom".to_string()), true);
        assert_eq!(
            state
                .artifact
                .get_untracked()
                .map(|artifact| artifact.version),
            Some(4)
        );
        assert_eq!(state.error.get_untracked().as_deref(), Some("boom"));
    }
}
