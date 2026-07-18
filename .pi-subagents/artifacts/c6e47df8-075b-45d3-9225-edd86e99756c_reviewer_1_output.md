## Review

- **Correct:** Daemon/runtime authority remains in `ocean-os`. Recall calls the daemon route with `limit=20` (`crates/ocean-surface-ui/src/daemon.rs:2932-2944`); the current daemon performs bounded local transcript search without provider/embedding calls and enforces query, concurrency, and store limits (`/Users/smathdaddy-macbook/ocean-os/crates/ocean-daemon/src/history_search.rs:40-83`).
- **Correct:** Recall query generations prevent an older query from overwriting newer input (`crates/ocean-surface-ui/src/daemon.rs:2901-2967`).
- **Correct:** Planner EventSources retain one owner, explicitly close replaced sources outside the `RefCell` borrow, and reject stale generations (`crates/ocean-surface-ui/src/daemon.rs:4281-4334`, plus the post-hydration generation check around `1996-2004`).
- **Correct:** `interactive_plot` performs no JavaScript evaluation or provider calls. Expressions use a local allowlisted AST; non-finite results are rejected; arrays, samples, expression length, and emitted numeric parameters are bounded (`crates/ocean-surface-ui/src/components/interactive_plot.rs:10-14,75-95,265-312,328-457,479-544,712-739`).
- **Correct:** Tauri diagnostic script evaluation and resize authority require the explicit `OCEAN_UI_DEBUG_SCRIPT` environment opt-in (`crates/ocean-tauri/src/lib.rs:267-277,1062-1086`). No daemon/provider authority was added to that hook.

### Findings

- **Blocker — global permission polling transports raw tool arguments.**  
  `crates/ocean-surface-ui/src/daemon.rs:773-789,2861-2895` describes an args-free DTO but polls the current global `GET /v1/permissions`. The actual daemon wire contract includes `PermissionStatus.args` (`/Users/smathdaddy-macbook/ocean-os/crates/ocean-core/src/lib.rs:694-705`). Serde ignores rather than retains the field, but every poll still transfers raw arguments for all pending sessions into the Tauri webview/network path. Commands, edit content, paths, or embedded secrets therefore cross a broader Surface boundary than the focused permission stream.  
  **Smallest safe fix:** add/use an args-free daemon attention projection or an `include_args=false` contract whose serializer never sends `args`. Until then, derive compact attention from request snapshots without globally fetching raw permission records.  
  **Missing test:** daemon/Surface integration fixture asserting the compact attention response contains no `args`, decision token, provider payload, sender, or runtime handle.

- **High — accepted cancellation is presented as terminal too early.**  
  `crates/ocean-surface-ui/src/daemon.rs:3163-3180` validates the response body correctly, but then sets focused `streaming=false`. The daemon contract only transitions to `Cancelling` when it responds successfully; terminal `Cancelled` is recorded later (`/Users/smathdaddy-macbook/ocean-os/crates/ocean-daemon/src/main.rs:1692-1718`, `request_control.rs:210-223`). This releases the composer/Stop state while work may still be unwinding, enabling a new turn or falsely implying execution stopped.  
  **Smallest safe fix:** keep `streaming` and `active_turn_id` until the terminal SSE frame; represent the accepted POST separately as “stopping…” via the cancellation set.  
  **Missing test:** accepted cancel remains streaming/cancelling until matching terminal SSE; rejected cancel restores Stop; unrelated request completion does not clear it.

- **Medium — Island permission actionability is not bound to the originating request.**  
  `crates/ocean-surface-ui/src/island.rs:255-270` checks only permission ID, focused session, and whether *some* local decision token exists. `crates/ocean-surface-ui/src/island_dynamic.rs:244-250` consequently marks a same-session permission from another client/turn actionable using the latest local token. The daemon rejects the wrong token, so this does not bypass server authorization, but the Surface inaccurately claims submitter authority.  
  **Smallest safe fix:** retain `request_id` on focused pending permissions and require it to equal the locally owned active request before showing Approve/Deny; alternatively add a non-secret daemon-issued submitter identity.  
  **Missing test:** a foreign permission in the same focused session remains read-only even when a different local turn token exists.

- **Medium — closing Recall does not retire its query or response.**  
  Closing only changes mode (`crates/ocean-surface-ui/src/island_dynamic.rs:474-475`); invalidation occurs only on input (`835-852`). A request completing after close may therefore repopulate `history_results`, and sensitive excerpts/query text remain in signals and become actionable immediately on reopening.  
  **Smallest safe fix:** on leaving Recall, increment both debounce/search generations and clear query, results, error, and searching state.  
  **Missing test:** close Recall before response completion, then verify the response is discarded and reopening starts empty.

- **Note — plot limits bound dimensions but not aggregate evaluation work.**  
  Maximum expressions can be evaluated across six series and 512 samples on every reactive input (`crates/ocean-surface-ui/src/components/interactive_plot.rs:403-420,479-500,711-732`). The 512-character cap bounds the damage, but a maximally nested/operator-heavy component can still cause roughly millions of recursive node visits per slider event. Display strings are also unbounded.  
  **Smallest safe fix:** cap AST depth/node count and aggregate `nodes × series × samples`; bound display metadata lengths.  
  **Missing test:** maximum aggregate budget rejection, deep nesting, oversized labels, and exact `parameters_changed` wire shape.

## Review Gate

**Rejected pending the raw-permission-payload blocker and cancellation truthfulness fix.** No source files were modified during review. The requested root `plan.md` and `progress.md` were absent; planning and worker evidence were recovered from `.pi-subagents` artifacts instead.