//! The create panel's CI-failure checkbox has no compiler guard, so it gets a
//! source assertion instead — and so, since the summary learned about access,
//! do the four rail rows.
//!
//! The rail rows WERE held by the compiler. `TriggerToggle` is a private enum,
//! and while the rows were the only place a non-test build constructed its
//! variants — `policy_with_toggle` and `trigger_row_dead_here` merely match on
//! them — deleting a row made the release lane's `RUSTFLAGS="-D warnings"` wasm
//! check fail on `variant is never constructed`. `trigger_summary`'s flag table
//! constructs all four in that same build now, so the rows are no longer the
//! sole construction site and a deleted row compiles clean. That guard is gone
//! and is not coming back; `the_rail_offers_a_row_for_every_live_trigger`
//! replaces it.
//!
//! The summary LINE is still held twice over: `trigger_summary` is a free
//! function a native unit test asserts over, and its own call site is the only
//! thing keeping it from being dead code on that lane. Its ARGUMENTS are held
//! by neither — handing it `None` instead of the room's access projection
//! compiles clean, passes every unit test (they call the function directly),
//! and silently restores a summary that names flags the rail two inches above
//! it has just greyed out. Same failure shape as a wrong create-time draft
//! below, so it gets the same answer.
//!
//! The checkbox is held by neither. `create_on_ci_failure` stays read by
//! `create_room` and by the draft reset whether or not anything can set it, so
//! deleting its row leaves a room-create form that silently cannot opt in and
//! a build that is entirely green. That is not hypothetical: the create
//! panel's checkbox and the summary's line were both deleted during review of
//! the change that added this flag's mirror, and the full suite plus the wasm
//! check stayed green. The create rows render through one shared helper now —
//! they grew a dead-here note and the rail's `<label>` was the only sane place
//! to put it — so the guard follows them: the call sites pin that each row
//! exists carrying its OWN draft signal, and the helper pins that a row reads
//! and writes the signal it is handed at all.
//!
//! Whitespace is stripped wholesale before matching, the same trick
//! `dead_trigger_row_affordance.rs` uses on CSS, so rustfmt is free to wrap
//! these lines however it likes without breaking the test. It does mean the
//! needles read without their spaces — `"CIfailure"` is the label `"CI
//! failure"`.
//!
//! The scanners themselves live in `tests/common/mod.rs` now. This file grew
//! them one incident at a time and `unheld_room_controls.rs` needed the same
//! three; `view_source` takes a module path there, and nothing else changed.

mod common;

use common::{view_source, without_whitespace};

fn rooms_workspace_source() -> String {
    common::src("rooms_workspace.rs")
}

#[test]
fn the_create_panel_offers_a_row_for_every_live_trigger() {
    let source = view_source("rooms_workspace.rs");
    let view = without_whitespace(&source);
    for (variant, label, draft) in [
        ("Mention", r#""@mention""#, "create_on_mention"),
        ("ThreadReply", r#""threadreply""#, "create_on_thread_reply"),
        (
            "BuildFailure",
            r#""buildfailure""#,
            "create_on_build_failure",
        ),
        ("CiFailure", r#""CIfailure""#, "create_on_ci_failure"),
    ] {
        assert!(
            view.contains(&format!(
                "create_trigger_row(TriggerToggle::{variant},{label},{draft},"
            )),
            "the create panel must render a row for `TriggerToggle::{variant}` \
             carrying its OWN draft signal; a missing row leaves a form that \
             silently cannot opt in, and a swapped one mirrors the wrong box",
        );
    }

    // And the shared row has to be a control at all. Sliced to the helper
    // because `trigger_toggle_row` renders byte-identical label markup — a
    // file-wide scan for it would be satisfied by the rail while the create
    // rows render nothing.
    let row_at = source
        .find("fn create_trigger_row(")
        .expect("the create rows must render from a helper in rooms_workspace.rs");
    let row = &source[row_at..];
    let row = without_whitespace(&row[..row.find("\nfn ").unwrap_or(row.len())]);
    assert!(
        row.contains("prop:checked=move||flag.get()"),
        "the create row must read the draft signal it is handed — without it \
         no box can show what the draft holds",
    );
    assert!(
        row.contains("flag.set(event_target_checked(&ev))"),
        "the create row must write the draft signal it is handed — without it \
         every box is inert and the room is created with the flag off",
    );
    assert!(
        row.contains(r#"<spanclass="rooms-workspace__trigger-label">{label}</span>"#),
        "the create rows need the same label markup as the rail's, or they \
         render unlabelled",
    );
}

/// The draft signal has to reach the create call, or the checkbox is a control
/// over nothing. `create_trigger_policy`'s arity change makes a MISSING
/// argument a compile error, but a wrong one — passing `create_on_mention`
/// twice — compiles clean and silently mirrors the wrong box.
#[test]
fn the_create_call_passes_the_ci_failure_draft() {
    let source = without_whitespace(&rooms_workspace_source());
    assert!(
        source.contains(
            "create_trigger_policy(create_on_mention.get_untracked(),\
             create_on_thread_reply.get_untracked(),\
             create_on_build_failure.get_untracked(),\
             create_on_ci_failure.get_untracked(),)"
        ),
        "each create-time toggle must pass its OWN draft signal, in the order \
         `create_trigger_policy` declares",
    );
}

/// The rail's four rows, pinned by source because nothing pins them any more.
/// Each row is a trigger's only live affordance — the create panel's checkboxes
/// set a draft, not an open room — so a row deleted the way the compiler used
/// to forbid leaves a flag a person can read in the summary and never turn off.
///
/// The needle stops after the label rather than running to the end of the call.
/// What is held here is that the row EXISTS and says the same word the summary
/// says; running on through the reactive `flag(..)` closure would break on any
/// tidy-up of it without holding anything the compiler is not already holding.
#[test]
fn the_rail_offers_a_row_for_every_live_trigger() {
    let view = without_whitespace(&view_source("rooms_workspace.rs"));
    for (variant, label) in [
        ("Mention", r#""@mention""#),
        ("ThreadReply", r#""threadreply""#),
        ("BuildFailure", r#""buildfailure""#),
        ("CiFailure", r#""CIfailure""#),
    ] {
        assert!(
            view.contains(&format!(
                "trigger_toggle_row(rooms,TriggerToggle::{variant},{label},"
            )),
            "the trigger rail must render a row for `TriggerToggle::{variant}`; \
             without it the flag stays stored, stays summarized, and cannot be \
             switched off",
        );
    }
}

/// `trigger_summary` takes this room's access projection so a flag that cannot
/// fire here carries the same note its row above it shows. Handing it `None`
/// compiles clean and both unit tests over that function keep passing, because
/// they call it directly and choose their own argument — the summary goes
/// straight back to naming flags the rail has just greyed out. Same shape as a
/// create call passing the wrong draft signal, so it gets the same answer.
#[test]
fn the_summary_call_site_passes_the_access_projection() {
    let view = without_whitespace(&view_source("rooms_workspace.rs"));
    assert!(
        view.contains("trigger_summary(&p,access.as_ref())"),
        "the Response Policy summary must be handed the open room's access \
         projection, or it contradicts the trigger rail two inches above it",
    );
}

/// The rail says `CI failure` and the summary says `CI failure`. `CI` is an
/// initialism, so it stays capitalized in both — unlike `@mention`, `thread
/// reply` and `build failure`, which are lowercase words. A rail and a summary
/// disagreeing about the casing of the same trigger is exactly the drift this
/// pins.
///
/// Only the rail needs pinning HERE. The summary's casing is a return value,
/// so `trigger_summary_names_every_live_flag_that_is_on` already holds it. So
/// the positive assert names the rail's CALL SITE rather than the bare
/// literal: a file-wide search for `"CI failure"` is satisfied by the label in
/// `trigger_summary`'s own flag table, and by that file's test module quoting
/// the summary's output, while the rail says whatever it likes. The scan stops
/// at the test module for the same reason.
#[test]
fn the_ci_failure_label_is_capitalized_everywhere() {
    assert!(
        !rooms_workspace_source().contains("\"ci failure\""),
        "`ci failure` is the wrong casing for an initialism; both the rail row \
         and the policy summary say `CI failure`",
    );
    assert!(
        without_whitespace(&view_source("rooms_workspace.rs"))
            .contains(r#"TriggerToggle::CiFailure,"CIfailure","#),
        "the rail row must label this trigger `CI failure`, the same casing \
         the summary returns",
    );
}
