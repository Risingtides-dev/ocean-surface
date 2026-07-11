//! Embed the git revision into the wasm as `OCEAN_SURFACE_REV`.
//!
//! Two jobs:
//!   1. Provenance — the app logs `ocean-surface <rev>` on boot, so any
//!      browser/webview instantly answers "which build is this?".
//!   2. Cache keying — Trunk computes the dist filename hash from the
//!      pre-wasm-opt module, so byte-identical source produces the same
//!      hashed URL even when the optimization pipeline changes. Embedding
//!      the commit makes every landed commit produce a distinct module, so
//!      an `immutable`-cached URL can never alias two different builds.
//!
//! The deploy contract keeps the rev honest: release builds run in a fresh
//! detached worktree at the pushed commit (clean tree), so the embedded rev
//! is exactly the deployed sha. Dev builds may carry `-dirty`; that flag is
//! best-effort (the rerun watchers below track HEAD and the index, not every
//! working-tree file mtime).

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn main() {
    let sha = git(&["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let rev = if dirty { format!("{sha}-dirty") } else { sha };
    println!("cargo::rustc-env=OCEAN_SURFACE_REV={rev}");
    // Watch the REAL per-checkout git files so a new commit re-embeds the rev.
    // `--git-path` resolves linked worktrees (where `.git` is a gitfile and
    // `.git/HEAD` does not exist); a plain relative guess would go stale.
    for name in ["HEAD", "index"] {
        if let Some(p) = git(&["rev-parse", "--git-path", name]) {
            println!("cargo::rerun-if-changed={p}");
        }
    }
}
