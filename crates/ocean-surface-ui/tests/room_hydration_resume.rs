//! The open room's ONE resume point, pinned where the compiler cannot hold it.
//!
//! Opening a room reads `/snapshot` and seeds `Rooms::resume_seq` with the
//! cursor that body names (`last_seq`) rather than anything re-deriving one from
//! the rows it just painted. Both readers of that signal are pinned here: the
//! live tail's first connection and its reconnects, and the catch-up read the
//! four roster/message mutations fire. That wiring is expressions spread over
//! 500 lines and every one of them is silent: the review of the change that
//! introduced it replaced `RwSignal::new(resume_seq)` with
//! `RwSignal::new(last_transcript_seq(&self.transcript.get_untracked()))` — the
//! exact behavior the change exists to stop — and all 1237 tests passed,
//! including the unit test named after the thesis. A pure helper tested in
//! isolation proves the RULE; nothing proved the rule was wired to anything.
//! The same gap then survived the fix: `refresh_open_transcript` went on
//! re-deriving its own cursor from the painted rows for another wave, one line
//! below a doc comment declaring the opposite, and every gate stayed green.
//!
//! The backward hydration walk is pinned here too, for the same reason and by
//! the same lever: it is wiring in the same function, its rules live in pure
//! helpers that unit tests already own, and the one thing no helper can say is
//! whether `open_room` still calls them — or still calls them on every open.
//!
//! `EventSource` and `spawn_local` are browser-only and `Rooms::new` takes a
//! live `Daemon`, so no test in this crate can start the tail and read the URL
//! it opens. Scanning the source is the lever that is left — the one
//! `unheld_room_controls.rs` documents for controls the compiler does not hold —
//! and the two rules that file paid for hold here: name the CALL SITE, and scan
//! `common::view_source`, which truncates at `#[cfg(test)]` so the module's own
//! unit tests cannot satisfy a needle by quoting it.
//!
//! ## Measured, not assumed
//!
//! Twelve mutations run for real against this tree, each with the gate actually
//! executed:
//!
//! | mutation                                                            | result |
//! |---------------------------------------------------------------------|--------|
//! | tail seed → `RwSignal::new(last_transcript_seq(&self.transcript…))`  | RED — both guards below |
//! | tail seed's parameter renamed `resume_seq` → `hydrated_seq` (no-op)  | RED — the chain is literal |
//! | `open_room`'s `snapshot_resume_seq(..)` → `last_transcript_seq(..)`  | RED — first needle |
//! | `room_snapshot_url` call → the old unpaged `format!` room GET        | RED here, and `dead_code` on the wasm lane |
//! | `HYDRATION_TRANSCRIPT_LIMIT` 1000 → 200 (`/snapshot`'s own default)  | RED in `rooms.rs`, not here — the URL literal is asserted there |
//! | one catch-up call site's cursor → `last_transcript_seq(&me.transcript…)` | RED — the prohibition, and the counted call sites at 3 against 4 |
//! | the catch-up walk's `page.next_seq` argument → `None`                | RED here; `rooms.rs`'s unit tests stay green, they own the rule and never its wiring |
//! | the tail's `advanced_resume_seq(*seq, entry.seq)` → `Some(entry.seq)` | RED — the tail must advance the shared resume like everything else |
//! | `me.backfill_open_transcript(..)` moved inside `if !closed`         | RED — and green on all seven gates without this file |
//! | the backward walk's `page.prev_seq` argument → `None`               | RED here; otherwise only `dead_code` on the field catches it |
//! | that same call wrapped in a `if !closed` of its OWN                 | RED — the count below is what makes "outside" mean outside |
//! | the walk's seed window `HYDRATION_TRANSCRIPT_LIMIT` → `200`         | RED — a walk seeded against a window hydration did not ask for |
//!
//! The rename row is the cost of a literal chain and is deliberate: a local can
//! be renamed freely as long as the needle moves with it, and the assert
//! messages say what has to stay true.

mod common;

use common::{view_source, without_whitespace};

/// The four expressions that carry the daemon's cursor from the hydration
/// response into the tail's first connection. Mutations run: the seed replaced
/// with `RwSignal::new(last_transcript_seq(&self.transcript.get_untracked()))`
/// (the reviewer's), and `resume_seq` at the `start_live_tail` call site
/// replaced with `last_transcript_seq(&transcript)`. Both leave the rest of the
/// gate green.
#[test]
fn the_snapshot_cursor_reaches_the_tails_first_connection() {
    let rooms = without_whitespace(&view_source("rooms.rs"));

    assert!(
        rooms.contains("letresume_seq=snapshot_resume_seq(last_seq,&transcript);"),
        "`open_room` must resolve the resume point from the snapshot body's own \
         `last_seq` — `snapshot_resume_seq` already falls back to the painted \
         rows for a daemon that predates the field, so a caller re-deriving one \
         is a caller throwing the daemon's answer away",
    );
    assert!(
        rooms.contains("me.start_live_tail(key,generation_id,resume_seq);"),
        "the hydration's cursor must be what `open_room` hands the tail; a tail \
         started without it is a tail that has to guess",
    );
    assert!(
        rooms.contains("self.resume_seq.set(resume_seq);"),
        "`start_live_tail` must seed the room's resume signal from the resume \
         point it was handed and from nothing else",
    );
    assert!(
        rooms.contains("letmutresume_seq=me.resume_seq.get_untracked();")
            && rooms.contains("leturl=url_with_after_seq(&events_url,resume_seq);"),
        "the seeded cursor must be what the first SSE connection resumes at — \
         `?after_seq=` is the whole reason hydration moved onto `/snapshot`",
    );
    assert!(
        rooms.contains("me.resume_seq.update(|seq|*seq=advanced_resume_seq(*seq,entry.seq));"),
        "the tail must advance the ROOM's resume point as it ingests, not a \
         signal of its own: a private cursor is how the catch-up read came to \
         hold a second, contradictory answer to the same question",
    );
}

/// The catch-up read is handed the same signal, for the same reason. Mutations
/// run: an `after_seq` argument at a call site replaced with
/// `last_transcript_seq(&me.transcript.get_untracked())` (the shape this slice
/// removed), and the walk's `page.next_seq` argument replaced with `None`, which
/// silently demotes the daemon's cursor to the page's last row. Both leave
/// clippy, both `cargo check`s and every unit test in `rooms.rs` green — the
/// unit tests own the cursor RULE, never its wiring.
#[test]
fn the_catchup_read_is_handed_the_rooms_resume_point_and_pages_on_the_daemons() {
    let rooms = without_whitespace(&view_source("rooms.rs"));

    assert!(
        rooms.contains(
            "fnrefresh_open_transcript(&self,key:&str,generation_id:u64,after_seq:Option<u64>){"
        ),
        "the catch-up read must take its start from the caller; deriving one \
         inside is what left the module holding two answers",
    );
    // COUNTED, not merely present: `contains` stays green while three of the
    // four sites hand over the resume and the fourth re-derives one, which is
    // the shape of the bug this slice removed. A fifth caller reds this on
    // purpose — the number is here to make its author say which cursor it hands
    // over, and 4 is join, leave, remove-participant and post-message.
    assert_eq!(
        rooms
            .matches("me.refresh_open_transcript(&key,generation_id,me.resume_seq.get_untracked())")
            .count(),
        4,
        "every catch-up call site must hand over the room's resume point — the \
         same signal the tail seeds and advances. If you have just ADDED a \
         caller, this count is the ask, not the failure: check which cursor \
         your call hands over, then bump the number",
    );
    assert!(
        rooms.contains("transcript_catchup_cursor(pages_read,page.has_more,page.next_seq,covered)"),
        "the walk must continue on the cursor the PAGE named: `/transcript` \
         serves at most 200 rows, so one request keeps the first page of a \
         burst and drops the rest in silence",
    );
}

/// The mutation itself, stated as a prohibition over the whole module.
/// `last_transcript_seq` is still the fallback INSIDE `snapshot_resume_seq`,
/// where the daemon has declined to answer, and it still reads a PAGE the daemon
/// just served in the catch-up walk. What it may never read is the transcript
/// signal: those rows are one page of a log (the store caps a page at 1000 rows)
/// and a resume taken from them is the daemon's answer thrown away.
#[test]
fn no_resume_is_ever_re_derived_from_the_painted_rows() {
    let rooms = without_whitespace(&view_source("rooms.rs"));

    assert!(
        !rooms.contains("RwSignal::new(last_transcript_seq("),
        "the tail's cursor must come from the hydration response, not from the \
         transcript signal",
    );
    assert!(
        !rooms.contains("last_transcript_seq(&me.transcript")
            && !rooms.contains("last_transcript_seq(&self.transcript"),
        "no resume may be re-derived from the transcript signal — the catch-up \
         read did exactly this, one line below a doc comment stating the \
         opposite rule, and nothing in the gate said so",
    );
}

/// Hydration reads the route that carries a cursor, at the size the unpaged
/// read used to paint. Mutation run: `?limit=1000` → `?limit=200`, which is
/// `/snapshot`'s own default and shrinks the first paint to a fifth with no
/// warning anywhere. The unit test in `rooms.rs` pins the URL the helper builds;
/// this pins that `open_room` is still the thing calling it.
#[test]
fn open_room_hydrates_through_the_snapshot_helper() {
    let rooms = without_whitespace(&view_source("rooms.rs"));

    assert!(
        rooms.contains("letget_url=room_snapshot_url(&base,&key);"),
        "`open_room` must hydrate through `room_snapshot_url`, the one place the \
         route and its explicit `limit` are asserted (`rooms.rs`: \
         `hydration_reads_snapshot_at_the_stores_full_page`)",
    );
}

/// The BACKWARD walk's wiring, and one rule the forward walk does not need: it
/// runs on EVERY open, outside the gate that decides whether a tail starts.
///
/// Mutation run: `me.backfill_open_transcript(..)` moved inside `if !closed`.
/// That un-fixes the one case this slice calls genuinely unreachable — a
/// soft-closed room past the window opens no `EventSource`, so the walk is the
/// only thing that can ever bring it a row it did not hydrate with — and it
/// passed `fmt`, both clippy lanes and every one of the 1301 tests standing in
/// the tree before this test existed, because each unit test covering this walk
/// is a pure-helper test. Second mutation: the walk's `page.prev_seq` argument
/// dropped. That one is caught today, but only by `dead_code` on the struct
/// field, and the first slice to add a second reader of `prev_seq` deletes that
/// catch without touching this walk.
#[test]
fn the_backward_walk_runs_on_every_open_and_pages_on_the_daemons_cursor() {
    let rooms = without_whitespace(&view_source("rooms.rs"));

    assert!(
        rooms.contains(
            "letbackfill_from=hydration_backfill_start(&transcript,HYDRATION_TRANSCRIPT_LIMIT);"
        ),
        "the walk's start must be measured against the window hydration \
         actually asked for; measuring a 1000-row page against any other number \
         either strands rows behind it or spends a request per open to be told \
         there is nothing there",
    );
    assert!(
        rooms.contains(
            "ifletSome(before_seq)=backfill_from{me.backfill_open_transcript(&key,generation_id,before_seq);}"
        ),
        "`open_room` must start the backward walk from the page it just \
         painted — anchoring the first paint at the tail is what makes every \
         row before it unreachable by any other read in this module",
    );
    // The two needles below are what make the one above mean OUTSIDE the
    // closedness gate rather than merely present. `contains` alone stays green
    // with the call moved inside the gate, which is the mutation.
    assert_eq!(
        rooms.matches("if!closed").count(),
        1,
        "the snapshot's `closed` may gate ONE thing, the live tail. A second \
         gate is how the backward walk silently stops running for the room that \
         needs it most: the soft-closed one, which has no tail to bring it \
         anything else. If you have just added a legitimate second gate, say \
         here which of the two it is",
    );
    assert!(
        rooms.contains("if!closed{me.start_live_tail(key,generation_id,resume_seq);}"),
        "that one gate holds the tail and nothing else — anything else moved \
         inside it stops running on a soft-closed room, which is a frozen audit \
         view that still has to paint",
    );
    assert!(
        rooms.contains(
            "transcript_backfill_cursor(pages_read,page.has_more,page.prev_seq,reached_back_to"
        ),
        "the walk must continue on the cursor the PAGE named: `prev_seq` is the \
         daemon's own `before_seq` for the next page, and the mirror of the \
         forward walk's `next_seq`",
    );
}
