## Review

- **Blocker — `crates/ocean-surface-ui/src/sessions.rs:147-207,1234-1273`** — The shared web Sessions panel now retains every zero-turn draft, directly contradicting the binding lazy-session policy requiring historical zero-turn litter to remain hidden. The updated test explicitly locks in the regression. **Smallest fix:** restore filtering in `group_for_panel`; let the Tauri-only Island derive its catalogue separately if zero-turn discovery is required there.

- **Blocker — `crates/ocean-surface-ui/src/main.rs:21-28`, `crates/ocean-surface-ui/src/components.rs:14,74-78`** — Shared tracked files reference untracked modules (`island.rs`, `island_dynamic.rs`, `search.rs`, `components/interactive_plot.rs`). A tracked-only commit, normal `git diff`, or patch transfer omits these files and breaks the build. `styles/island.css` and both new Island documents are likewise untracked. **Smallest fix:** commit each module/style atomically with its shared-file references, then validate the committed tree in a detached worktree.

- **High — `crates/ocean-surface-ui/src/components/interactive_plot.rs:1-867`, `crates/ocean-surface-ui/src/components.rs:74-78`** — The 867-line interactive-plot feature is unrelated to the only available scope contract, whose file map at `docs/OCEAN_DYNAMIC_ISLAND_BUILD_PLAN.md:163-175` does not include it. The requested root `plan.md` and `progress.md` are absent, so no broader authorization could be verified. **Smallest fix:** split the plot and associated component styling into a separate focused change.

- **High — `crates/ocean-surface-ui/src/daemon.rs:2828-2967`, `docs/OCEAN_DYNAMIC_ISLAND_BUILD_PLAN.md:88-116`** — Recall depends on a new sibling-repository daemon endpoint, `GET /v1/agent/history/search`. This repository cannot provide or deploy that route, and no cross-repository validation evidence is present. A Surface-first deployment leaves Recall reporting an error. **Smallest fix:** land and validate the `ocean-os` endpoint first or gate Recall on an explicit daemon capability/version.

- **Medium — `crates/ocean-surface-ui/src/daemon.rs:5680-5730`** — `recall_input_change_invalidates_stale_results_before_debounce` only tests synchronous clearing and generation increment. It does not exercise two out-of-order HTTP completions, despite the implementation contract claiming stale responses cannot replace newer results. **Smallest fix:** extract the generation acceptance decision into a pure helper and test old/new response ordering, or use a mock transport.

- **Medium — `crates/ocean-tauri/src/lib.rs:1058-1082`** — The diagnostic-script harness waits a fixed two seconds before `webview.eval`, rather than using the existing `ui_ready` handshake. Slow release startup can make screenshots flaky; fast startup still wastes time. **Smallest fix:** queue the script and execute it when `ui_ready` confirms the Leptos listener/UI is mounted, with a bounded timeout only as fallback.

- **Low — `AGENTS.md:81-85`** — `index.html` and `extension/sidepanel.html` correctly enumerate `island.css` in matching order, and `scripts/build-extension.sh` copies all CSS, but the binding stylesheet-order documentation still omits the new Island domain. **Smallest fix:** add `island` after `chrome` to the documented cascade.

- **Low — `docs/OCEAN_DESKTOP_NORTH_STAR.md:10-12`** — The document names the host gate as `running_as_tauri()`, while implementation uses `running_in_tauri()`. **Smallest fix:** use the actual function name.

## Residual manual checks

1. Install `wasm32-unknown-unknown`, rerun the WASM check and extension build.
2. Validate Agent, Sessions, Recall, Escape, outside-click, focus restoration, IME handling, and both native widths.
3. Exercise Recall against the matching `ocean-os` revision, including out-of-order responses.
4. Verify File/Edit/Window roles and `CmdOrCtrl+N` on macOS; smoke-test menu portability on Windows/Linux.
5. Run proxy check and a detached-worktree release/Trunk build from the eventual commit.
6. Confirm the missing root `plan.md`/`progress.md` were not required delivery artifacts.