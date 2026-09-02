//! A rail that lists a bounded page of rooms must offer the rest of them.
//!
//! `GET /v1/rooms/persistent` has been paged since OCEAN-250: `?limit=&cursor=`
//! in, `next_cursor`/`has_more` out, and a store-side default of 100 rooms per
//! page (`ocean_store::DEFAULT_LIST_LIMIT`). The surface's `RoomsListResponse`
//! decoded `ok`, `rooms`, `read_states` and `error` and nothing else, so the
//! rail stopped at the daemon's page size, a member with more rooms could not
//! reach them, and — the half that makes it a product bug rather than a
//! shortfall — nothing on screen said there were any.
//!
//! This file pins the consumer half the way `room_load_older_affordance.rs`
//! pins the transcript's: every rule involved lives in a pure helper that unit
//! tests already own, and the one thing no helper can say is whether anything
//! still CALLS it, or whether the control that fires it is still where a member
//! looking for the end of the list would find it. `crates/ocean-surface-ui` is
//! a binary crate (`src/main.rs`, no `[lib]`), so a test here cannot mount the
//! workspace or press the button; scanning the source is the lever that is
//! left, and both rules that directory paid for hold here too — name the CALL
//! SITE, and scan `common::view_source`, which truncates at `#[cfg(test)]` so a
//! module's own unit tests cannot satisfy a needle by quoting it.
//!
//! ## Measured, not assumed
//!
//! Seven mutations applied for real against this tree. Each was run three ways:
//! this file, `cargo clippy -p ocean-surface-ui --target wasm32-unknown-unknown
//! -- -D warnings`, and `cargo test -p ocean-surface-ui --bins`. Every unit-test
//! column came back green, so it is not repeated per row.
//!
//! | mutation                                                              | guard | wasm clippy |
//! |-----------------------------------------------------------------------|-------|-------------|
//! | the whole affordance block deleted from the rail                       | RED | RED — see below |
//! | the affordance moved above the rows instead of after them              | RED | green |
//! | `retain_paged_tail` argument replaced with `false`                     | RED | green |
//! | the `if !retain_paged_tail` guard around the cursor park deleted       | RED | green |
//! | `rooms_next_page_cursor(grew, ..)` replaced with `success.next_cursor` | RED | RED — see below |
//! | the press's `rooms_more_in_flight` guard deleted                       | RED | green |
//! | the press's parked-cursor re-check deleted                             | RED | green |
//!
//! Five of the seven are this file's own catch: nothing else in the crate reads
//! where the control sits, and the four wiring rules all live in pure helpers
//! whose unit tests own the RULE and never its wiring — the mutated tree keeps
//! all 1258 of them green while the rail silently drops every page below the
//! first, once every eight seconds, in front of the member reading it.
//!
//! The two RED clippy rows are findings, not failures, and they have a shelf
//! life worth writing down. Deleting the block was assumed to leave a rail that
//! simply stops at 100 with every gate green; it does not, because
//! `rooms_more_in_flight` loses its last reader and the three `pub(crate)`
//! methods lose their only callers ("field `rooms_more_in_flight` is never
//! read", "methods `more_rooms_available`, `more_rooms_in_flight`, and
//! `load_more_rooms` are never used", "function `rooms_next_page_cursor` is
//! never used"). Bypassing the cursor helper is held by the last of those three
//! alone. Both holds are the accident of these being the helpers' ONLY callers,
//! which a second caller anywhere would end — the same way the rail rows in
//! `ci_failure_trigger_control.rs` were compiler-held until a flag table
//! elsewhere started constructing the same variants.

mod common;

use common::{read, view_source, without_whitespace};

/// The control. A member with 140 rooms can DO something about the 40 they
/// cannot see, and the rail TELLS them the 40 are there by growing a row that a
/// whole list does not have.
#[test]
fn the_rail_offers_the_next_page_and_fires_the_read() {
    let workspace = without_whitespace(&view_source("rooms_workspace.rs"));

    assert!(
        workspace.contains("{move||rooms.more_rooms_available().then(||view!{"),
        "the affordance renders on the parked cursor and on nothing else — a \
         rail holding every room the daemon has parks `None` and must grow no \
         control at all, because a control that says `Load more rooms` over a \
         complete list is a worse lie than the silence it replaced",
    );
    assert!(
        workspace.contains("class=\"rooms-workspace__load-more-rooms\""),
        "the class the stylesheet dresses",
    );
    assert!(
        workspace.contains("rooms.load_more_rooms();"),
        "the button must fire the read; a control that renders the state \
         without acting on it is the same unreachable rooms with extra steps",
    );
    assert!(
        workspace.contains("disabled=move||rooms.more_rooms_in_flight()"),
        "and it must refuse the second press while the first is in flight",
    );
}

/// Position, not merely presence. The affordance answers "is that all of them?"
/// — a question a member asks at the BOTTOM of the list — so it belongs inside
/// the scrolling list and after the rows. `contains` alone stays green with it
/// rendered above the rail, beside the create form, or under the status line.
///
/// Mutation run: moved above the rows instead of after them. Every other lane
/// stayed green — nothing else in the crate reads where this control sits, and
/// a row reading "Load more rooms" at the TOP of a list points at rooms that
/// are not above it.
#[test]
fn the_affordance_sits_at_the_end_of_the_scrolling_list() {
    let workspace = without_whitespace(&view_source("rooms_workspace.rs"));

    let list = workspace
        .find("class=\"rooms-workspace__left-list\"")
        .expect("the rail's scroll container");
    let rows = workspace
        .find("each=move||rooms.list.get()")
        .expect("the `<For>` over the room list");
    let affordance = workspace
        .find("class=\"rooms-workspace__load-more-rooms\"")
        .expect("the load-more affordance");
    let after_list = workspace
        .find("class=\"rooms-workspace__left-status\"")
        .expect("the status line, which is the first thing outside the list");

    assert!(
        list < rows && rows < affordance && affordance < after_list,
        "it names a position in the list — the end of what is loaded — so it \
         has to scroll with the list and follow the rows, not float over the \
         rail or sit outside it",
    );
}

/// The press. Its two guards are the transcript press's two guards, for the
/// same two reasons, and neither is held by anything the compiler checks —
/// measured: deleting either leaves both clippy lanes and all 1258 unit tests
/// green. The cursor needle below is the one exception in this test, held today
/// by `rooms_next_page_cursor` having exactly one caller.
#[test]
fn the_press_is_one_page_and_keeps_the_guards_a_press_needs() {
    let rooms = without_whitespace(&view_source("rooms.rs"));

    assert!(
        rooms.contains("pub(crate)fnload_more_rooms(&self){"),
        "the workspace needs a `pub(crate)` door to one more page; a private \
         one leaves the parked cursor unreachable from the view that renders \
         the affordance",
    );
    let body = fn_body(&rooms, "pub(crate)fnload_more_rooms(&self){");

    assert!(
        body.contains("ifself.rooms_more_in_flight.get_untracked(){return;}"),
        "a second press must not fire a second request against a cursor the \
         first has not moved yet",
    );
    assert!(
        body.contains("leturl=rooms_list_url(&base,Some(&cursor));"),
        "one page, through the same builder the first read uses, with the \
         cursor as an encoded query value",
    );
    assert!(
        body.contains("ifme.rooms_next_cursor.get_untracked().as_deref()!=Some(cursor.as_str()){"),
        "the page lands after an await, and an interactive refresh during one \
         re-parks the rail on its own first-page cursor — appending page N \
         onto a rail that has gone back to page one lists rooms that refresh \
         deliberately dropped",
    );
    assert!(
        body.contains(
            "me.rooms_next_cursor.set(rooms_next_page_cursor(grew,success.next_cursor));"
        ),
        "and the cursor the press leaves behind must go through the helper \
         that refuses to park one for a page that added nothing: the daemon \
         falls back to its FIRST page when the cursor names a closed room, so \
         `success.next_cursor` alone leaves a control permanently pressable \
         and permanently inert",
    );
}

/// The 8-second unread poll, which is the half no screenshot shows.
///
/// Mutations run: `retain_paged_tail` replaced with `false`, and the
/// `if !retain_paged_tail` guard deleted. The first leaves `cargo test` fully
/// green — `rooms_after_first_page`'s unit tests own the rule and never its
/// wiring — and silently deletes every page below the first, once every eight
/// seconds, while the member is looking at it. The second rewinds the parked
/// cursor to the end of page one on the same tick, so the next press re-serves
/// rooms already on screen instead of the ones behind them.
#[test]
fn the_unread_poll_reads_one_page_and_keeps_the_pages_it_did_not_read() {
    let rooms = without_whitespace(&view_source("rooms.rs"));

    assert!(
        rooms.contains(
            "letretain_paged_tail=matches!(mode,RoomsFetchMode::Silent)&&me.rooms_paged_beyond_first.get_untracked();"
        ),
        "only the silent poll retains, and only on a rail that has actually \
         paged: an interactive read is a fresh start and a rail that never \
         paged IS one page, so a fresh page is the whole truth about it",
    );
    assert!(
        rooms.contains(
            "letrooms=rooms_after_first_page(&me.list.get_untracked(),success.rooms,retain_paged_tail,);"
        ),
        "the poll must be TOLD whether it may replace the list — without the \
         argument it reads one page and throws away every other page the \
         member has loaded, eight seconds after they loaded it",
    );
    assert!(
        rooms.contains(
            "if!retain_paged_tail{me.rooms_paged_beyond_first.set(false);me.rooms_next_cursor.set(success.next_cursor);}"
        ),
        "a poll that kept a paged tail must keep that tail's cursor with it; \
         parking the first page's cursor there rewinds paging on every tick",
    );
    assert!(
        rooms.contains("letget_url=rooms_list_url(&base,None);"),
        "and it stays ONE request — the alternative this slice refused is a \
         poll that walks every loaded page every eight seconds",
    );
}

/// The stylesheet. Asserted by extracted rule body rather than byte-exact, the
/// lesson `dead_trigger_row_affordance.rs` carries: a pinned block fires on
/// unrelated design work and teaches the next author to delete the guard.
#[test]
fn the_affordance_is_a_full_width_row_not_a_floating_pill() {
    let normalized =
        css_without_whitespace(&strip_css_comments(&read("styles/rooms-workspace.css")));

    let body = rule_body(&normalized, ".rooms-workspace__load-more-rooms");
    assert!(
        body.contains("width:100%;"),
        "it marks the end of the loaded rail, so it spans it rather than \
         floating over it, got `{body}`",
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
        normalized.contains(".rooms-workspace__load-more-rooms:focus-visible{"),
        "a keyboard reader needs the same focus ring every other control in \
         this rail carries",
    );
    let disabled = rule_body(&normalized, ".rooms-workspace__load-more-rooms:disabled");
    assert!(
        !disabled.contains("pointer-events:none"),
        "the in-flight state is a fade and a cursor, never a removed hit area — \
         `pointer-events: none` suppresses the disabled cursor along with the \
         click, got `{disabled}`",
    );
}

// ---- Scanning helpers -------------------------------------------------------

/// The brace-matched body of the function whose whitespace-stripped signature is
/// `signature`, within whitespace-stripped Rust source. Depth counting assumes
/// the body holds no brace inside a string or char literal; the one scanned here
/// does not, and a `format!` added to it would need this helper taught about
/// quoting.
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
