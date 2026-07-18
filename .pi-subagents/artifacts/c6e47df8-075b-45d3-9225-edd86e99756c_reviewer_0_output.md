## Review

- **Correct:** Upstream Voice Planner logic remains present. Island opening cancels non-idle Planner state, while opening Planner closes the Island (`crates/ocean-surface-ui/src/app.rs:1204-1221`, `1781-1805`).
- **Correct:** Shortcut helpers preserve browser Print/Find and bypass IME composition (`crates/ocean-surface-ui/src/app.rs:54-74`, `1658-1682`).
- **Correct:** EventSource session/generation markers and stale-event guards remain exact-session scoped (`crates/ocean-surface-ui/src/daemon.rs:2073-2151`, `3294-3334`).
- **Correct:** History searches use a monotonic generation guard so stale responses cannot replace newer results (`crates/ocean-surface-ui/src/daemon.rs:2904-2966`).

- **Blocker (high): Connecting EventSources are not lifecycle-owned or promptly closed during session replacement.** `connect()` only stores each source after `await_event_source_open()` (`crates/ocean-surface-ui/src/daemon.rs:2030-2084`; permission equivalent `2226-2263`). Until then, `close_planner_stream_sources()` at `1931` sees `None`. Rapid switches can therefore leave stale CONNECTING agent/control streams alive for the five-second open timeout, consuming browser connection slots. Verified in gloo-net 0.6 source: clones share the underlying `web_sys::EventSource`, and dropping/closing any clone closes it and dispatches an error.  
  **Smallest safe fix:** install the source in `PlannerStreamSources` immediately after construction with `Connecting` lifecycle state, before awaiting OPEN; then a generation replacement can close it. Re-check generation after the await before publishing readiness.  
  **Missing test:** browser/WASM lifecycle test using a never-opening EventSource, followed by rapid generation changes, asserting each stale source is closed immediately.

- **Blocker (high): Session switching can erase live events received during hydration.** `switch_session()` starts snapshot loading and `connect()` concurrently (`crates/ocean-surface-ui/src/daemon.rs:3208-3211`). The stream can apply an event before `hydrate_active_session()` later replaces the entire transcript (`crates/ocean-surface-ui/src/daemon.rs:3241-3283`). The session-id guard does not help because both operations target the same session. This is especially relevant now that Sessions and Recall make switching a primary interaction.  
  **Smallest safe fix:** provide one coordinated switch operation that buffers the new generation’s SSE events while applying the initial snapshot, then folds buffered events afterward. Simply hydrating before subscribing still leaves an SSE live-tail gap for sessions shared with another surface.  
  **Missing test:** delayed snapshot plus an intervening same-session SSE delta, proving the final transcript retains both persisted and live content.

- **High: The full web/extension Sessions panel now exposes historical zero-turn drafts.** `group_for_panel()` removed the established active-only zero-turn filter for every host (`crates/ocean-surface-ui/src/sessions.rs:139-153`). The Island already derives its zero-turn-discoverable list directly from `session_list`; changing the shared full panel is unnecessary and contradicts the Dynamic Island plan’s requirement that browser/extension behavior remain unchanged.  
  **Smallest safe fix:** restore filtering in `group_for_panel()` and keep zero-turn discoverability solely in `derive_island_sessions()`. Restore the previous panel test and retain an Island-specific zero-turn test.

- **Medium: Escape can close two surfaces in one keypress.** SessionsPanel handles Escape without stopping propagation (`crates/ocean-surface-ui/src/sessions.rs:736-740`). Because Sessions can coexist with `deck_panel`, it first sets `show_sessions=false`; the event then reaches the window handler (`crates/ocean-surface-ui/src/app.rs:1690-1705`), which observes Sessions already closed and closes the deck too. This violates the stated “exactly one topmost surface” contract.  
  **Smallest safe fix:** call `stop_propagation()` in SessionsPanel’s handled Escape branch.  
  **Missing test:** DOM/browser test with both deck and Sessions open, asserting one Escape closes only Sessions.

- **Medium: The diff widens scope beyond the Dynamic Island plan.** The unrelated interactive-plot feature adds an 867-line parser/renderer plus component registration and approximately 294 CSS lines (`crates/ocean-surface-ui/src/components/interactive_plot.rs`, `crates/ocean-surface-ui/src/components.rs:14-15,65-71`, `styles/components.css:758-1048`). It is absent from the Island build plan’s file map.  
  **Smallest safe fix:** split the interactive-plot work into a separate change.

- **Note:** Shortcut tests cover only the pure key-to-mode helper (`crates/ocean-surface-ui/src/app.rs:2937-2962`). There are no browser-level tests proving listener ordering for Cmd/Ctrl+K, Escape propagation, Planner/Island replacement, focus restoration, or that only one Island mode is mounted.
- **Note:** Requested `plan.md` and `progress.md` were absent at the supplied paths. Scope was checked against `docs/OCEAN_DYNAMIC_ISLAND_BUILD_PLAN.md` and the implementation contract instead.
- **Note:** No build/test command was run before the tool budget expired; acceptance should remain blocked until the documented Rust/WASM/Tauri/extension gates run.