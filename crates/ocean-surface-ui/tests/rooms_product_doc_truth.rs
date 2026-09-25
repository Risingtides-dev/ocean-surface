//! Rooms DoD 5.4 — `docs/OCEAN_ROOMS_PRODUCT.md` may only name what exists.
//!
//! An audit found the product doc describing a surface that was never built:
//! `Local`/`Remote`/`None` access states, rooms opening off the unpaged GET,
//! relative timestamps, and a `.rooms-panel__list` rule whose CSS had already
//! been deleted (see `dead_selector_removal.rs`). Nothing held the doc, so it
//! drifted with every gate green. This guard holds the two facts a reader acts
//! on and a scanner can check:
//!
//! * every ACCESS STATE the doc names is one of `RoomAccessState`'s five
//!   variants, read from `src/rooms.rs` itself so the doc cannot agree with a
//!   list this file made up; and
//! * every `.rooms-*` CSS class the doc names exists in `styles/` or is
//!   emitted somewhere in the crate's Rust source.

mod common;

use std::collections::BTreeSet;

use common::{all_rust_src, read, repo_root, src};

const DOC: &str = "docs/OCEAN_ROOMS_PRODUCT.md";

/// The five wire states — also ocean-os `docs/contracts/room-wire.json`'s
/// `access_states`. Asserted equal to the enum below, never trusted alone.
const ACCESS_STATES: [&str; 5] = ["local", "connecting", "live", "recovering", "revoked"];

/// Backticked tokens that legitimately share a line with the word "access"
/// without being an access state. Anything else backticked and bare-word on
/// such a line is read as a claimed state and must be one of the five.
const ACCESS_LINE_NON_STATES: [&str; 3] = ["access", "outbox", "state"];

/// The variants of `pub enum RoomAccessState { .. }` in `src/rooms.rs`,
/// snake-cased the way `#[serde(rename_all = "snake_case")]` puts them on the
/// wire (all five are single words, so lowercasing is that conversion).
fn access_state_variants() -> BTreeSet<String> {
    let rooms = src("rooms.rs");
    let body = rooms
        .split_once("pub enum RoomAccessState {")
        .expect("src/rooms.rs declares `pub enum RoomAccessState`")
        .1
        .split_once('}')
        .expect("RoomAccessState's body closes")
        .0;
    body.split(',')
        .map(|variant| variant.trim().to_lowercase())
        .filter(|variant| !variant.is_empty())
        .collect()
}

/// Every `` `token` `` on `line` shaped like a state name: one alphabetic word,
/// either the wire form (`live`) or a bare variant (`Live`). Multi-capital
/// type names such as `RoomAccessProjection` are not state names and are
/// skipped.
fn backticked_words(line: &str) -> Vec<&str> {
    line.split('`')
        .skip(1)
        .step_by(2)
        .filter(|token| {
            let mut chars = token.chars();
            chars
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic())
                && chars.all(|c| c.is_ascii_lowercase())
        })
        .collect()
}

/// The body of the doc's `### Access States` section, up to the next heading.
fn access_states_section(doc: &str) -> &str {
    let after = doc
        .split_once("### Access States")
        .expect("the product doc keeps an `### Access States` section")
        .1;
    match after.find("\n#") {
        Some(end) => &after[..end],
        None => after,
    }
}

/// Every `.rooms-<class>` the doc names, dot stripped.
fn doc_rooms_selectors(doc: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let bytes = doc.as_bytes();
    let mut i = 0;
    while let Some(offset) = doc[i..].find(".rooms-") {
        let start = i + offset + 1;
        let mut end = start;
        while end < bytes.len()
            && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_' || bytes[end] == b'-')
        {
            end += 1;
        }
        found.insert(doc[start..end].trim_end_matches('-').to_string());
        i = end;
    }
    found
}

/// `needle` occurs in `hay` as a whole class name — not merely as the prefix
/// of a longer live class, which would let `.rooms-workspace__left` ride on
/// `.rooms-workspace__left-list`.
fn contains_whole_class(hay: &str, needle: &str) -> bool {
    hay.match_indices(needle).any(|(at, _)| {
        !hay[at + needle.len()..]
            .chars()
            .next()
            .is_some_and(|next| next.is_ascii_alphanumeric() || next == '_' || next == '-')
    })
}

fn all_styles() -> String {
    let dir = repo_root().join("styles");
    let mut blob = String::new();
    for entry in std::fs::read_dir(&dir).expect("styles/ exists") {
        let path = entry.expect("styles entry").path();
        if path.extension().and_then(|e| e.to_str()) == Some("css") {
            blob.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
            blob.push('\n');
        }
    }
    blob
}

#[test]
fn the_five_access_states_are_the_enum_s_variants() {
    let expected: BTreeSet<String> = ACCESS_STATES.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        access_state_variants(),
        expected,
        "RoomAccessState in src/rooms.rs no longer has exactly the five wire states; \
         update ACCESS_STATES here, docs/OCEAN_ROOMS_PRODUCT.md and the vendored \
         room-wire contract together",
    );
}

#[test]
fn product_doc_lists_exactly_the_five_access_states() {
    let doc = read(DOC);
    let listed: BTreeSet<String> = access_states_section(&doc)
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("- `"))
        .filter_map(|rest| rest.split_once('`').map(|(state, _)| state.to_string()))
        .collect();
    let variants = access_state_variants();
    assert_eq!(
        listed, variants,
        "{DOC}'s `### Access States` list must name exactly RoomAccessState's variants \
         in wire form",
    );
}

#[test]
fn product_doc_names_no_access_state_outside_the_five() {
    let doc = read(DOC);
    let variants = access_state_variants();
    for (number, line) in doc.lines().enumerate() {
        let names_access = line.to_lowercase().contains("access");
        let in_state_list = access_states_section(&doc).contains(line) && line.contains("- `");
        if !names_access && !in_state_list {
            continue;
        }
        for word in backticked_words(line) {
            let lowered = word.to_lowercase();
            assert!(
                variants.contains(&lowered) || ACCESS_LINE_NON_STATES.contains(&lowered.as_str()),
                "{DOC}:{} names `{word}` beside room access, but the access states are \
                 exactly {variants:?} (RoomAccessState in src/rooms.rs). If `{word}` is \
                 not a state, add it to ACCESS_LINE_NON_STATES",
                number + 1,
            );
        }
    }
    // The audit's specific ghosts, independent of formatting.
    for ghost in ["`Remote`", "`remote`"] {
        assert!(
            !doc.contains(ghost),
            "{DOC} names {ghost}: there is no remote access state"
        );
    }
}

#[test]
fn product_doc_names_only_live_rooms_selectors() {
    let doc = read(DOC);
    let selectors = doc_rooms_selectors(&doc);
    assert!(
        !selectors.is_empty(),
        "{DOC} names no `.rooms-*` selector — the scan found nothing to hold, so \
         either the doc lost its rail/transcript classes or this scanner broke",
    );
    let styles = all_styles();
    let rust = all_rust_src();
    for class in &selectors {
        let in_css = contains_whole_class(&styles, &format!(".{class}"));
        let in_rust = contains_whole_class(&rust, class);
        assert!(
            in_css || in_rust,
            "{DOC} names `.{class}`, which no stylesheet in styles/ defines and no \
             Rust source emits — a dead selector (the doc once bound \
             `.rooms-panel__list` after its CSS was deleted)",
        );
    }
}
