//! A closed room is an AUDIT VIEW, and the surface has to paint it as one.
//!
//! `GET /v1/rooms/persistent/{key}/snapshot` falls through to the daemon's
//! soft-closed audit view so a finished call stays replayable (OCEAN-170), and
//! since ocean-os#434 the body says which of the two arms answered: `closed`,
//! a plain bool, true exactly when the open-room read missed. It exists on
//! `/snapshot` alone — deliberately not on `Room`, which four other routes
//! serialize — and it is the ONLY discriminator on the wire. Closing a room
//! stamps `closed_at` and leaves its access row alone, so a closed room answers
//! 200, carries its transcript, and goes on projecting whatever access it had:
//! `Local` for a room that never had an access row — every purely local room,
//! and the daemon's own soft-closed fixture — and an unchanged `Live` or
//! `Revoked`, members and outbox included, for a federated one.
//!
//! Everything underneath that body disagrees: `GET .../events` 404s a closed
//! room and so does `POST .../messages`. So a surface that hydrates and asks no
//! further question paints a transcript above an `EventSource` reconnecting
//! forever and a composer whose every send is silently dropped — a room that
//! looks alive and is not. That is the state this file exists to keep out.
//!
//! One write in the pane is worse than looking alive: the outbox Retry button
//! is genuinely alive. `retry_failed_outbox` gates on the room EXISTING, not on
//! `closed_at`, so it is the one control here that a closed room answers 202
//! to — and a closed FEDERATED room carries the outbox that paints it, since
//! `access` survives the close untouched. It is gated on both axes for that
//! reason and is guarded here alongside the composer.
//!
//! ## Why a scanner
//!
//! `ocean-surface-ui` is a BINARY crate, so an integration test here cannot
//! import an item, mount a component, or press anything (`tests/common/mod.rs`
//! says so at length). Worse, the two things this slice turns on are the two
//! least testable things in the crate: `EventSource` and `spawn_local` are
//! browser-only, and `Rooms::new` needs a live `Daemon`. Nothing in this
//! workspace can open a room and observe that no socket was opened.
//!
//! Scanning the source is the lever that is left, under the two rules
//! `unheld_room_controls.rs` paid for and `room_hydration_resume.rs` restated:
//! name the CALL SITE, not the helper it calls, and scan
//! `common::view_source`, which truncates at `#[cfg(test)]` so a module's own
//! unit tests cannot satisfy a needle by quoting it.
//!
//! ## Measured, not assumed
//!
//! Ten mutations run for real against this tree, each applied ALONE with
//! `cargo test -p ocean-surface-ui` actually executed and the tree restored
//! verbatim in between. The right-hand column is the one that matters: nine of
//! the ten are caught by nothing in this repository except this file.
//!
//! | mutation                                                          | what went red |
//! |-------------------------------------------------------------------|---------------|
//! | `if !closed { … }` deleted — an `EventSource` on a corpse again    | `a_closed_room_opens_no_event_source_at_all`, and nothing else in the suite |
//! | `me.closed.set(closed)` deleted — flag read, never published       | `the_snapshots_closed_flag_reaches_the_open_room_call_site`, alone |
//! | `self.closed.set(false)` deleted from `reset_room_state`           | the same guard, alone |
//! | `post_message`'s gate back to `access_allows_writes` alone         | `the_composer_refuses_a_closed_room_and_says_so`, alone |
//! | `composer_writes_allowed`'s body reduced to the access gate        | the same guard, alone — every call site still compiles |
//! | ONE of the six composer sites back to the access gate              | the same guard's COUNT, alone — five correct sites hide it from everything else |
//! | the closed notice's `view!` block deleted                          | `a_closed_room_paints_a_reason_a_person_can_read`, alone |
//! | `#[serde(default)]` dropped from `closed`                          | three `rooms.rs` unit tests AND this file's first test |
//! | `can_retry` back to `failed` — a live Retry in the frozen pane      | `a_closed_rooms_outbox_paints_no_retry_and_dispatches_none`, alone |
//! | `retry_outbox`'s head guard deleted — the button gone, dispatch open | the same guard, alone |
//!
//! The last row is the only mutation something else already holds, and it is
//! asserted here anyway. Two of those three unit tests are fixtures that omit
//! `closed` incidentally, not on purpose — a wave that adds the key to them
//! while tidying would delete the only thing pinning the contract's
//! absent-means-open rule and never notice. (It is measured on its own run: the
//! unit-test binary fails first and cargo then never reaches the integration
//! binaries, so a single full-suite run cannot see both halves.)
//!
//! One process note, because it cost a full pass: the first attempt at this
//! table reverted each mutation by swapping the text back. One mutation's
//! replacement was a SUBSTRING of the original — deleting `if !closed { … }`
//! leaves the `start_live_tail` line it wrapped — so the reverse swap was a
//! no-op and the next seven mutations ran against a tree that still had the
//! first one applied. Every row came back RED for the same wrong reason. Revert
//! by writing the original bytes back, and check the failing test's NAME.

mod common;

use common::{read, view_source, without_whitespace};

/// The daemon's answer has to survive the decode and reach the one place that
/// can act on it. Mutations run: `me.closed.set(closed)` deleted, and
/// `self.closed.set(false)` deleted from `reset_room_state`. Both leave the
/// whole rest of the suite green — the tail gate reads the LOCAL, not the
/// signal, so a room with either line missing stops tailing correctly and keeps
/// a writable composer, which is the worse half of the bug wearing the fix's
/// clothes.
#[test]
fn the_snapshots_closed_flag_reaches_the_open_room_call_site() {
    let rooms = without_whitespace(&view_source("rooms.rs"));

    assert!(
        rooms.contains("#[serde(default)]closed:bool,"),
        "`closed` must decode with `#[serde(default)]`: the ecosystem contract \
         rules the field additive, so a daemon predating ocean-os#434 sends no \
         key and a missing key means OPEN. Without the default that body is a \
         decode error, and `open_room` reports decode errors as a failed \
         open — every room on a pre-field daemon refusing to load",
    );
    // The ARM, not the whole tuple. This assertion used to quote every element
    // of `open_room`'s success tuple, which made it red for the next slice that
    // carried one more field out of the same envelope — `agent_owners` was the
    // first, and it went red having broken nothing this file is about. What is
    // load-bearing is that `closed` leaves the response at all; the arm's other
    // passengers are somebody else's guard.
    let hydration_arm = rooms
        .split_once("Ok(r)ifr.ok=>Ok((")
        .and_then(|(_, rest)| rest.split_once("))"))
        .map(|(arm, _)| arm)
        .expect("`open_room`'s hydration decode arm builds a tuple");
    assert!(
        hydration_arm.contains("r.closed"),
        "the hydration decode arm must carry `closed` out with the record it \
         describes; a field decoded and left in the response is a field nothing \
         can gate on",
    );
    assert!(
        rooms.contains("me.closed.set(closed);"),
        "`open_room` must PUBLISH closedness — the tail gate below reads the \
         local, so dropping this line leaves the tail correctly stopped and the \
         composer wide open, which is the worse half of the bug wearing the \
         fix's clothes",
    );
    assert!(
        rooms.contains("self.closed.set(false);"),
        "`reset_room_state` must clear it. Both `open_room` and `close_room` \
         call that, so this is what stops one closed room's gate from outliving \
         it into the next room opened — which would present as a live room \
         nobody can type in",
    );
}

/// The tail gate, at the call site, by name. `start_live_tail` is an
/// unconditional reconnect loop — no argument, signal or branch inside it can
/// stop it, so the gate cannot live there and a guard naming the helper would
/// pin nothing. Mutation run: `if !closed { … }` deleted, restoring the
/// forever-reconnecting `EventSource`. Nothing else in the suite moved.
#[test]
fn a_closed_room_opens_no_event_source_at_all() {
    let rooms = without_whitespace(&view_source("rooms.rs"));

    assert!(
        rooms.contains("if!closed{me.start_live_tail(key,generation_id,resume_seq);}"),
        "hydration must not start the live tail for a closed room. `/events` \
         404s it and the loop inside `start_live_tail` treats every failure as \
         a reason to retry, so the only connection that never reconnects is the \
         one never opened — the gate belongs HERE, at the call site",
    );
    // COUNTED, not merely present: the needle above stays green while a second,
    // ungated `start_live_tail` sits elsewhere in the module doing exactly what
    // the gate forbids. One call site is the invariant.
    assert_eq!(
        rooms.matches("me.start_live_tail(").count(),
        1,
        "`start_live_tail` must have exactly ONE call site, so gating that site \
         gates the tail. If you have just added a caller, this count is the \
         ask, not the failure: say what stops YOUR call opening a socket \
         against a closed room, then bump the number",
    );
}

/// The write half. A room whose `POST .../messages` answers 404 must refuse the
/// send before it is dispatched, and must LOOK refused — a composer that
/// accepts a message and drops it reads as the message having been sent.
/// Mutations run: `post_message`'s gate reverted to `access_allows_writes`
/// alone, `composer_writes_allowed`'s body reduced to the same, and ONE of the
/// six composer sites reverted while the other five stayed correct. All three
/// compile, and all three leave the rest of the suite green.
#[test]
fn the_composer_refuses_a_closed_room_and_says_so() {
    let rooms = without_whitespace(&view_source("rooms.rs"));
    let workspace = without_whitespace(&view_source("rooms_workspace.rs"));

    assert!(
        rooms.contains(
            "if!composer_writes_allowed(self.access.get_untracked().as_ref(),\
             self.closed.get_untracked(),){return;}"
        ),
        "`post_message` must refuse a closed room at its head, the way it \
         already refuses a disallowed access state — a dispatched send answers \
         404 and lands in the status line as a transient error, which is not \
         what a permanently frozen room is",
    );
    assert!(
        workspace.contains(
            "pub(crate)fncomposer_writes_allowed(access:Option<&RoomAccessProjection>,\
             room_closed:bool,)->bool{access_allows_writes(access)&&!room_closed}"
        ),
        "the composer's gate must ask BOTH axes. Neither implies the other: \
         closing leaves the access row untouched, so a frozen room projects \
         `Local` with no access row and an unchanged `Live` when federated, \
         and `access_allows_writes` waves every send through in either shape",
    );
    // COUNTED: one definition plus six live sites — `post_message`, the two
    // send handlers, and the four `disabled=` bindings (main + thread input,
    // main + thread send button), one of which is the `rooms.rs` call. A
    // `contains` here stays green while five sites ask both questions and the
    // sixth asks one, which is precisely the shape of the bug.
    assert_eq!(
        workspace.matches("composer_writes_allowed(").count(),
        7,
        "every composer write path must go through the one gate: its definition \
         plus the two send handlers and the four `disabled=` bindings. If you \
         have just added a composer control, this count is the ask — route it \
         through `composer_writes_allowed`, then bump the number",
    );
    assert!(
        !workspace.contains("!access_allows_writes(rooms.access.get()"),
        "no composer control may still gate on access ALONE. The two surviving \
         `access_allows_writes(rooms.access.get()…)` reads are the invite and \
         repo rails' `Signal::derive`, which are not negated and are documented \
         as deliberately keeping the COMPOSER's access gate — a NEGATED one is a \
         `disabled=` binding or a send refusal, and those must ask both axes",
    );
}

/// The reason, on screen, where the dead input is. A disabled composer with no
/// explanation is indistinguishable from a broken one, and this is the only
/// thing a closed room paints that a live room does not — for an empty closed
/// room it is the only thing on the pane at all, since the transcript's own
/// empty state is suppressed until the tail reaches Live and this room's tail
/// never starts. Mutation run: the `view!` block deleted; the rest of the suite
/// stays green, because a component that renders nothing compiles fine.
#[test]
fn a_closed_room_paints_a_reason_a_person_can_read() {
    let workspace = without_whitespace(&view_source("rooms_workspace.rs"));
    let css = read("styles/rooms-workspace.css");

    assert!(
        workspace.contains("if!rooms.closed.get(){return().into_any();}"),
        "the closed notice must be gated on the room's own closedness and \
         render for nothing else",
    );
    assert!(
        workspace.contains("class=\"rooms-workspace__composer-closed\""),
        "the notice must be emitted from the composer, next to the input it \
         explains — not from the status line, which carries transient errors \
         and is cleared by the next one",
    );
    assert!(
        workspace.contains("Thisroomisclosed."),
        "the notice must say the room is closed in words. `closed` is a wire \
         field; what an operator needs is why their message will not send",
    );
    // Stylesheets live at the repo ROOT, never under the crate.
    assert!(
        css.contains(".rooms-workspace__composer-closed"),
        "the notice's class must be styled in styles/rooms-workspace.css — an \
         emitted class with no rule is an unstyled paragraph shoved against the \
         composer",
    );
}

/// The pane's OTHER write control, and the only one in this room that does not
/// fail loudly. `POST .../messages` 404s a closed room, so the composer's gate
/// is about telling the truth early; the retry route is a different animal —
/// the daemon's `retry_failed_outbox` gates on `SELECT 1 FROM rooms WHERE
/// id = ?1` with no `closed_at` filter, so a Retry pressed inside the frozen
/// audit view answers 202 and requeues a federated send out of a transcript
/// this pane calls finished. Both halves are pinned here because either alone
/// is a half-gate: the outbox itself stays on screen — it is part of the
/// record — while the button and the method behind it do not. Mutations run:
/// `can_retry` reverted to `failed`, and the `retry_outbox` head guard
/// deleted. Each compiles, each leaves the rest of the suite green, and each
/// fails only here.
#[test]
fn a_closed_rooms_outbox_paints_no_retry_and_dispatches_none() {
    let rooms = without_whitespace(&view_source("rooms.rs"));
    let workspace = without_whitespace(&view_source("rooms_workspace.rs"));

    assert!(
        workspace.contains("letroom_closed=rooms.closed.get();"),
        "the outbox block must read closedness reactively, inside the same \
         closure as `rooms.access.get()`. Read outside it, the pane keeps the \
         button until something else happens to touch `access`",
    );
    assert!(
        workspace.contains("letcan_retry=failed&&!room_closed;"),
        "the Retry button must ask the room's closedness as well as the item's \
         state. `failed` alone paints a live, SUCCEEDING write control in a \
         pane whose composer says nothing can be posted to this room",
    );
    assert!(
        workspace.contains("{ifcan_retry{view!{<buttonclass=\"rooms-workspace__outbox-retry\""),
        "`can_retry` must gate the BUTTON. Gating the outbox pane on it instead \
         would hide the frozen record itself, which is the half of this that \
         belongs in an audit view",
    );
    // COUNTED: an unpainted button is not a gate if a second call site paints
    // one. One call site is what makes the guard above load-bearing.
    assert_eq!(
        workspace.matches("rooms.retry_outbox(").count(),
        1,
        "`retry_outbox` must have exactly ONE call site in the workspace. If \
         you have just added a caller, this count is the ask: say what stops \
         YOUR call requeuing a federated send out of a closed room, then bump \
         the number",
    );
    assert!(
        rooms.contains(
            "pubfnretry_outbox(&self,client_event_id:String){\
             ifself.closed.get_untracked(){return;}"
        ),
        "`retry_outbox` must refuse a closed room at its head too, the way \
         `post_message` does. This route is the one write in the pane that \
         answers 202 rather than 404 when it is reached, so the dispatch — not \
         only the pixel — has to be held",
    );
}
