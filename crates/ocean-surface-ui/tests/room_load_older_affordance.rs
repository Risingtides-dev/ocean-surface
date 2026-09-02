//! A transcript with an older half must offer a way to ask for it.
//!
//! ocean-surface#190 anchored hydration at a room's NEWEST page and walked
//! backward from there, bounded at 1000 + 5×200 rows. Its own author wrote the
//! consequence into `rooms.rs`: past that budget the older half is off screen
//! and nothing says so, because the last page's `prev_seq` and `has_more` were
//! dropped at the instant `backfill_open_transcript` returned. The oldest row
//! painted read as the first message in the room.
//!
//! This file pins the consumer half — the parked cursor, the one-page read that
//! replays it, and the control that fires it — for the same reason
//! `room_hydration_resume.rs` pins the producer half: every rule involved lives
//! in a pure helper that unit tests already own, and the one thing no helper can
//! say is whether anything still CALLS it. `crates/ocean-surface-ui` is a binary
//! crate (`src/main.rs`, no `[lib]`), so a test here cannot mount the workspace
//! or press the button; scanning the source is the lever that is left, and the
//! two rules `unheld_room_controls.rs` paid for hold here too — name the CALL
//! SITE, and scan `common::view_source`, which truncates at `#[cfg(test)]` so a
//! module's own unit tests cannot satisfy a needle by quoting it.
//!
//! ## Measured, not assumed
//!
//! Twelve mutations applied for real against this tree, each with this file run
//! against the mutated source. Where a row claims something about the REST of
//! the gate, that lane was executed too:
//!
//! | mutation                                                              | result |
//! |-----------------------------------------------------------------------|--------|
//! | the walk's stop parks `None` instead of `transcript_older_cursor(..)`  | RED — and all 1251 unit tests stayed green (run) |
//! | that park given `false` for `has_more` (always "nothing older")        | RED — the literal chain |
//! | `load_older_transcript_page` drops its `room_is_current` re-check      | RED — otherwise a switched room gets the previous room's rows |
//! | its in-flight guard removed                                            | RED — a double press spends two requests off one cursor |
//! | `reset_room_state` stops clearing `older_cursor`                       | RED — the next room inherits this one's position in the log |
//! | `reset_room_state` stops clearing `older_in_flight`                    | RED — a press mid-switch leaves the button disabled forever |
//! | the button's `disabled=` dropped                                       | RED — nothing else stops the second press |
//! | `AnchorOlder` arm's body replaced with `{}`                            | RED — the prepend then jumps the viewport by a page |
//! | `grew_at_front` argument replaced with `false`                         | RED — and `cargo test` stayed green at 1251 passed (run): the arm's unit tests own the rule and never its wiring |
//! | `anchor.is_some()` argument replaced with `true`                       | RED — and `cargo test` stayed green (run): the walk's prepends take the anchor arm with no anchor, which is #190 again |
//! | the `<For>` key reverted to `m.seq`                                    | RED — and every other lane green (run): nothing else in the crate reads that key |
//! | the button's `on:click` deleted                                        | RED — but so is the wasm lane, see below |
//!
//! That last row is the one that came back other than expected, and it is worth
//! writing down rather than quietly dropping. Deleting the `on:click` was
//! assumed to leave every other lane green — a rendered control wired to
//! nothing. It does not: `load_older_transcript_page` loses its only caller and
//! `cargo clippy --target wasm32-unknown-unknown -- -D warnings` fails with
//! "method `load_older_transcript_page` is never used". So the press needle
//! below is belt-and-braces, not this file's own catch, and it is kept for the
//! message rather than the coverage.

mod common;

use common::{read, view_source, without_whitespace};

/// What the hydration walk stops holding is the whole feature. Mutation run:
/// the park replaced with `me.older_cursor.set(None)`, which compiles, keeps
/// both clippy lanes and all 1251 unit tests green, and ships a room whose
/// older half is exactly as unreachable as before.
#[test]
fn the_walk_parks_the_cursor_it_used_to_drop() {
    let rooms = without_whitespace(&view_source("rooms.rs"));
    // Scoped: the on-demand read parks through the same call, so a module-wide
    // `contains` stays green with the WALK's park deleted — and the walk's is
    // the one that decides whether the affordance ever appears.
    let walk = fn_body(
        &rooms,
        "fnbackfill_open_transcript(&self,key:&str,generation_id:u64,before_seq:u64){",
    );

    assert!(
        walk.contains(
            "me.older_cursor.set(transcript_older_cursor(page.has_more,page.prev_seq,reached_back_to,));"
        ),
        "where the backward walk stops it must park the page's own cursor — \
         `has_more` and `prev_seq` are dropped at that instant otherwise, and \
         they are the only route left to the rows behind the paint",
    );
    assert!(
        walk.contains("me.park_older_cursor(generation_id,&key,Some(cursor));"),
        "a request that never answered leaves its page exactly where it was; \
         parking the cursor it was reading turns a flaky network into a \
         repeatable press instead of history that silently ends",
    );
    assert!(
        rooms.contains("fnpark_older_cursor(&self,generation_id:u64,key:&str,cursor:Option<u64>){")
            && rooms.contains(
                "ifself.room_is_current(generation_id,key){self.older_cursor.set(cursor);}"
            ),
        "that park lands after an await like everything else here, so it must \
         re-check the room — writing a retired room's cursor offers the \
         operator history belonging to a room they have left",
    );
}

/// The on-demand read is the walk's own request with one stop condition
/// removed, and the guards it must keep are the ones the walk keeps. Mutations
/// run: the `room_is_current` re-check deleted, and the in-flight guard deleted.
#[test]
fn the_on_demand_page_is_the_walks_request_with_the_walks_guards() {
    let rooms = without_whitespace(&view_source("rooms.rs"));

    assert!(
        rooms.contains("pub(crate)fnload_older_transcript_page(&self){"),
        "the workspace needs a `pub(crate)` door to one older page; a private \
         one leaves the parked cursor unreachable from the view that renders \
         the affordance",
    );
    // Scoped to this function's own body, not the module. Every needle below
    // also occurs in `backfill_open_transcript` — same route, same merge, same
    // re-check — so a module-wide `contains` would go on passing with the whole
    // on-demand read deleted, which is the opposite of a guard.
    let body = fn_body(&rooms, "pub(crate)fnload_older_transcript_page(&self){");

    assert!(
        body.contains("ifself.older_in_flight.get_untracked(){return;}"),
        "a second press must not fire a second request against a cursor the \
         first has not moved yet",
    );
    assert!(
        body.contains(
            "leturl=room_snapshot_tail_url(&base,&key,cursor,BACKFILL_TRANSCRIPT_PAGE_LIMIT);"
        ),
        "one page, through the same builder and at the same size as the \
         hydration walk — `/transcript` is forward-only and cannot serve a row \
         older than the paint at all",
    );
    assert!(
        body.contains("if!me.room_is_current(generation_id,&key){return;}"),
        "the response lands after an await, and a room switched during one must \
         not have its rows prepended under the room the operator switched to — \
         nor its in-flight flag lowered, which would lower the NEXT room's",
    );
    assert!(
        body.contains(
            "me.transcript.update(|transcript|prepend_transcript_page(transcript,page.transcript));"
        ),
        "the page merges through `prepend_transcript_page`, which is strict \
         about rows already painted and keeps the vector ascending by `seq`",
    );
    assert!(
        body.contains(
            "me.older_cursor.set(transcript_older_cursor(page.has_more,page.prev_seq,reached_back_to,));"
        ),
        "each press must leave the cursor the page it just read named, so the \
         next press continues rather than re-serving the same rows",
    );
    assert!(
        body.contains("me.older_in_flight.set(false);"),
        "the flag has to come down on the success path too, or the affordance \
         disables itself after one press",
    );
}

/// A cursor is one room's position in one room's log. Mutations run: each
/// `reset_room_state` clear deleted in turn. The `older_cursor` one offers the
/// next room a press against the previous room's history; the `older_in_flight`
/// one is worse — a press outstanding across a switch returns to a
/// `room_is_current` check that refuses to write, so nothing else will ever
/// lower the flag and the button stays disabled for the life of the session.
#[test]
fn a_room_switch_clears_both_halves_of_the_older_state() {
    let rooms = without_whitespace(&view_source("rooms.rs"));

    let reset = fn_body(&rooms, "fnreset_room_state(&self){");
    assert!(
        reset.contains("self.older_cursor.set(None);"),
        "the parked cursor belongs to the room that parked it",
    );
    assert!(
        reset.contains("self.older_in_flight.set(false);"),
        "an in-flight press outlives the room it was made in and its completion \
         refuses to write — this clear is the only thing that lowers the flag",
    );
}

/// The control. Mutation run: the `on:click` deleted, which leaves every gate
/// green — `load_older_transcript_page` stays reachable from nothing and the
/// button renders, disabled by nothing, doing nothing.
#[test]
fn the_affordance_renders_at_the_top_and_fires_the_read() {
    let workspace = without_whitespace(&view_source("rooms_workspace.rs"));

    assert!(
        workspace.contains("{move||rooms.older_transcript_available().then(||view!{"),
        "the affordance renders on the parked cursor and on nothing else — a \
         room whose walk reached the start of the log parks `None` and must \
         grow no control at all",
    );
    assert!(
        workspace.contains("class=\"rooms-workspace__load-older\""),
        "the class the stylesheet dresses",
    );
    assert!(
        workspace.contains("rooms.load_older_transcript_page();"),
        "the button must fire the read; a control that renders the state \
         without acting on it is the same off-screen history with extra steps",
    );
    assert!(
        workspace.contains("disabled=move||rooms.older_transcript_in_flight()"),
        "and it must refuse the second press while the first is in flight",
    );

    // Position, not merely presence. The affordance marks where the loaded
    // transcript starts, so it belongs INSIDE the scroll container and above
    // the rows — `contains` alone stays green with it rendered under the
    // composer.
    let transcript_open = workspace
        .find("class=\"rooms-workspace__transcript\"")
        .expect("the transcript scroll container");
    let affordance = workspace
        .find("class=\"rooms-workspace__load-older\"")
        .expect("the load-older affordance");
    let rows = workspace
        .find("each=move||{letroots=partition_thread_messages(&rooms.transcript.get(),0).roots;")
        .expect("the `<For>` over the transcript roots");
    assert!(
        transcript_open < affordance && affordance < rows,
        "the affordance sits inside `.rooms-workspace__transcript` and ahead of \
         the rows: it names a position in the log, so it has to scroll with the \
         log — unlike `.rooms-workspace__jump-new`, which is a viewport-fixed \
         control and lives outside the container",
    );
}

/// Scroll anchoring, which is the half a reviewer cannot see in a screenshot.
///
/// Mutations run: the `AnchorOlder` arm's body emptied, and the `grew_at_front`
/// argument replaced with `false`. The second is the one worth naming — it
/// leaves `cargo test` fully green, because `transcript_pass_action`'s unit
/// tests own the rule and never its wiring, and it silently restores the
/// behaviour this slice removed: a prepend read as an append, raising
/// "New messages" over rows that arrived ABOVE the reader.
#[test]
fn a_prepended_page_anchors_the_scroll_instead_of_reading_as_an_append() {
    let workspace = without_whitespace(&view_source("rooms_workspace.rs"));

    assert!(
        workspace.contains(
            "matchtranscript_pass_action(len,prev_len,el.is_some(),near_bottom,grew_at_front,anchor.is_some(),){"
        ),
        "the pass must be told whether its growth arrived at the front — without \
         it a prepend is indistinguishable from a tail append and takes the \
         jump-affordance arm — and whether a press ASKED for that growth, \
         without which the hydration walk's own prepends take the anchor arm \
         and land unanchored, which is #190 again",
    );
    assert!(
        workspace.contains("letanchor=older_anchor.get_untracked();"),
        "read untracked: two arms below clear this signal, and an Effect that \
         tracked what it writes would re-enter itself",
    );
    assert!(
        workspace
            .contains("letgrew_at_front=transcript_grew_at_front(previous.oldest_seq,oldest_seq);"),
        "and that answer must come from the oldest `seq` across passes — the \
         row count cannot tell which end grew",
    );
    assert!(
        workspace
            .contains("let(len,oldest_seq)=transcript.with(|t|(t.len(),t.first().map(|m|m.seq)));"),
        "both halves of the pass state are read from the same borrow of the \
         same transcript, so they can never describe different vectors",
    );

    // The anchor is captured by the PRESS, before the rows land. This Effect
    // cannot know whether it runs before or after the `<For>` writes them, so
    // measuring inside the arm would read whichever it happened to be.
    assert!(
        workspace.contains("older_anchor.set(Some((el.scroll_height(),el.scroll_top(),)));"),
        "the press must record the scroll geometry it is about to invalidate",
    );
    assert!(
        workspace.contains("letgrown=el.scroll_height()-anchored_height;")
            && workspace.contains("el.set_scroll_top(anchored_top+grown);"),
        "and the restore must move the scroll by exactly what arrived above it \
         — rows prepended to a scrolled container push the reader's place down \
         by their own height, and a page is roughly a screenful of them",
    );
    assert!(
        workspace.contains("request_animation_frame(move||{letgrown=el.scroll_height()"),
        "measured in a frame callback, which is the first point that can see \
         the grown element; the same reason the at-bottom pin next to it uses \
         one",
    );
}

/// The seam the press creates, which is the half a unit test cannot see at all.
///
/// `<For>` caches a child per key. Keyed on `m.seq` alone, the row that WAS the
/// oldest keeps the view it was built with — built when its predecessor was
/// `None`, and `day_separator_label(None, cur)` answers `Some` unconditionally.
/// So a same-day row arriving directly above it leaves a stray day divider
/// between them and the seam row still avatar-headed and ungrouped. One of each
/// per press, exactly where the reader is looking.
///
/// Mutation run: the key reverted to `m.seq`. Every other lane stays green —
/// nothing but this needle reads the key. The comment beside it, which used to
/// assert the append-only invariant this slice exists to break, is corrected in
/// the source and deliberately NOT pinned here: a needle spanning a wrapped
/// comment breaks on rustfmt rather than on meaning, which is the guard that
/// teaches the next author to delete it.
#[test]
fn the_row_a_prepend_gives_a_predecessor_is_rebuilt_not_reused() {
    let workspace = without_whitespace(&view_source("rooms_workspace.rs"));

    assert!(
        workspace.contains(
            "key=|(prev,m):&(Option<RoomMessage>,RoomMessage)|{(prev.as_ref().map(|p|p.seq),m.seq)}"
        ),
        "the predecessor is half the identity of a row whose density is derived \
         from it, so it has to be half the key — the transcript grows at BOTH \
         ends now, and a child cached under `seq` alone keeps a divider and a \
         header that the page above it has just made wrong",
    );
}

/// The stylesheet. Asserted by extracted rule body rather than byte-exact, the
/// lesson `dead_trigger_row_affordance.rs` carries in its own first test: a
/// pinned block fires on unrelated design work and teaches the next author to
/// delete the guard.
#[test]
fn the_affordance_is_a_full_width_row_not_a_floating_pill() {
    let normalized =
        css_without_whitespace(&strip_css_comments(&read("styles/rooms-workspace.css")));

    let body = rule_body(&normalized, ".rooms-workspace__load-older");
    assert!(
        body.contains("width:100%;"),
        "it marks the top edge of the loaded log, so it spans the rail rather \
         than floating over it, got `{body}`",
    );
    assert!(
        !body.contains("position:absolute") && !body.contains("position:fixed"),
        "taking it out of flow would leave it pinned to the viewport while the \
         position it names scrolls away, got `{body}`",
    );
    assert!(
        body.contains("cursor:pointer;"),
        "it is a real control and has to read as one, got `{body}`",
    );
    assert!(
        normalized.contains(".rooms-workspace__load-older:focus-visible{"),
        "a keyboard reader needs the same focus ring every other control in \
         this rail carries",
    );
    let disabled = rule_body(&normalized, ".rooms-workspace__load-older:disabled");
    assert!(
        !disabled.contains("pointer-events:none"),
        "the in-flight state is a fade and a cursor, never a removed hit area — \
         `pointer-events: none` suppresses the disabled cursor along with the \
         click, got `{disabled}`",
    );
}

// ---- Scanning helpers -------------------------------------------------------

/// The brace-matched body of the function whose whitespace-stripped signature is
/// `signature`, within whitespace-stripped Rust source.
///
/// Needed because this slice's two readers of the parked cursor are deliberately
/// the same three calls — same route builder, same merge, same `room_is_current`
/// re-check — so a needle asserted over the whole module is satisfied by either
/// one and pins neither. Depth counting assumes the function body holds no brace
/// inside a string or char literal; neither of the two scanned here does, and a
/// `format!` added to one would need this helper taught about quoting.
fn fn_body(stripped: &str, signature: &str) -> String {
    let at = stripped
        .find(signature)
        .unwrap_or_else(|| panic!("no function matching `{signature}`"));
    let open = at + signature.len() - 1;
    let bytes = stripped.as_bytes();
    let mut depth = 0usize;
    for i in open..bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return stripped[open + 1..i].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("unterminated body for `{signature}`")
}

// ---- CSS scanning helpers (the shape `dead_trigger_row_affordance.rs` uses) --

fn strip_css_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(open) = rest.find("/*") {
        out.push_str(&rest[..open]);
        if let Some(close) = rest[open + 2..].find("*/") {
            rest = &rest[open + 2 + close + 2..];
        } else {
            break;
        }
    }
    out.push_str(rest);
    out
}

fn css_without_whitespace(css: &str) -> String {
    css.chars().filter(|char| !char.is_whitespace()).collect()
}

/// Declarations of the first rule whose normalized prelude ends with `selector`.
/// Braces are single-byte ASCII, so byte scanning is safe over multi-byte text.
fn rule_body(normalized: &str, selector: &str) -> String {
    let needle = format!("{selector}{{");
    let at = normalized
        .find(&needle)
        .unwrap_or_else(|| panic!("no rule for `{selector}`"));
    let open = at + needle.len();
    let end = normalized[open..]
        .find('}')
        .unwrap_or_else(|| panic!("unterminated rule for `{selector}`"));
    normalized[open..open + end].to_string()
}
