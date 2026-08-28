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
//!    identity. The control is therefore gated exactly as the composer is: on
//!    the access projection, with identity refused at the action.
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
/// `Ok(None)` is the case that matters: the daemon's 404 means this room has
/// never been summarized, which is an ANSWER and not a failure — it is the only
/// thing that earns the empty state the right to speak. A `Result` rather than
/// a named three-variant enum because every arm but one would be a `String`
/// next to a 248-byte artifact.
type SummaryRead = Result<Option<RoomArtifact>, String>;

fn classify_read(status: u16, body: ArtifactBody) -> SummaryRead {
    if body.ok {
        return Ok(body.artifact);
    }
    if status == 404 {
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
            ticket: RwSignal::new(0),
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
    /// permanently if that request never resolves.
    fn reset(&self) {
        self.ticket
            .update(|ticket| *ticket = ticket.wrapping_add(1));
        self.artifact.set(None);
        self.loaded.set(false);
        self.loading.set(false);
        self.error.set(None);
        self.note.set(None);
        self.summarizing.set(false);
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
            // Latest-wins: an older read publishing over a newer one is what put
            // a premature empty state on screen in three previous features.
            if ticket != me.ticket.get_untracked() {
                return;
            }
            me.loading.set(false);
            match read {
                // Only an ANSWER may declare the summary known — including the
                // 404 that answers "never summarized". A failed read that
                // flipped this would replace an honest error with the false
                // claim that the room has no summary.
                Ok(artifact) => {
                    me.artifact.set(artifact);
                    me.loaded.set(true);
                }
                Err(error) => me.error.set(Some(error)),
            }
        });
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
        self.summarizing.set(true);
        self.error.set(None);
        self.note.set(None);
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
            // Every write below belongs to the room this run started in. If that
            // room is gone, `reset` has already cleared this state for whoever is
            // on screen now, and the artifact still landed in the room it was
            // meant for — reopening it reads the summary back.
            if !rooms.room_is_current(generation, &key) {
                return;
            }
            me.summarizing.set(false);
            match outcome {
                SummarizeOutcome::Wrote(artifact) => {
                    me.artifact.set(Some(artifact));
                    me.loaded.set(true);
                }
                SummarizeOutcome::Unchanged(artifact) => {
                    me.note.set(Some(summary_note(Some("unchanged"))));
                    me.artifact.set(Some(artifact));
                    me.loaded.set(true);
                }
                // A note never clears the artifact: `no_messages` is about the
                // transcript, not about the summary that already stands.
                SummarizeOutcome::Note(note) => me.note.set(Some(note)),
                SummarizeOutcome::Failure(error) => me.error.set(Some(error)),
            }
        });
    }
}

// ---- Component --------------------------------------------------------------

/// The open room's summary: what it says now, and one control to re-run it.
///
/// `writes_allowed` is supplied by the workspace rather than recomputed here so
/// this control and the composer can never disagree about the same room's
/// access projection. `members` is the same roster memo the transcript renders
/// against, so an `@id` means the same thing in a summary as in a message.
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
    // re-enable when bootstrap answers; the composer gates on access here and
    // refuses on identity at the action, and this control does the same.
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
                                "Still signing in \u{2014} try again in a moment.".to_string(),
                            ));
                            return;
                        }
                        state.summarize(rooms, key, rooms.identity_id.get_untracked());
                    }
                >
                    {move || {
                        if state.summarizing.get() { "summarizing\u{2026}" } else { "summarize" }
                    }}
                </button>
            </div>

            {move || {
                state.error.get().map(|error| view! {
                    <div class="rooms-workspace__summary-error" role="alert">{error}</div>
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
                        <div class="rooms-workspace__summary-note">"Loading summary\u{2026}"</div>
                    }.into_any();
                }
                // The standing summary stays readable through a re-run. Blanking
                // it for the duration would remove the only thing on screen worth
                // reading, and a run that comes back `unchanged` would have taken
                // away nothing but the reader's place in the text.
                if let Some(artifact) = state.artifact.get() {
                    let dropped = artifact.state == "dropped";
                    let detail = format!(
                        "{} \u{2014} updated {} by {}",
                        artifact.id, artifact.updated_at, artifact.updated_by,
                    );
                    return view! {
                        <div class="rooms-workspace__summary-scroll">
                            <div
                                class="rooms-workspace__summary-body"
                                class:rooms-workspace__summary-body--dropped=dropped
                            >
                                // Structural rendering only: `body_view` emits
                                // Leptos text nodes inside a fixed element set
                                // with no innerHTML path, so model-written prose
                                // cannot become markup on this origin.
                                {crate::room_markdown::body_view(artifact.body.clone(), members)}
                            </div>
                            <div class="rooms-workspace__summary-meta" title=detail>
                                {summary_meta(&artifact)}
                            </div>
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
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rooms::room_request_is_current;

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
        // And the ticket still moves, so a read in flight cannot land either.
        assert_eq!(state.ticket.get_untracked(), 5);
    }

    /// The read ticket cannot cover a summarize run: a room change bumps it for
    /// the NEW room's read, which would then admit the OLD room's answer. So the
    /// completion re-validates the `(generation, key)` pair it started with
    /// through `Rooms::room_is_current`, which delegates to
    /// `crate::rooms::room_request_is_current` — exercised directly here, with
    /// no live `Rooms` or browser runtime needed.
    #[test]
    fn a_run_landing_after_a_room_change_cannot_publish() {
        let started_generation = 3;
        let started_key = "room-a";

        // It lands while A is still open: publish, so the operator reads the
        // summary they just paid a model turn for.
        assert!(room_request_is_current(
            started_generation,
            started_generation,
            started_key,
            Some(started_key),
        ));

        // The operator switched to room B first. Publishing here would put A's
        // conversation on screen as B's summary — prose about the wrong room,
        // which is the worst thing this control could render.
        assert!(!room_request_is_current(
            started_generation,
            started_generation + 1,
            started_key,
            Some("room-b"),
        ));

        // Closed to no room at all: nothing to publish into.
        assert!(!room_request_is_current(
            started_generation,
            started_generation + 1,
            started_key,
            None,
        ));

        // A close/reopen of the SAME key is a different admission — `reset` has
        // already cleared this state, so the in-flight run must not write into
        // it even though the key still matches.
        assert!(!room_request_is_current(
            started_generation,
            started_generation + 1,
            started_key,
            Some(started_key),
        ));
    }
}
