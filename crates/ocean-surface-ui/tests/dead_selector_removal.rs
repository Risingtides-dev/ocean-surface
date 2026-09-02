//! TASK-49 — dead CSS selector removal regressions.
//!
//! Source-assertion tests (same style as mobile_component_reflow.rs /
//! mobile_composer_regressions.rs) that pin the removal of three dead selectors
//! confirmed to have no Rust emitter:
//!   * `.island-group`               (was styles/island.css)
//!   * `.ocean-council-modal__frame` (was styles/chrome.css)
//!   * `.ocean-map__panel` (was styles/components.css, incl. its `gmp-place-search` descendant rule)
//!   * the whole `.rooms-*` panel family (was styles/panels.css, plus its
//!     `.room-stage .rooms-*` re-scales in styles/call.css)
//!
//! Each is asserted absent from BOTH the stylesheet AND the Rust source tree,
//! so it can neither be re-added to CSS nor emitted from a component without
//! failing this guard. Live sibling selectors that must survive are asserted
//! present so the deletions can't over-reach.

mod common;

use common::{all_rust_src, read};

// ---- Deleted selectors are gone from CSS and never emitted from Rust --------

#[test]
fn island_group_selector_is_removed_and_unemitted() {
    assert!(
        !read("styles/island.css").contains(".island-group"),
        "`.island-group` is dead (no Rust emitter) and must not reappear in \
         styles/island.css (TASK-49)",
    );
    assert!(
        !all_rust_src().contains("island-group"),
        "`island-group` must have no Rust emitter — if one is added, restore \
         the stylesheet rule instead of relying on a dead selector (TASK-49)",
    );
}

#[test]
fn council_modal_frame_selector_is_removed_and_unemitted() {
    assert!(
        !read("styles/chrome.css").contains(".ocean-council-modal__frame"),
        "`.ocean-council-modal__frame` is dead (the council modal emits \
         __bar/__title/__close, never __frame) and must not reappear in \
         styles/chrome.css (TASK-49)",
    );
    assert!(
        !all_rust_src().contains("ocean-council-modal__frame"),
        "`ocean-council-modal__frame` must have no Rust emitter (TASK-49)",
    );
}

#[test]
fn map_panel_selector_is_removed_and_unemitted() {
    let components = read("styles/components.css");
    assert!(
        !components.contains(".ocean-map__panel"),
        "`.ocean-map__panel` (and its `gmp-place-search` descendant rule) is \
         dead (MapView emits only .ocean-map / .ocean-map__loading) and must \
         not reappear in styles/components.css (TASK-49)",
    );
    assert!(
        !all_rust_src().contains("ocean-map__panel"),
        "`ocean-map__panel` must have no Rust emitter (TASK-49)",
    );
}

/// The rooms *panel* — a right slide-over with its own list, create form,
/// policy details, roster chips, message rows, outbox and composer — was never
/// rendered by the Leptos surface. `rooms_workspace.rs` is the shipped rooms
/// UI and emits `.rooms-workspace__*` throughout; every `.rooms-<leaf>` class
/// below had zero emitters anywhere in `src/`, in `index.html`, or in the
/// extension wrapper. ~110 selector occurrences of styling nobody could see.
///
/// The cost of leaving them was not weight, it was cover: a guard in `app.rs`
/// asserted on one of these dead rules, and `mobile_composer_regressions.rs`
/// asserted the iOS 16px anti-zoom floor on three dead input classes — so the
/// live floor in `styles/rooms-workspace.css` could be deleted with every gate
/// green. That test now names the six live fields instead.
#[test]
fn rooms_panel_selector_family_is_removed_and_unemitted() {
    let panels = read("styles/panels.css");
    let call = read("styles/call.css");
    let rust = all_rust_src();
    // One representative per removed block. `rooms-panel` covers the shell,
    // head, list, create form and status; the rest are its leaves.
    for dead in [
        "rooms-panel",
        "rooms-overlay",
        "rooms-item",
        "rooms-chip",
        "rooms-policy",
        "rooms-msg",
        "rooms-outbox",
        "rooms-mention-hint",
        "rooms-composer",
        "rooms-addagent",
    ] {
        assert!(
            !panels.contains(dead),
            "`.{dead}` belongs to the rooms panel nothing renders and must not \
             reappear in styles/panels.css — the shipped rooms UI is \
             rooms_workspace.rs and styles it with `.rooms-workspace__*`",
        );
        assert!(
            !call.contains(dead),
            "the `.room-stage .{dead}` re-scale in styles/call.css re-scaled a \
             base rule that no longer exists and must not reappear",
        );
        assert!(
            !rust.contains(dead),
            "`{dead}` must have no Rust emitter — if the rooms panel is ever \
             rebuilt, land its CSS with the markup, not ahead of it",
        );
    }
}

// ---- Live siblings must survive the deletions -------------------------------

#[test]
fn live_siblings_of_deleted_selectors_survive() {
    // The deletions must not over-reach into adjacent live rules.
    assert!(
        read("styles/island.css").contains(".island-result"),
        ".island-result is live and must survive the .island-group deletion",
    );
    assert!(
        read("styles/chrome.css").contains(".ocean-council-modal__close"),
        ".ocean-council-modal__close is live and must survive the __frame \
         deletion",
    );
    let components = read("styles/components.css");
    assert!(
        components.contains(".ocean-map__loading"),
        ".ocean-map__loading is live and must survive the __panel deletion",
    );
    // The sessions slide-over shared every one of the rooms panel's grouped
    // `:is(...)` rules — shell, close button, field treatment, scrollbars. It
    // is live, and unpicking the rooms members must not have taken it with
    // them. Its coarse-pointer floor is asserted in
    // mobile_composer_regressions.rs.
    let panels = read("styles/panels.css");
    for live in [
        ".sessions-overlay",
        ".sessions-panel__close",
        ".sessions-panel__list",
        ".sessions-create__input",
    ] {
        assert!(
            panels.contains(live),
            "{live} is live and must survive the rooms-panel deletion",
        );
    }
    // Same for the room stage's own classes in call.css: only the
    // `.room-stage .rooms-*` descendant re-scales were removed.
    let call = read("styles/call.css");
    assert!(
        call.contains(".room-stage__transcript") && call.contains(".room-stage__head"),
        ".room-stage__* is a separate family and must survive the \
         `.room-stage .rooms-*` deletion",
    );
    // The bare Places UI Kit element selectors are theming for Google's own
    // web components (not Rust-class-emitted) and must stay after only the
    // `.ocean-map__panel gmp-place-search` descendant selector was removed.
    assert!(
        components.contains("gmp-place-details") && components.contains("gmp-place-search"),
        "the gmp-place-details / gmp-place-search theming rule must survive \
         (only the .ocean-map__panel descendant selector was removed)",
    );
}
