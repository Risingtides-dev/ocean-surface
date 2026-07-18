## Review

- **Pass — no remaining blocker.**
- **Correct:** Recall input immediately calls `invalidate_history_search()` before scheduling the 220 ms debounce (`crates/ocean-surface-ui/src/island_dynamic.rs:837-851`). Invalidation increments the search generation and clears stale results/error/searching state (`crates/ocean-surface-ui/src/daemon.rs:2750-2758`), preventing prior results from remaining actionable.
- **Correct:** Agent keys include every `IslandAttentionItem` snapshot field; Recall keys include every `HistorySearchHit` field (`crates/ocean-surface-ui/src/island_dynamic.rs:60-93`). These keys are used by the Agent and Recall keyed lists at lines 222 and 882.
- **Correct:** Regression tests cover immediate invalidation (`crates/ocean-surface-ui/src/daemon.rs:5083-5113`) and same-ID Agent/Recall content replacement (`crates/ocean-surface-ui/src/island_dynamic.rs:938-975`).
- **Note:** The requested root `plan.md` and `progress.md` were absent. No new blocker was introduced by the reviewed fixes. No files were modified or staged.