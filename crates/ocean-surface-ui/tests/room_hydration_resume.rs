//! The hydration → live-tail handoff in `rooms.rs`, pinned where the compiler
//! cannot hold it.
//!
//! Opening a room reads `/snapshot` and hands the tail the cursor that body
//! names (`last_seq`) rather than the tail re-deriving one from the rows it just
//! painted. That handoff is four expressions spread over 500 lines and every one
//! of them is silent: the review of the change that introduced it replaced
//! `RwSignal::new(resume_seq)` with
//! `RwSignal::new(last_transcript_seq(&self.transcript.get_untracked()))` — the
//! exact behavior the change exists to stop — and all 1237 tests passed,
//! including the unit test named after the thesis. A pure helper tested in
//! isolation proves the RULE; nothing proved the rule was wired to anything.
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
//! Five mutations run for real against this tree, each with the gate actually
//! executed:
//!
//! | mutation                                                            | result |
//! |---------------------------------------------------------------------|--------|
//! | tail seed → `RwSignal::new(last_transcript_seq(&self.transcript…))`  | RED — both guards below |
//! | tail seed's parameter renamed `resume_seq` → `hydrated_seq` (no-op)  | RED — the chain is literal |
//! | `open_room`'s `snapshot_resume_seq(..)` → `last_transcript_seq(..)`  | RED — first needle |
//! | `room_snapshot_url` call → the old unpaged `format!` room GET        | RED here, and `dead_code` on the wasm lane |
//! | `HYDRATION_TRANSCRIPT_LIMIT` 1000 → 200 (`/snapshot`'s own default)  | RED in `rooms.rs`, not here — the URL literal is asserted there |
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
        rooms.contains("letlast_seq=RwSignal::new(resume_seq);"),
        "`start_live_tail` must seed its cursor signal from the resume point it \
         was handed and from nothing else",
    );
    assert!(
        rooms.contains("letmutresume_seq=last_seq.get_untracked();")
            && rooms.contains("leturl=url_with_after_seq(&events_url,resume_seq);"),
        "the seeded cursor must be what the first SSE connection resumes at — \
         `?after_seq=` is the whole reason hydration moved onto `/snapshot`",
    );
}

/// The mutation itself, stated as a prohibition. `last_transcript_seq` is still
/// the fallback INSIDE `snapshot_resume_seq`, where the daemon has declined to
/// answer; anywhere it seeds a signal, the daemon's answer is being ignored.
#[test]
fn the_tail_never_re_derives_its_cursor_from_the_painted_rows() {
    let rooms = without_whitespace(&view_source("rooms.rs"));

    assert!(
        !rooms.contains("RwSignal::new(last_transcript_seq("),
        "the tail's cursor must come from the hydration response, not from the \
         transcript signal: the painted rows are ONE PAGE (the store caps a page \
         at 1000 rows) and the daemon's `last_seq` is that page's own end",
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
