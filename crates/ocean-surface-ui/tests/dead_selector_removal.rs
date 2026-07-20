//! TASK-49 — dead CSS selector removal regressions.
//!
//! Source-assertion tests (same style as mobile_component_reflow.rs /
//! mobile_composer_regressions.rs) that pin the removal of three dead selectors
//! confirmed to have no Rust emitter:
//!   * `.island-group`               (was styles/island.css)
//!   * `.ocean-council-modal__frame` (was styles/chrome.css)
//!   * `.ocean-map__panel` (was styles/components.css, incl. its `gmp-place-search` descendant rule)
//!
//! Each is asserted absent from BOTH the stylesheet AND the Rust source tree,
//! so it can neither be re-added to CSS nor emitted from a component without
//! failing this guard. Live sibling selectors that must survive are asserted
//! present so the deletions can't over-reach.

use std::path::Path;

fn repo_root() -> &'static Path {
    // tests/ -> ocean-surface-ui/ -> crates/ -> repo root
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Every `.rs` file under the crate's `src/`, read into one blob so a class
/// literal emitted anywhere in the component tree is detectable.
fn all_rust_src() -> String {
    let src = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
    let mut blob = String::new();
    let mut stack = vec![src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in
            std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                blob.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
                blob.push('\n');
            }
        }
    }
    blob
}

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
    // The bare Places UI Kit element selectors are theming for Google's own
    // web components (not Rust-class-emitted) and must stay after only the
    // `.ocean-map__panel gmp-place-search` descendant selector was removed.
    assert!(
        components.contains("gmp-place-details") && components.contains("gmp-place-search"),
        "the gmp-place-details / gmp-place-search theming rule must survive \
         (only the .ocean-map__panel descendant selector was removed)",
    );
}
