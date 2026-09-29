//! Source guard: the mention notifier fires from the LIVE TAIL and nowhere
//! else.
//!
//! `mention_notification_is_due` is a pure function with its own table test,
//! so what it decides is held by the compiler and by that table. What neither
//! holds is WHERE it is asked. The transcript is written from three places —
//! the room-scoped SSE tail, the hydration snapshot, and the "load older"
//! backfill walk (#192) — and only the first is someone talking to you now.
//! Move the call, or add a second one, and every gate in this repo stays
//! green while opening a long room replays months of old mentions as OS
//! notifications, one per row, all at once. That failure is loud for the
//! reader and silent for CI, which is the shape a scanner exists for.
//!
//! The needles name the CALL, not a literal: a bare-literal assertion would be
//! satisfied by `rooms.rs`'s own test module quoting the same function name,
//! and this scan runs over `view_source` for the same reason.

mod common;

use common::{view_source, without_whitespace};

/// The tail's `Message` arm asks, and it is the only asker.
#[test]
fn only_the_live_tail_asks_whether_a_row_notifies() {
    let view = view_source("rooms.rs");
    let dense = without_whitespace(&view);

    assert_eq!(
        dense.matches("mention_notification_is_due(").count(),
        2,
        "expected the definition plus exactly ONE call site; hydration and the \
         load-older backfill must never ask, because history arriving is not \
         someone talking to you now",
    );

    // The one call site is inside the tail's Message arm. Slice the arm out
    // and require the call to be in it — a call that drifted to another arm,
    // or up into the reconnect loop, would still count as one above.
    const HEAD: &str = "RoomTailFrame::Message(entry)=>{";
    let arm_start = dense
        .find(HEAD)
        .expect("the live tail must still decode a Message frame");
    let body = &dense[arm_start + HEAD.len()..];
    // The arm runs to the next `RoomTailFrame::` match arm, or to the end.
    let arm = match body.find("RoomTailFrame::") {
        Some(next) => &body[..next],
        None => body,
    };
    assert!(
        arm.contains("mention_notification_is_due("),
        "the mention decision belongs to the tail's Message arm",
    );
    assert!(
        arm.contains("crate::host::notify_with_focus("),
        "a decision nothing acts on notifies nobody; the arm must call the \
         host notifier",
    );

    // Visibility, not just the open key. `open_key` outlives the Rooms
    // workspace unmounting behind Direct messages and the tail keeps running,
    // so reading it alone suppressed every mention for a reader who could not
    // see the room. The tail must hand the predicate what is ON SCREEN.
    assert!(
        arm.contains("me.workspace_visible.get_untracked()"),
        "the tail must pass Rooms' on-screen state, not just the open key",
    );
    // The click has to reveal Rooms, and unconditionally: when the reader is
    // behind Direct messages the room is already `open_key`, so a
    // reopen-if-different check alone navigates nowhere and the click does
    // nothing visible.
    assert!(
        arm.contains("me.reveal_request"),
        "the notification's click must ask app.rs to reveal Rooms, which owns \
         the competing-surface closures",
    );

    // The dedupe gate. A resumed tail redelivers a seq already on screen, and
    // a notification per redelivery is how a reconnect becomes a burst.
    assert!(
        arm.contains(".filter(|_|appended)"),
        "the notification must fire only when the row was actually appended, \
         so a resumed tail cannot ping twice for one message",
    );
}

/// The reader's own ids — not the roster — are what the tokenizer is asked
/// about. Handing it the roster would answer "did anyone get named".
#[test]
fn the_notifier_asks_about_the_reader_and_not_the_roster() {
    let view = view_source("rooms.rs");
    let dense = without_whitespace(&view);
    assert!(
        dense.contains("letreader_ids=me.reader_member_ids();")
            && dense.contains("mention_notification_is_due(&entry,&reader_ids,"),
        "the tail must build the READER's id set and hand exactly that to the \
         mention test; the room roster would answer 'did anyone get named'",
    );
    // Both of the reader's ids, and only those. Dropping either half silently
    // stops notifying a whole class of member: the local identity is what a
    // G1 room names, and `self_member_id` is what a federated one names.
    let ids = dense
        .split_once("fnreader_member_ids(&self)->HashSet<String>{")
        .expect("the reader's id set must still have a builder")
        .1;
    // Whitespace is stripped, so the function's end is found by the next
    // `fn`, not by a newline — a `\n}` needle here would match nothing and
    // quietly widen the scan to the rest of the file.
    let ids = &ids[..ids.find("fn").unwrap_or(ids.len())];
    assert!(
        ids.contains("self.identity_id.get_untracked()") && ids.contains("self_member_id"),
        "the reader's ids are the local identity AND the access projection's \
         self member id",
    );
}

/// The reveal request is honoured by `app.rs`, and only there. Setting
/// `show_rooms` from the notification's click site would skip the
/// competing-surface closures the reveal lifecycle requires (AGENTS.md
/// 222-227), and the signal exists precisely because `rooms.rs` sits below
/// those signals and cannot reach them.
#[test]
fn the_reveal_request_is_answered_by_the_app_that_owns_the_reveals() {
    let app = view_source("app.rs");
    let dense = without_whitespace(&app);
    assert!(
        dense.contains("rooms.reveal_request.get()"),
        "app.rs must observe the reveal request",
    );
    assert!(
        dense.contains("show_sessions.set(false);show_rooms.set(true);"),
        "answering it must close the competing surface and reveal Rooms — the \
         same pair the ocean://room deep link takes",
    );
    assert!(
        dense.contains("rooms.workspace_visible.set(show_rooms.get())"),
        "app.rs must mirror Rooms' on-screen state onto the handle; the tail \
         has no other way to learn it",
    );
    // Names the WRITE, not the word: `rooms.rs` documents `show_rooms` in
    // prose (it has to, to explain why `open_key` is not visibility), and a
    // bare-literal ban would fail on the explanation rather than on a reach.
    assert!(
        !without_whitespace(&view_source("rooms.rs")).contains("show_rooms.set("),
        "rooms.rs must not write the reveal signals directly; it is below them \
         and would skip the competing-surface closures",
    );
}

/// One grammar. The notifier's predicate is the highlighter's tokenizer, so a
/// second hand-rolled `@`-scan appearing anywhere in `rooms.rs` would be a
/// copy that drifts.
#[test]
fn the_mention_test_stays_the_highlighters_tokenizer() {
    let markdown = view_source("room_markdown.rs");
    assert!(
        markdown.contains("pub fn mentions_member("),
        "the predicate lives beside the tokenizer it runs",
    );
    assert!(
        markdown.contains("tokenize(body, ids)"),
        "mentions_member must run the same tokenizer the renderer runs, not a \
         second scan of its own",
    );
    assert!(
        !without_whitespace(&view_source("rooms.rs")).contains("is_mention_char("),
        "rooms.rs must not grow its own mention scanner beside the tokenizer's",
    );
}
