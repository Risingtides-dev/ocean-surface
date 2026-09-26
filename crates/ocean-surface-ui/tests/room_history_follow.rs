//! DoD 1.5, the reading half: live-follow never yanks a scrolled reader, and
//! return-to-latest re-pins.
//!
//! `transcript_pass_action` owns the RULE and its unit tests pin every arm. What
//! no unit test can say is whether the arms still DO what their names claim, or
//! whether anything still reaches the "↓ New messages" control. Measured before
//! this file existed: replacing the `RaiseJump` arm's body with the pin
//! (`el.set_scroll_top(el.scroll_height())`) compiles, keeps both clippy lanes
//! and every unit test green, and ships a transcript that throws a reader who
//! scrolled up back to the bottom on every live message. Deleting the jump
//! button is just as silent — `new_below` is an `RwSignal`, so losing its only
//! reader warns about nothing, and `queue_bottom_read_advance` keeps its other
//! caller in the `scroll` handler.
//!
//! `crates/ocean-surface-ui` is a binary crate, so these are source scans over
//! `common::view_source` (the half a release build compiles), with the two
//! rules `unheld_room_controls.rs` paid for: name the CALL SITE, and never let a
//! module's own test module satisfy a needle by quoting it.
//!
//! ## Measured, not assumed
//!
//! Each mutation applied alone to `src/rooms_workspace.rs`, this file run
//! against it, the source restored and touched before the next.
//!
//! | mutation                                                                 | result |
//! |--------------------------------------------------------------------------|--------|
//! | `RaiseJump` arm also pins (`set_scroll_top(scroll_height)` in a frame)    | RED    |
//! | `RaiseJump` arm stops raising the affordance (`new_below.set(false)`)     | RED    |
//! | `PinAndQueue` arm's scroll-to-bottom frame deleted                        | RED    |
//! | jump button deleted                                                       | RED    |
//! | jump button stops scrolling (its `set_scroll_top` removed)                | RED    |
//! | jump button stops clearing `new_below`                                    | RED    |
//! | `scroll` handler stops clearing `new_below` at the bottom                 | RED    |
//! | the Effect measures "at the bottom" at 400px instead of the handler's 120 | RED    |
//!
//! The first and fourth rows were also run against the whole unit suite and
//! both clippy lanes: all green, so this file is the only thing that objects.
//! The RULE — `transcript_pass_action` pinning any append (`|| len > prev_len`)
//! — is deliberately not scanned here: it reds three unit tests in
//! `rooms_workspace.rs`, including the DoD 1.5 sequence test beside them, and
//! leaves this file green, which is the right split.

mod common;

use common::{view_source, without_whitespace};

fn workspace() -> String {
    without_whitespace(&view_source("rooms_workspace.rs"))
}

/// A scrolled-up reader gets the affordance, never the scroll. The arm is
/// pinned WHOLE, because the failure is an addition to it, not a deletion.
#[test]
fn live_follow_raises_the_jump_and_never_moves_a_scrolled_reader() {
    let workspace = workspace();
    assert!(
        workspace.contains(
            "TranscriptPassAction::RaiseJump=>{new_below.set(true);pending_read_advance.set(None);}"
        ),
        "an append below a reader who scrolled up raises \"↓ New messages\", \
         queues no read, and touches the scroll position NOT AT ALL — a \
         `set_scroll_top` here is the yank DoD 1.5 forbids",
    );
    assert!(
        workspace.contains("TranscriptPassAction::Hold=>{}"),
        "a pass that is neither an append nor a reset (an access projection \
         arriving while the reader is up) does nothing",
    );
}

/// Every programmatic scroll write in the module is one of three: the pin, the
/// anchor that holds a reader still while an older page lands above, and the
/// jump button. A fourth is a new way to move a reader who did not ask to move.
#[test]
fn the_transcript_scrolls_itself_in_exactly_three_places() {
    let workspace = workspace();
    assert_eq!(
        workspace.matches("set_scroll_top(").count(),
        3,
        "a new programmatic scroll write in `rooms_workspace.rs` must be \
         reconciled with DoD 1.5 (\"live-follow never yanks a scrolled \
         reader\") and this count updated deliberately",
    );
    assert!(
        workspace.contains(
            "TranscriptPassAction::PinAndQueue=>{let(scroll_height,_,client_height)=metrics.unwrap_or_default();ifletSome(el)=el.clone(){request_animation_frame(move||el.set_scroll_top(el.scroll_height()));}new_below.set(false);"
        ),
        "the pin: a first fill (which is what opens a room at its newest \
         message) and an at-bottom reader follow the tail, and the \
         affordance goes away",
    );
    assert!(
        workspace.contains("el.set_scroll_top(anchored_top+grown);"),
        "the older-page anchor",
    );
}

/// Return-to-latest: the jump button takes the reader to the newest row and
/// clears the affordance, and from there the NEXT append takes the pin arm
/// again because "at the bottom" is measured, not remembered.
#[test]
fn return_to_latest_scrolls_to_the_newest_row_and_re_pins() {
    let workspace = workspace();
    assert!(
        workspace.contains(
            "{move||new_below.get().then(||view!{<buttontype=\"button\"class=\"rooms-workspace__jump-new\"on:click=move|_|{ifletSome(el)=list_ref.get(){el.set_scroll_top(el.scroll_height());}new_below.set(false);queue_bottom_read_advance();}>"
        ),
        "the \"↓ New messages\" control renders while appends are waiting \
         below, scrolls to the newest row, clears itself, and marks what it \
         brought on screen read",
    );
    assert!(
        workspace.contains(
            "on:scroll=move|_|{ifletSome(el)=list_ref.get(){iftranscript_is_near_bottom(el.scroll_height(),el.scroll_top(),el.client_height(),120,){new_below.set(false);queue_bottom_read_advance();}}}"
        ),
        "scrolling back down by hand is the same return: the affordance \
         clears the moment the reader is at the bottom",
    );
    assert!(
        workspace.contains(
            "letnear_bottom=metrics.is_some_and(|(scroll_height,scroll_top,client_height)|{transcript_is_near_bottom(scroll_height,scroll_top,client_height,120)});"
        ),
        "the pass that decides pin-or-jump measures \"at the bottom\" with the \
         SAME threshold the scroll handler clears the affordance at — otherwise \
         a reader the handler calls returned is still `RaiseJump` to the \
         Effect, and the follow never re-pins",
    );
}
