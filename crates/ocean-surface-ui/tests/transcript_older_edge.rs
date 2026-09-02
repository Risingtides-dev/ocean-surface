//! The two things a transcript's older edge still could not say after #192.
//!
//! #190 anchored hydration at a room's NEWEST page and walked back a bounded
//! 1000 + 5×200 rows, and wrote both consequences into `rooms.rs` itself. #192
//! closed the first: the walk parks its cursor, a press replays one page, and a
//! long room's older half is reachable again. Two gaps survived it, and this
//! file pins their closure.
//!
//! **A room that reached the beginning could not say so.** The affordance
//! rendered on `older_cursor.is_some()`, so `None` rendered nothing — and `None`
//! is four situations: no room open, hydration still walking, a page that
//! provably reached the start of the log, and a walk that never ran. A room
//! holding its entire history looked exactly like one three seconds into
//! hydration, and the claim an operator wanted — this row IS the first message
//! in this room — was the one the transcript could not make. `OlderHistory` is
//! three states now, and `older_settled` is the fact that separates them.
//!
//! **A reply whose ROOT fell outside the window rendered NOWHERE.** The main
//! list was `partition_thread_messages(transcript, 0).roots`, which drops every
//! row carrying a `thread_parent_seq`; the thread pane could not take it either,
//! because `thread_root_for` finds no root and `sync_thread_selection` clears
//! the selection the moment it is made. A reply at seq 2500 to a root at seq 800
//! was loaded, dropped from both, and invisible with nothing implying it
//! existed. #192 said so explicitly and declined to grow into it: pressing
//! "load older" is not an answer, because the walk goes back a page at a time
//! and cannot jump to one named root. The answer here is the cheaper of the two
//! the brief allowed — the reply renders INLINE, at its own position in time,
//! carrying a note that says why it has no thread above it.
//!
//! ## Why a scanner
//!
//! Every rule here is a pure function with unit tests in `rooms.rs` and
//! `rooms_workspace.rs`. What no unit test in a BINARY crate can say is whether
//! anything still CALLS them: `crates/ocean-surface-ui` is `src/main.rs` with no
//! `[lib]`, so a test in this directory cannot mount the workspace or read a
//! rendered row (`tests/common/mod.rs` says so at length). The two rules
//! `unheld_room_controls.rs` paid for hold here — name the CALL SITE, and scan
//! `common::view_source`, which truncates at `#[cfg(test)]` so a module's own
//! unit tests cannot satisfy a needle by quoting it.
//!
//! ## Measured, not assumed
//!
//! Seven mutations applied for real against this tree, each ALONE, with this
//! file and `cargo test -p ocean-surface-ui` both executed and the tree restored
//! verbatim in between. Where a row claims something about another lane, that
//! lane was run too.
//!
//! | mutation                                                            | result |
//! |----------------------------------------------------------------------|--------|
//! | `settle_older_cursor` stops setting `older_settled`                   | RED here, ALONE — and every unit test stays green (run). The pure decider's tests own the rule and never its wiring, which is why the settle is pinned at its call site rather than trusted to them |
//! | `reset_room_state` stops clearing `older_settled`                     | RED here, alone — the next room claims a beginning it has not read |
//! | `open_room`'s short-page arm back to `if let` (no `None` body)         | RED here, and `room_hydration_resume.rs`, which names that arm too — every room fitting in one page otherwise renders the hydrating silence forever |
//! | the `ReachedBeginning` arm replaced with `().into_any()`               | RED here, alone — the affordance is back to two states wearing three states' clothes |
//! | the `<For>`'s list back to `partition_thread_messages(…, 0).roots`     | RED here (two of this file's tests), and `room_load_older_affordance.rs`, whose position anchor names the same list — the orphan disappears again |
//! | the empty state's list back to `partition_thread_messages(…, 0).roots` | RED here, alone — "No messages yet" over visible rows |
//! | the orphan note's block deleted                                        | RED here — but so is the wasm lane: `orphaned_reply_note` loses its only non-test caller and `cargo clippy --target wasm32-unknown-unknown -- -D warnings` fails with `unused variable: orphan_row` and `function orphaned_reply_note is never used`. Kept for the message, not the coverage |

mod common;

use common::{read, view_source, without_whitespace};

/// The settled flag has to be written where a page ANSWERS and cleared where a
/// room is left, or the third state is unreachable in one direction and wrong in
/// the other.
#[test]
fn a_backward_page_that_answers_settles_and_a_room_switch_unsettles() {
    let rooms = without_whitespace(&view_source("rooms.rs"));

    assert!(
        rooms.contains(
            "fnsettle_older_cursor(&self,cursor:Option<u64>){self.older_cursor.set(cursor);self.older_settled.set(true);}"
        ),
        "the cursor and the fact that a read answered are ONE fact and must be \
         written together. Split, a `None` cursor parked without the flag reads \
         as `nothing has asked yet` forever — which is the silence this slice \
         exists to remove, restored through the back door",
    );
    assert!(
        rooms.contains("self.older_settled.set(false);"),
        "`reset_room_state` must clear it. Both `open_room` and `close_room` \
         call that, so without it the next room opened claims its first painted \
         row is the beginning of the log on the strength of the PREVIOUS room's \
         read",
    );
    assert!(
        rooms.contains("None=>me.older_settled.set(true),"),
        "a first page shorter than the window provably held the whole log, and \
         nothing else will ever answer for that room — no walk starts. Without \
         this arm every short room renders the hydrating silence permanently, \
         which is the exact bug on the rooms where it is most obviously wrong",
    );

    // `park_older_cursor` is the deliberate non-settler and must stay one: a
    // request that never answered is not evidence about the shape of the log.
    let park = rooms
        .split_once("fnpark_older_cursor(&self,generation_id:u64,key:&str,cursor:Option<u64>){")
        .and_then(|(_, rest)| rest.split_once("}}"))
        .map(|(body, _)| body)
        .expect("`park_older_cursor` has a body");
    assert!(
        !park.contains("older_settled"),
        "a dropped request parks the page it was reading and settles NOTHING. \
         Settling there would let a flaky network tell an operator they have \
         reached the beginning of a room they have not finished reading",
    );
}

/// The affordance's three arms, at the call site. The pure decider has unit
/// tests; what they cannot say is that the view still asks it, or that all
/// three answers reach a person.
#[test]
fn the_older_edge_renders_all_three_of_its_states() {
    let workspace = without_whitespace(&view_source("rooms_workspace.rs"));

    assert!(
        workspace.contains("{move||matchrooms.older_history(){OlderHistory::Available=>view!{"),
        "the affordance must ask the tri-state decider, reactively — hydration \
         publishes the answer several page-loads after the first paint, so a \
         view reading it untracked asked before the answer existed",
    );
    assert!(
        workspace.contains(
            "OlderHistory::ReachedBeginning=>view!{<pclass=\"rooms-workspace__transcript-start\"role=\"status\">\"Beginningoftheroom\"</p>}.into_any(),"
        ),
        "a room whose walk provably reached the start of its log must SAY so. \
         Rendering nothing is what it did before this slice, and nothing is \
         also what a room still hydrating renders — so the one fact worth \
         having was the one indistinguishable from not having asked",
    );
    assert!(
        workspace.contains("OlderHistory::Unknown=>().into_any(),"),
        "and the unasked state must stay silent: claiming a beginning the \
         surface has not read is worse than saying nothing at all",
    );

    // Position: the notice marks the same edge the button does, so it sits
    // where the button sits — inside the scroll container, above the rows.
    let transcript_open = workspace
        .find("class=\"rooms-workspace__transcript\"")
        .expect("the transcript scroll container");
    let notice = workspace
        .find("class=\"rooms-workspace__transcript-start\"")
        .expect("the beginning-of-room notice");
    let rows = workspace
        .find("each=move||{letroots=main_transcript_rows(&rooms.transcript.get());")
        .expect("the `<For>` over the transcript rows");
    assert!(
        transcript_open < notice && notice < rows,
        "the notice names a position in the log — the top of it — so it has to \
         sit at that position and scroll with it, exactly where the button it \
         replaces sat",
    );
}

/// The orphaned reply. Both call sites, because they must agree: the list the
/// `<For>` paints and the list the empty state counts are the same list, or a
/// room holding only orphans paints rows under "No messages yet".
#[test]
fn an_orphaned_reply_reaches_the_main_list_and_says_why() {
    let workspace = without_whitespace(&view_source("rooms_workspace.rs"));

    assert_eq!(
        workspace
            .matches("main_transcript_rows(&rooms.transcript.get())")
            .count(),
        2,
        "both the `<For>` and the empty-state check read the SAME list. The \
         list this replaced keeps only rows with no `thread_parent_seq`, so a \
         room hydrated into the middle of a long thread holds orphaned replies, \
         no roots at all, and would paint them under `No messages yet`",
    );
    assert!(
        !workspace.contains("partition_thread_messages(&rooms.transcript.get(),0)"),
        "nothing in the view may still build the main list from the roots-only \
         partition — that function is the THREAD pane's, where a named root is \
         the whole point",
    );

    assert!(
        workspace.contains("reply_is_orphaned(&transcript,&orphan_row).then("),
        "the row must ask whether it is an orphan, reactively: the press that \
         brings the root in makes this an ordinary reply again, at which point \
         the row leaves the list entirely",
    );
    assert!(
        workspace.contains(
            "<pclass=\"rooms-workspace__msg-orphan\"role=\"note\">{orphaned_reply_note(rooms.older_history())}</p>"
        ),
        "and it must SAY it is one. A reply rendered in the main column with no \
         note is a lie about what it is — it reads as a top-level message, and \
         the thread it belongs to is invisible rather than merely unloaded",
    );
    assert!(
        workspace.contains("orphaned_reply_note(rooms.older_history())"),
        "the note's wording turns on whether the root is still reachable: while \
         older history remains it points at the press, and once the log's start \
         has been read it must stop promising a button that cannot help",
    );
}

/// The stylesheet half. Both new elements are text-only, so an unstyled one is
/// not a visible break — it is a paragraph in body copy where a quiet marker
/// belongs, which is precisely the kind of regression nothing else here catches.
#[test]
fn the_older_edge_has_the_rules_it_renders_against() {
    let css = read("styles/rooms-workspace.css");

    for selector in [
        ".rooms-workspace__transcript-start {",
        ".rooms-workspace__msg-orphan {",
    ] {
        assert!(
            css.contains(selector),
            "`{selector}` is emitted by the transcript and must exist in \
             styles/rooms-workspace.css",
        );
    }
}
