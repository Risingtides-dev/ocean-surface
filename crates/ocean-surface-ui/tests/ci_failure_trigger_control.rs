//! The create panel's CI-failure checkbox has no compiler guard, so it gets a
//! source assertion instead.
//!
//! The other two halves of this control are already held by something that
//! fails loudly. The rail row is held by the compiler: `TriggerToggle` is a
//! private enum whose `CiFailure` variant is CONSTRUCTED only there in a
//! non-test build — `policy_with_toggle` and `trigger_row_dead_here` merely
//! match on it — so deleting the row makes the release lane's
//! `RUSTFLAGS="-D warnings"` wasm check fail on `variant is never constructed`.
//! The summary line is held by `trigger_summary`, a free function a native
//! unit test asserts over, and whose own call site is the only thing keeping it
//! from being dead code on that same lane.
//!
//! The checkbox is held by neither. `create_on_ci_failure` stays read by
//! `create_room` and by the draft reset whether or not anything can set it, so
//! deleting the `<label>` leaves a room-create form that silently cannot opt in
//! and a build that is entirely green. That is not hypothetical: the create
//! panel's checkbox and the summary's line were both deleted during review of
//! the change that added this flag's mirror, and the full suite plus the wasm
//! check stayed green.
//!
//! Whitespace is stripped wholesale before matching, the same trick
//! `dead_trigger_row_affordance.rs` uses on CSS, so rustfmt is free to wrap
//! these lines however it likes without breaking the test. It does mean the
//! needles read without their spaces — `"CIfailure"` is the label `"CI
//! failure"`.

fn rooms_workspace_source() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/rooms_workspace.rs");
    std::fs::read_to_string(path).expect("read src/rooms_workspace.rs")
}

fn without_whitespace(source: &str) -> String {
    source
        .chars()
        .filter(|char| !char.is_whitespace())
        .collect()
}

#[test]
fn the_create_panel_offers_a_ci_failure_checkbox() {
    let source = without_whitespace(&rooms_workspace_source());
    assert!(
        source.contains("prop:checked=move||create_on_ci_failure.get()"),
        "the create panel's CI-failure checkbox must read the draft signal — \
         without it the box cannot show what the draft holds",
    );
    assert!(
        source.contains("create_on_ci_failure.set(event_target_checked(&ev))"),
        "the create panel's CI-failure checkbox must write the draft signal — \
         without it the box is inert and the room is created with the flag off",
    );
    assert!(
        source.contains(r#"<spanclass="rooms-workspace__trigger-label">"CIfailure"</span>"#),
        "the CI-failure checkbox needs the same label markup as its three \
         neighbours, or it renders unlabelled",
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

/// The rail says `CI failure` and the summary says `CI failure`. `CI` is an
/// initialism, so it stays capitalized in both — unlike `@mention`, `thread
/// reply` and `build failure`, which are lowercase words. A rail and a summary
/// disagreeing about the casing of the same trigger is exactly the drift this
/// pins.
///
/// Only the rail needs pinning HERE. The summary's casing is a return value,
/// so `trigger_summary_names_every_live_flag_that_is_on` already holds it. So
/// the positive assert names the rail's CALL SITE rather than the bare
/// literal: a file-wide search for `"CI failure"` is satisfied by the
/// summary's own `on.push`, and by that file's test module quoting the
/// summary's output, while the rail says whatever it likes. The scan stops at
/// the test module for the same reason.
#[test]
fn the_ci_failure_label_is_capitalized_everywhere() {
    let source = rooms_workspace_source();
    assert!(
        !source.contains("\"ci failure\""),
        "`ci failure` is the wrong casing for an initialism; both the rail row \
         and the policy summary say `CI failure`",
    );
    let view = source
        .split_once("#[cfg(test)]")
        .expect("rooms_workspace.rs carries its unit tests at the bottom")
        .0;
    assert!(
        without_whitespace(view).contains(r#"TriggerToggle::CiFailure,"CIfailure","#),
        "the rail row must label this trigger `CI failure`, the same casing \
         the summary returns",
    );
}
