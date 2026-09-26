//! Closing a room — the member-facing half of the daemon's close route
//! (Rooms DoD 4.3).
//!
//!   POST /v1/rooms/persistent/{key}/close?actor_id=<member>
//!     → 200 `{ok:true, room, closed:true, marker_seq}`
//!
//! Five facts about that route shape everything below.
//!
//! 1. **Two authorities, and this surface uses exactly one.** The daemon takes
//!    either `?actor_id=` naming a roster member (the MEMBER lane) or an
//!    `X-Ocean-Operator` credential (the OPERATOR lane, deliberately not
//!    roster-checked), and the header's PRESENCE picks the lane. The operator
//!    key never enters a WASM bundle and the proxy injects it only on the six
//!    room-agent authority routes, so every host — browser PWA, extension,
//!    Tauri — closes through the member lane with no operator header. Wiring
//!    the operator lane here would let anyone signed in to the proxy close a
//!    room they are not in, which is precisely what the daemon's member lane
//!    refuses.
//! 2. **Who the daemon lets close.** In the member lane: the room must be OPEN
//!    (a closed or absent room is the flat `404 room_not_open`), `actor_id`
//!    must be non-empty after trimming (`400 invalid_request`), the id must
//!    NOT name an `Agent` or `System` participant (`403 forged_closer`), and
//!    the id must be ON the room's local participant roster, checked inside the
//!    closing transaction (`403`, `RoomCloserNotInRoster`). `Human`, `Bot` and
//!    `Tool` rows all pass. [`close_offered`] is that rule, over
//!    `Room.participants` — which the daemon serves from the same participants
//!    table its transaction reads — and the control does not render for anyone
//!    it answers `false` for. The daemon stays the authority; this copy only
//!    stops the surface offering a button the daemon would refuse.
//! 3. **A second close is a 404, not a silent success.** `close_with_marker`
//!    refuses a room that is not open with the same `room_not_open` answer
//!    every other write to a closed room gets. So a close that raced another
//!    member's, or a retry after a cut response whose close DID land, reads as
//!    [`CloseOutcome::AlreadyClosed`] and lands in the same audit view a clean
//!    close does — the room is closed either way, and saying "failed" would be
//!    a lie.
//! 4. **Afterwards the room is frozen, not gone.** `/snapshot` answers
//!    `closed: true` with the transcript, roster and agent ownership intact,
//!    and the close marker names who closed it. The surface already renders
//!    that as the audit view (DoD 1.10: no composer writes, no tail, no
//!    minting), so a successful close re-hydrates through
//!    [`Rooms::open_room`] — the one path that reads `closed` and starts no
//!    tail on it — instead of flipping signals by hand beside it.
//! 5. **Closing is local.** Nothing is sent to Bedrock: a federated room's
//!    credential, outbox and access projection are untouched. Members on other
//!    nodes keep their copy, so the confirmation says so for any room whose
//!    access state is not `Local`.
//!
//! Everything that decides what the operator sees is a free function below,
//! unit-tested natively.

use gloo_net::http::Request;
use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen_futures::spawn_local;

use crate::rooms::{
    encode, Room, RoomAccessProjection, RoomAccessState, RoomParticipantKind, Rooms,
};

// ---- Eligibility ------------------------------------------------------------

/// Should the close control render for this viewer in this room?
///
/// The daemon's member-lane rule, point for point (module doc, fact 2):
///
/// - the room is open (`closed` false) and loaded;
/// - the viewer's identity is authoritative and non-empty after trimming —
///   the same bar join and post clear, because acting under an unresolved
///   identity is how ghost members were made;
/// - that trimmed id is on the room's participant roster;
/// - and its row is not an `Agent` or `System` participant.
pub(crate) fn close_offered(
    closed: bool,
    identity_authoritative: bool,
    identity_id: &str,
    room: Option<&Room>,
) -> bool {
    if closed || !identity_authoritative {
        return false;
    }
    let actor = identity_id.trim();
    if actor.is_empty() {
        return false;
    }
    let Some(room) = room else {
        return false;
    };
    room.participants
        .iter()
        .find(|participant| participant.id == actor)
        .is_some_and(|participant| {
            !matches!(
                participant.kind,
                RoomParticipantKind::Agent | RoomParticipantKind::System
            )
        })
}

/// Whether closing leaves copies of this room elsewhere. Any access state but
/// `Local` means the room is federated, and the daemon's close is a LOCAL
/// statement that tells Bedrock nothing. `None` (no projection yet) is not
/// assumed federated: the note is a claim, and a claim needs an answer behind
/// it.
pub(crate) fn close_is_local_only(access: Option<&RoomAccessProjection>) -> bool {
    access.is_some_and(|access| access.state != RoomAccessState::Local)
}

/// The member-lane close URL. Both the key and the actor are encoded as one
/// path segment / one query value each: an id carrying `&` or `#` would
/// otherwise split the query and close as somebody else, or as nobody.
pub(crate) fn room_close_url(base: &str, key: &str, actor_id: &str) -> String {
    format!(
        "{}/v1/rooms/persistent/{}/close?actor_id={}",
        base.trim_end_matches('/'),
        encode(key),
        encode(actor_id.trim()),
    )
}

// ---- Reply ------------------------------------------------------------------

/// Every field the close route or its refusals can carry. All optional: a
/// refusal body has no `closed`, a success has no `code`, and an older
/// daemon's bare 404 has no body at all (`None` at the call site).
#[derive(Debug, Default, Deserialize)]
pub(crate) struct CloseReply {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    closed: bool,
    #[serde(default)]
    room_not_open: bool,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// What a close request came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CloseOutcome {
    /// The daemon closed the room and said so.
    Closed,
    /// The daemon says the room is not open — somebody closed it first, or a
    /// cut response hid that this screen's own close landed. Either way the
    /// room is closed and the audit view is the honest destination.
    AlreadyClosed,
    /// Refused or failed; the sentence is what the operator reads.
    Refused(String),
}

/// Map a close reply onto what the dialog shows. Success is settled first, on
/// the status AND the body's own `closed: true`, so a 200 from something that
/// is not the close route cannot read as a closed room.
pub(crate) fn classify_close(status: u16, body: Option<CloseReply>) -> CloseOutcome {
    let Some(body) = body else {
        // No JSON at all. A bare 404/405 is a daemon that predates the route
        // (or a proxy that does not carry it); anything else is unreadable.
        return match status {
            404 | 405 => CloseOutcome::Refused(route_absent_sentence()),
            _ => CloseOutcome::Refused(format!(
                "The close reply could not be read (HTTP {status}). The room may or may not \
                 have closed \u{2014} try again: a room that already closed says so."
            )),
        };
    };
    if (200..300).contains(&status) && body.ok && body.closed {
        return CloseOutcome::Closed;
    }
    if status == 404 && body.room_not_open {
        return CloseOutcome::AlreadyClosed;
    }
    let code = body
        .code
        .as_deref()
        .map(str::trim)
        .filter(|code| !code.is_empty());
    let detail = body
        .error
        .as_deref()
        .map(str::trim)
        .filter(|error| !error.is_empty());
    match (status, code) {
        (403, Some("forged_closer")) => CloseOutcome::Refused(
            "The daemon refused: this identity belongs to an agent or the system in this \
             room, and only a person on the roster can close it."
                .to_string(),
        ),
        // The store's roster proof inside the closing transaction. The control
        // only renders for a roster member, so reaching this means the roster
        // moved under the open dialog.
        (403, _) => CloseOutcome::Refused(with_detail(
            "The daemon refused: you are not on this room's roster, so you cannot close it.",
            detail,
        )),
        (400, _) => CloseOutcome::Refused(with_detail(
            "The daemon refused the request: it did not name a roster member.",
            detail,
        )),
        (404 | 405, _) => CloseOutcome::Refused(route_absent_sentence()),
        (200..=299, _) => CloseOutcome::Refused(
            "The daemon answered without confirming the room closed. Reopen the room to see \
             its current state."
                .to_string(),
        ),
        _ => CloseOutcome::Refused(with_detail(
            &format!("Closing the room failed (HTTP {status})."),
            detail,
        )),
    }
}

fn with_detail(sentence: &str, detail: Option<&str>) -> String {
    match detail {
        Some(detail) => format!("{sentence} ({detail})"),
        None => sentence.to_string(),
    }
}

fn route_absent_sentence() -> String {
    "This daemon cannot close rooms yet \u{2014} it predates the close route. Update ocean-os \
     on the machine running it."
        .to_string()
}

/// The transport failed before any reply. The daemon commits the close before
/// it answers, so a cut response can hide a close that happened.
fn cut_sentence(err: &str) -> String {
    format!(
        "The request was cut ({err}). The room may have closed without this screen hearing \
         back \u{2014} try again: a room that already closed says so."
    )
}

// ---- State ------------------------------------------------------------------

/// Reactive handle for the open room's close control.
///
/// Constructed at `RoomsWorkspace` scope, never inside the header closure:
/// that closure re-runs on every `open_room` write, and an in-flight flag
/// rebuilt by one would re-enable the button during its own close.
#[derive(Clone, Copy)]
pub struct RoomCloseState {
    /// The header overflow menu is open.
    menu: RwSignal<bool>,
    /// The confirmation dialog is open.
    confirm: RwSignal<bool>,
    /// A close is in flight — blocks a second POST and drives the label.
    in_flight: RwSignal<bool>,
    /// The last refusal, shown inside the dialog.
    error: RwSignal<Option<String>>,
    /// The overflow trigger, so dismissing hands focus back.
    trigger_ref: NodeRef<leptos::html::Button>,
    /// The dialog's safe choice, focused when it opens.
    keep_ref: NodeRef<leptos::html::Button>,
}

impl RoomCloseState {
    pub fn new() -> Self {
        Self {
            menu: RwSignal::new(false),
            confirm: RwSignal::new(false),
            in_flight: RwSignal::new(false),
            error: RwSignal::new(None),
            trigger_ref: NodeRef::new(),
            keep_ref: NodeRef::new(),
        }
    }

    /// Retire everything: a confirmation belongs to the room it was opened
    /// over. A reply still in flight is dropped by its own generation check.
    fn reset(&self) {
        self.menu.set(false);
        self.confirm.set(false);
        self.in_flight.set(false);
        self.error.set(None);
    }

    /// Dismiss the dialog without closing anything. Refused while a close is
    /// in flight: the request is already out, and pretending otherwise would
    /// hide its answer.
    fn dismiss(&self) {
        if self.in_flight.get_untracked() {
            return;
        }
        self.confirm.set(false);
        self.error.set(None);
        if let Some(trigger) = self.trigger_ref.get_untracked() {
            let _ = trigger.focus();
        }
    }

    /// Fire the close for the open room.
    fn fire(&self, rooms: Rooms) {
        if self.in_flight.get_untracked() {
            return;
        }
        let Some(key) = rooms.open_key.get_untracked().filter(|key| !key.is_empty()) else {
            return;
        };
        let actor = rooms.identity_id.get_untracked();
        // Re-checked at dispatch, not only at render: identity or roster can
        // move between opening the dialog and pressing it.
        if !close_offered(
            rooms.closed.get_untracked(),
            rooms.identity_authoritative.get_untracked(),
            &actor,
            rooms.open_room.get_untracked().as_ref(),
        ) {
            self.error.set(Some(
                "You can no longer close this room from here \u{2014} you are not on its roster \
                 as a person, or it has already closed."
                    .to_string(),
            ));
            return;
        }
        let url = room_close_url(&rooms.url.get_untracked(), &key, &actor);
        let generation = rooms.generation_snapshot();
        let me = *self;
        self.in_flight.set(true);
        self.error.set(None);
        spawn_local(async move {
            let outcome = post_close(&url).await;
            if !rooms.room_is_current(generation, &key) {
                return;
            }
            me.in_flight.set(false);
            match outcome {
                CloseOutcome::Closed | CloseOutcome::AlreadyClosed => {
                    me.confirm.set(false);
                    // The rail lists open rooms; drop the row without
                    // disturbing a paged rail.
                    rooms.fetch_rooms_silent();
                    // Re-hydrate into the audit view. `open_room` bumps the
                    // generation (retiring the live tail), reads `/snapshot`'s
                    // `closed: true`, and starts no tail on it.
                    rooms.open_room(key);
                }
                CloseOutcome::Refused(sentence) => me.error.set(Some(sentence)),
            }
        });
    }
}

impl Default for RoomCloseState {
    fn default() -> Self {
        Self::new()
    }
}

/// One close POST: transport, decode, classify. No body — the route reads
/// only its path and query.
async fn post_close(url: &str) -> CloseOutcome {
    match Request::post(url).send().await {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.json::<CloseReply>().await.ok();
            classify_close(status, body)
        }
        Err(err) => CloseOutcome::Refused(cut_sentence(&err.to_string())),
    }
}

// ---- Component --------------------------------------------------------------

/// The open room's header overflow (`⋯`) and the close confirmation it leads
/// to. Renders nothing at all for a viewer [`close_offered`] refuses —
/// absence, not a disabled button — and nothing for a closed room.
#[component]
pub fn RoomCloseControl(rooms: Rooms, state: RoomCloseState) -> impl IntoView {
    // One room admission, one confirmation. Keyed on the generation rather
    // than the key: a same-key reopen (which is exactly what a successful
    // close does) is a new admission too.
    Effect::new(move |_| {
        let _ = rooms.generation_snapshot_reactive();
        state.reset();
    });

    // A Memo so roster SSE traffic that leaves the answer unchanged does not
    // rebuild an open dialog mid-close.
    let offered = Memo::new(move |_| {
        close_offered(
            rooms.closed.get(),
            rooms.identity_authoritative.get(),
            &rooms.identity_id.get(),
            rooms.open_room.get().as_ref(),
        )
    });
    let local_only = Memo::new(move |_| close_is_local_only(rooms.access.get().as_ref()));

    // Focus the safe choice when the dialog mounts.
    Effect::new(move |_| {
        if let Some(keep) = state.keep_ref.get() {
            let _ = keep.focus();
        }
    });

    let room_name = move || {
        rooms
            .open_room
            .get()
            .map(|room| room.name)
            .unwrap_or_default()
    };

    view! {
        {move || {
            if !offered.get() {
                return ().into_any();
            }
            view! {
                <div class="rooms-workspace__room-menu">
                    <button
                        class="rooms-workspace__room-menu-trigger"
                        type="button"
                        node_ref=state.trigger_ref
                        aria-label="More room actions"
                        aria-haspopup="menu"
                        aria-expanded=move || state.menu.get().to_string()
                        on:click=move |_| state.menu.update(|open| *open = !*open)
                    >
                        "\u{22ef}"
                    </button>
                    {move || {
                        state.menu.get().then(|| view! {
                            <div
                                class="rooms-workspace__room-menu-scrim"
                                on:click=move |_| state.menu.set(false)
                            ></div>
                            <div
                                class="rooms-workspace__room-menu-list"
                                role="menu"
                                on:keydown=move |ev| {
                                    if ev.key() == "Escape" {
                                        ev.prevent_default();
                                        state.menu.set(false);
                                        if let Some(trigger) = state.trigger_ref.get_untracked() {
                                            let _ = trigger.focus();
                                        }
                                    }
                                }
                            >
                                <button
                                    class="rooms-workspace__room-menu-item rooms-workspace__room-menu-item--danger"
                                    type="button"
                                    role="menuitem"
                                    on:click=move |_| {
                                        state.menu.set(false);
                                        state.error.set(None);
                                        state.confirm.set(true);
                                    }
                                >
                                    "Close room\u{2026}"
                                </button>
                            </div>
                        })
                    }}
                    {move || {
                        if !state.confirm.get() {
                            return ().into_any();
                        }
                        view! {
                            <div
                                class="rooms-workspace__close-scrim"
                                on:click=move |_| state.dismiss()
                            ></div>
                            <div
                                class="rooms-workspace__close-dialog"
                                role="alertdialog"
                                aria-modal="true"
                                aria-labelledby="rooms-close-title"
                                aria-describedby="rooms-close-desc"
                                on:keydown=move |ev| {
                                    if ev.key() == "Escape" {
                                        ev.prevent_default();
                                        state.dismiss();
                                    }
                                }
                            >
                                <h2 class="rooms-workspace__close-title" id="rooms-close-title">
                                    {move || format!("Close #{}?", room_name())}
                                </h2>
                                <p class="rooms-workspace__close-warn" id="rooms-close-desc">
                                    "Closing is permanent \u{2014} a closed room cannot be \
                                     reopened. It freezes for everyone here: nobody can post, \
                                     agent turns running in it are cancelled, and no new \
                                     messages arrive. The transcript, roster and files stay \
                                     readable as a closed record, and the transcript will say \
                                     you closed it."
                                </p>
                                {move || {
                                    local_only.get().then(|| view! {
                                        <p class="rooms-workspace__close-note">
                                            "This room is federated. Closing it here does not \
                                             close it on Bedrock \u{2014} members on other Ocean \
                                             nodes keep their copy."
                                        </p>
                                    })
                                }}
                                {move || {
                                    state.error.get().map(|error| view! {
                                        <div class="rooms-workspace__close-error" role="alert">
                                            {error}
                                        </div>
                                    })
                                }}
                                <div class="rooms-workspace__close-actions">
                                    <button
                                        class="rooms-workspace__close-keep"
                                        type="button"
                                        node_ref=state.keep_ref
                                        disabled=move || state.in_flight.get()
                                        on:click=move |_| state.dismiss()
                                    >
                                        "Keep room open"
                                    </button>
                                    <button
                                        class="rooms-workspace__close-fire"
                                        type="button"
                                        disabled=move || state.in_flight.get()
                                        on:click=move |_| state.fire(rooms)
                                    >
                                        {move || {
                                            if state.in_flight.get() {
                                                "Closing\u{2026}"
                                            } else {
                                                "Close room"
                                            }
                                        }}
                                    </button>
                                </div>
                            </div>
                        }
                        .into_any()
                    }}
                </div>
            }
            .into_any()
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rooms::RoomParticipant;

    fn participant(id: &str, kind: RoomParticipantKind) -> RoomParticipant {
        RoomParticipant {
            id: id.to_string(),
            kind,
            display_name: id.to_string(),
        }
    }

    fn room(participants: Vec<RoomParticipant>) -> Room {
        serde_json::from_value::<Room>(serde_json::json!({ "id": "team", "name": "Team" }))
            .map(|mut room| {
                room.participants = participants;
                room
            })
            .expect("minimal room decodes")
    }

    fn reply(value: serde_json::Value) -> Option<CloseReply> {
        Some(serde_json::from_value(value).expect("close reply decodes"))
    }

    // ── eligibility: the daemon's member lane, point for point ─────────────

    #[test]
    fn a_person_on_the_roster_of_an_open_room_is_offered_the_close() {
        let room = room(vec![participant("alice", RoomParticipantKind::Human)]);
        assert!(close_offered(false, true, "alice", Some(&room)));
    }

    #[test]
    fn bot_and_tool_rows_pass_the_daemon_forged_kind_gate_too() {
        // `room_close` refuses only Agent and System; the store proves roster
        // membership for everyone else.
        for kind in [RoomParticipantKind::Bot, RoomParticipantKind::Tool] {
            let room = room(vec![participant("me", kind)]);
            assert!(close_offered(false, true, "me", Some(&room)), "{kind:?}");
        }
    }

    #[test]
    fn agent_and_system_identities_are_never_offered_the_close() {
        // The daemon answers 403 forged_closer for both.
        for kind in [RoomParticipantKind::Agent, RoomParticipantKind::System] {
            let room = room(vec![participant("me", kind)]);
            assert!(!close_offered(false, true, "me", Some(&room)), "{kind:?}");
        }
    }

    #[test]
    fn a_viewer_off_the_roster_is_not_offered_the_close() {
        // The store's `RoomCloserNotInRoster` 403.
        let room = room(vec![participant("alice", RoomParticipantKind::Human)]);
        assert!(!close_offered(false, true, "bob", Some(&room)));
    }

    #[test]
    fn a_closed_room_offers_no_close() {
        // A second close is the daemon's 404 room_not_open.
        let room = room(vec![participant("alice", RoomParticipantKind::Human)]);
        assert!(!close_offered(true, true, "alice", Some(&room)));
    }

    #[test]
    fn an_unresolved_or_blank_identity_is_not_offered_the_close() {
        let room = room(vec![
            participant("alice", RoomParticipantKind::Human),
            participant("", RoomParticipantKind::Human),
        ]);
        assert!(
            !close_offered(false, false, "alice", Some(&room)),
            "an identity bootstrap has not confirmed must not act"
        );
        assert!(
            !close_offered(false, true, "   ", Some(&room)),
            "the daemon trims actor_id and answers 400 for a blank one"
        );
        assert!(!close_offered(false, true, "alice", None), "no room loaded");
    }

    #[test]
    fn the_identity_is_compared_trimmed_as_the_daemon_trims_it() {
        let room = room(vec![participant("alice", RoomParticipantKind::Human)]);
        assert!(close_offered(false, true, " alice ", Some(&room)));
    }

    // ── the URL ───────────────────────────────────────────────────────────

    #[test]
    fn the_close_url_is_the_member_lane_with_every_part_encoded() {
        assert_eq!(
            room_close_url("http://127.0.0.1:4780/", "team room", " a&b#c "),
            "http://127.0.0.1:4780/v1/rooms/persistent/team%20room/close?actor_id=a%26b%23c",
        );
        assert_eq!(
            room_close_url("", "team", "alice"),
            "/v1/rooms/persistent/team/close?actor_id=alice",
            "the browser PWA reaches the proxy on its own origin"
        );
    }

    // ── the reply ─────────────────────────────────────────────────────────

    #[test]
    fn a_confirmed_close_is_closed() {
        let body = reply(serde_json::json!({
            "ok": true, "closed": true, "marker_seq": 7, "room": { "id": "team", "name": "Team" }
        }));
        assert_eq!(classify_close(200, body), CloseOutcome::Closed);
    }

    #[test]
    fn a_2xx_without_the_closed_flag_is_not_read_as_closed() {
        assert!(matches!(
            classify_close(200, reply(serde_json::json!({ "ok": true }))),
            CloseOutcome::Refused(_)
        ));
        assert!(matches!(
            classify_close(
                200,
                reply(serde_json::json!({ "ok": false, "closed": true }))
            ),
            CloseOutcome::Refused(_)
        ));
    }

    #[test]
    fn the_room_not_open_404_is_already_closed_not_a_failure() {
        let body = reply(serde_json::json!({
            "ok": false,
            "error": "unknown room: team",
            "room_not_open": true,
            "code": "room_not_found",
        }));
        assert_eq!(classify_close(404, body), CloseOutcome::AlreadyClosed);
    }

    #[test]
    fn a_404_without_the_marker_is_a_daemon_without_the_route() {
        for body in [
            None,
            reply(serde_json::json!({ "ok": false, "error": "nope" })),
        ] {
            let CloseOutcome::Refused(sentence) = classify_close(404, body) else {
                panic!("a markerless 404 is not a closed room");
            };
            assert!(sentence.contains("predates the close route"), "{sentence}");
        }
        let CloseOutcome::Refused(sentence) = classify_close(405, None) else {
            panic!("405 is not a closed room");
        };
        assert!(sentence.contains("predates the close route"), "{sentence}");
    }

    #[test]
    fn forged_closer_says_why_in_words() {
        let body = reply(serde_json::json!({
            "ok": false,
            "code": "forged_closer",
            "error": "an agent does not close a room; a client may not close one while claiming its identity",
        }));
        let CloseOutcome::Refused(sentence) = classify_close(403, body) else {
            panic!("forged_closer is a refusal");
        };
        assert!(sentence.contains("agent or the system"), "{sentence}");
    }

    #[test]
    fn a_roster_refusal_says_so_and_carries_the_daemon_detail() {
        let body = reply(serde_json::json!({
            "ok": false,
            "error": "room closer bob is not in room team's roster",
        }));
        let CloseOutcome::Refused(sentence) = classify_close(403, body) else {
            panic!("a roster refusal is a refusal");
        };
        assert!(sentence.contains("not on this room's roster"), "{sentence}");
        assert!(sentence.contains("room closer bob"), "{sentence}");
    }

    #[test]
    fn a_bad_request_and_a_server_fault_are_refusals_with_their_status() {
        let CloseOutcome::Refused(sentence) = classify_close(
            400,
            reply(
                serde_json::json!({ "ok": false, "code": "invalid_request", "error": "closing a room needs ?actor_id=" }),
            ),
        ) else {
            panic!("400 is a refusal");
        };
        assert!(sentence.contains("closing a room needs"), "{sentence}");

        let CloseOutcome::Refused(sentence) = classify_close(
            500,
            reply(serde_json::json!({ "ok": false, "error": "disk I/O" })),
        ) else {
            panic!("500 is a refusal");
        };
        assert!(
            sentence.contains("HTTP 500") && sentence.contains("disk I/O"),
            "{sentence}"
        );

        let CloseOutcome::Refused(sentence) = classify_close(502, None) else {
            panic!("an unreadable 502 is a refusal");
        };
        assert!(sentence.contains("may or may not"), "{sentence}");
    }

    #[test]
    fn a_cut_response_does_not_claim_nothing_happened() {
        let sentence = cut_sentence("NetworkError");
        assert!(sentence.contains("may have closed"), "{sentence}");
    }

    // ── the federation note ───────────────────────────────────────────────

    #[test]
    fn only_a_room_known_to_be_federated_gets_the_local_only_note() {
        let projection = |state: &str| {
            serde_json::from_value::<RoomAccessProjection>(serde_json::json!({ "state": state }))
                .expect("projection decodes")
        };
        assert!(!close_is_local_only(Some(&projection("local"))));
        assert!(!close_is_local_only(None));
        for state in ["connecting", "live", "recovering", "revoked"] {
            assert!(close_is_local_only(Some(&projection(state))), "{state}");
        }
    }
}
