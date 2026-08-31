//! Room controls that measurement proves nothing else holds.
//!
//! The failure this repo keeps having: a reviewer deletes a control, every
//! gate stays green, and a landed daemon route goes back to being unreachable.
//! It is not hypothetical — #165 deleted the create panel's CI-failure
//! checkbox and the Response Policy summary line, and the full suite plus the
//! wasm check said nothing. `ci_failure_trigger_control.rs` is that incident's
//! guard; this file is the general lane behind it.
//!
//! ## Measured, not assumed
//!
//! Every control below was mutated for real at `4ed9a7c` and the gate actually
//! run. Only what stayed GREEN under mutation is pinned here — a control the
//! compiler already holds gets a redundant guard that costs maintenance and
//! buys nothing, so the ones that turned out to be held are recorded at the
//! bottom of this file instead of pinned.
//!
//! The split is not where intuition puts it. In the SAME panel, `provision`
//! is compiler-held (`error: variant `Provision` is never constructed`) while
//! `destroy`'s arming click is not; in the SAME component, the summarize RUN
//! button is compiler-held (deleting it takes `SummarizeRequest`,
//! `summarize_url`, `classify_summarize` and `SummarizeOutcome` dead with it)
//! while the `open` button that is the only way to reach it is not. That is
//! why each test below says which mutation was run and what stayed green.
//!
//! ## The shape that hides
//!
//! Four of the six are the ARMING half of a two-click confirm, and they are
//! silent for one reason. Deleting the arm leaves the confirm branch standing,
//! so the enum variant is still constructed, the fire method is still called,
//! and the signal is still read and reset. Nothing is unreferenced; nothing
//! warns. What is gone is the only way to reach the confirm — the destructive
//! verb is still fully implemented and permanently unpressable.
//!
//! ## Two rules the last two waves paid for
//!
//! **Name the CALL SITE, not the literal.** A bare-literal assert is satisfied
//! by the module's own test quoting it. Measured in this tree:
//! `state.confirm_destroy.set(true)` occurs 5x in `room_workspace_panel.rs`
//! and once outside its test module; `state.confirm_purge.set(Some(
//! PurgeTarget::All))` occurs 3x and once. So every needle here carries its
//! `on:click=` prefix, and every scan runs over `common::view_source`, which
//! truncates at `#[cfg(test)]`. Truncating alone is not enough and neither is
//! the prefix alone — both, or the control can be renamed to anything.
//!
//! **A guard is pinned by a RENAME as well as a deletion.** Each test below
//! was verified both ways: the control deleted (or its handler stripped), and
//! the view's occurrence renamed while the test module kept the old spelling.

mod common;

use common::{view_source, without_whitespace};

/// The rail's `open` button is the ONLY way into the summary panel, and the
/// panel is the only place the summarize control lives. Deleting it therefore
/// takes the whole `POST /v1/rooms/persistent/{key}/summarize` lane off the
/// browser — and both gates stay green, because nothing it touches goes
/// unreferenced: `state.panel` is still read by the panel's own render and
/// still written by `close_panel`, and `state.open_ref` is still used by
/// `summary_escape_closes` over in `rooms_workspace.rs`.
///
/// The contrast is the point. The summarize RUN button one level in IS
/// compiler-held; this one, the door to it, is not.
#[test]
fn the_summary_rail_offers_the_only_door_to_the_summarize_panel() {
    let view = without_whitespace(&view_source("room_summary.rs"));
    assert!(
        view.contains("on:click=move|_|{state.error.set(None);state.panel.set(true);}"),
        "the summary rail's `open` button is the only way into the panel the \
         summarize control lives in; without it the room's summarize route is \
         unreachable from the browser and every gate stays green",
    );
}

/// Unbinding a repo deletes the room's workspace checkout, so it sits behind a
/// two-click confirm. The first click — this one — arms it.
///
/// Nothing holds it. `confirm_unbind` is still read by the `if` this button
/// sits in the `else` of, and still written `false` by the confirm branch and
/// by `reset`; `unbind_repo` is still called from the confirm branch. Delete
/// the arm and unbind is fully implemented, fully wired, and permanently
/// unreachable.
#[test]
fn the_repo_section_offers_the_click_that_arms_unbind() {
    let view = without_whitespace(&view_source("room_repo.rs"));
    assert!(
        view.contains("on:click=move|_|state.confirm_unbind.set(true)"),
        "the repo section's `unbind\u{2026}` button is the only thing that arms \
         the unbind confirm; without it the confirm branch can never render",
    );
}

/// Destroying a room's container flushes it back to Bedrock and discards it,
/// so it too sits behind a two-click confirm, and this is the arming click.
///
/// Silent for the same reason as unbind: `confirm_destroy` stays read by the
/// branch above and reset in three other places, and `run_lifecycle` stays
/// called by the confirm branch — which is also what keeps
/// `WorkspaceCommand::Destroy` constructed. Its sibling `provision` has no
/// confirm, so deleting THAT button leaves `WorkspaceCommand::Provision`
/// unconstructed and the wasm clippy lane catches it. One panel, two verbs,
/// only one of them held.
#[test]
fn the_compute_section_offers_the_click_that_arms_destroy() {
    let view = without_whitespace(&view_source("room_workspace_panel.rs"));
    assert!(
        view.contains("on:click=move|_|state.confirm_destroy.set(true)"),
        "the compute section's `destroy\u{2026}` button is the only thing that \
         arms the destroy confirm; without it a room's container can never be \
         destroyed from the room",
    );
}

/// Taking back every finished command's stored output un-publishes it for
/// everyone at once, so it is armed before it fires. This is the arming click.
///
/// `PurgeTarget::All` survives the deletion because the confirm branch's own
/// `== Some(PurgeTarget::All)` comparison still constructs it, and
/// `run_exec_purge` survives because that branch still calls it. The needle
/// carries `on:click=` for a measured reason: the bare
/// `state.confirm_purge.set(Some(PurgeTarget::All))` appears three times in
/// this file and only once outside the test module.
#[test]
fn the_exec_section_offers_the_click_that_arms_a_full_purge() {
    let view = without_whitespace(&view_source("room_workspace_panel.rs"));
    assert!(
        view.contains("on:click=move|_|{state.confirm_purge.set(Some(PurgeTarget::All));}"),
        "the exec section's `take back output\u{2026}` button is the only thing \
         that arms the all-rows purge confirm; without it stored output can \
         never be taken back for the room",
    );
}

/// Both rosters — the participant list and the member list — render the same
/// arming button, so the count is the assertion: two live rosters, two arms.
/// Deleting either drops it to one.
///
/// Neither is held. `member_remove_armed` stays read by both `armed`
/// predicates and reset by both confirm branches, `remove_participant` and
/// `remove_member` stay called from those branches, and `removable` stays read
/// by the predicate — so a roster losing its remove affordance leaves a room
/// whose members cannot be removed and a build with nothing to say about it.
#[test]
fn both_rosters_offer_the_click_that_arms_a_removal() {
    let view = without_whitespace(&view_source("rooms_workspace.rs"));
    let arms = view
        .matches("on:click=move|_|{member_remove_armed.set(Some(arm_id.clone()))}")
        .count();
    assert_eq!(
        arms, 2,
        "the participant roster and the member roster each need the button \
         that arms a removal; one of them has lost it",
    );
}

/// A control can be deleted without its markup going anywhere. Strip
/// `on:click` off the redeem button and the `<button>` still renders, still
/// carries `rooms-workspace__redeem-run`, still says "join" — and does
/// nothing, because `fire` stays alive on the input's Enter handler and
/// nothing warns.
///
/// `room_redeem.rs`'s own guard, `the_view_renders_the_control_the_stylesheet_
/// dresses`, scans for that class literal and so passes straight through this
/// mutation. It holds that the button EXISTS; this holds that it does
/// something. Both are needed, which is why this assertion lives here rather
/// than being folded into that one.
#[test]
fn the_redeem_button_is_wired_and_not_just_rendered() {
    let view = without_whitespace(&view_source("room_redeem.rs"));
    assert!(
        view.contains(r#"class="rooms-workspace__redeem-run""#),
        "the redeem control's button must still render",
    );
    assert!(
        view.contains("on:click=move|_|fire()"),
        "the redeem button must still CALL `fire` — stripping the handler \
         leaves an inert button that looks exactly like a working one, and \
         `fire` stays alive on the input's Enter handler so nothing warns",
    );
}

// ---- Measured and already held: recorded, deliberately not pinned -----------
//
// Each of these was mutated the same way and the gate went RED on its own, so
// a guard here would be dead weight. Recorded because "the compiler holds it"
// is a fact with a shelf life — the rail rows in `ci_failure_trigger_control.rs`
// were compiler-held until a flag table elsewhere started constructing the same
// variants, and that guard exists because the hold evaporated.
//
//   room_summary.rs — the summarize RUN button.
//     `cargo clippy --target wasm32-unknown-unknown -- -D warnings` fails with
//     `struct SummarizeRequest is never constructed`, `function summarize_url
//     is never used`, `enum SummarizeOutcome is never used`, `function
//     classify_summarize is never used`, `unused variable: can_summarize`.
//     The button is the sole non-test caller of `RoomSummaryState::summarize`,
//     and that method is the only thing keeping the request pipeline alive.
//
//   room_workspace_panel.rs — the `provision` button.
//     Fails with `variant Provision is never constructed`. `WorkspaceCommand::
//     Provision` is only ever MATCHED elsewhere; this button is its only
//     construction site outside the test module. Note how narrow that hold is:
//     the day anything else constructs the variant, it is gone.
//
//   room_redeem.rs — the redeem button's MARKUP.
//     Deleting the whole `<button>` fails `cargo test` on that module's own
//     `the_view_renders_the_control_the_stylesheet_dresses`, which scans for
//     the class literal. Held by a test rather than the compiler — and only
//     against deletion, which is why the handler gets the assertion above.
