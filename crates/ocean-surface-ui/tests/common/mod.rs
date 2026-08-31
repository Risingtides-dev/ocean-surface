//! The source-scanning toolkit this crate's guard tests share.
//!
//! `ocean-surface-ui` is a BINARY crate — its Cargo.toml says so in as many
//! words ("A Trunk CSR app is a plain binary crate: src/main.rs + fn main().
//! No [lib] needed") — so an integration test in `tests/` cannot import a
//! single item from it, cannot mount a component, and cannot press anything.
//! Reading the source is the only lever a guard here has, which is why every
//! test in this directory is a scanner and why the scanners want one toolkit
//! rather than four copies of it.
//!
//! This is a SUBDIRECTORY module on purpose. Cargo compiles each top-level
//! `tests/*.rs` as its own test binary; a `tests/common/mod.rs` is not a
//! target, it is a file the binaries pull in with `mod common;`.
//!
//! `#![allow(dead_code)]` because that inclusion is per-binary: each guard
//! calls only the helpers it needs, and an uncalled `pub fn` in a test binary
//! is a `dead_code` warning — which the gate's `-D warnings` turns into a
//! failure in every binary that happens not to use it.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// The repository root. Stylesheets live at the repo's `styles/`, never under
/// the crate, so a guard reading CSS has to climb out of `CARGO_MANIFEST_DIR`.
pub fn repo_root() -> &'static Path {
    // tests/ -> ocean-surface-ui/ -> crates/ -> repo root
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// The UI crate's `src/`.
pub fn src_root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))
}

/// A file read relative to the repository root — `styles/rooms-workspace.css`
/// and friends.
pub fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// A module read relative to the crate's `src/` — `rooms_workspace.rs`.
pub fn src(rel: &str) -> String {
    let path = src_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read src/{rel}: {e}"))
}

/// The half of a `src/` module that a release build actually compiles.
///
/// A scan whose needle the module's own unit tests could quote stops here.
/// Those tests assert on what the view is SUPPOSED to render, so a needle they
/// satisfy pins nothing while the view says whatever it likes — and this is
/// not theoretical: `state.confirm_destroy.set(true)` occurs five times in
/// `room_workspace_panel.rs` and exactly once outside its test module.
pub fn view_source(rel: &str) -> String {
    src(rel)
        .split_once("#[cfg(test)]")
        .unwrap_or_else(|| panic!("src/{rel} carries its unit tests at the bottom"))
        .0
        .to_string()
}

/// Whitespace stripped wholesale, so rustfmt is free to wrap a needle however
/// it likes without breaking the test. It does mean needles read without their
/// spaces: `"CIfailure"` is the label `"CI failure"`.
pub fn without_whitespace(source: &str) -> String {
    source
        .chars()
        .filter(|char| !char.is_whitespace())
        .collect()
}

/// Every `.rs` file under the crate's `src/`, read into one blob, so a literal
/// emitted anywhere in the component tree is detectable.
pub fn all_rust_src() -> String {
    let mut blob = String::new();
    let mut stack: Vec<PathBuf> = vec![src_root().to_path_buf()];
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
