## Review

- **Correct:** Projection snapshot reduction follows daemon precedence: checkpoint replacement, ordered pending content frames, then authoritative folded state (`crates/ocean-surface-ui/src/daemon.rs:1925-2047`). Folded permission frames are not double-applied, and state variants/session IDs are validated.
- **Correct:** Cursor handling requires valid `sp1` cursors, matching prefixes, contiguous next sequence, and matching SSE `id:`/body cursor (`crates/ocean-surface-ui/src/daemon.rs:1578-1637`, `3809-3827`). This matches the daemon’s opaque cursor and Last-Event-ID override contract (`ocean-daemon/src/main.rs:6882-6898`).
- **Correct:** Projection reconnects are generation-scoped and reset all stored transports before refetch (`crates/ocean-surface-ui/src/daemon.rs:3419-3458`). New-daemon projection absence is retried without silently falling back; legacy fallback requires `persistence_revision` to be absent (`3461-3569`).
- **Correct:** Proxy routing preserves the full query and forwards `Last-Event-ID` (`crates/ocean-surface-proxy/src/main.rs:1130-1162`). The daemon consumes that header ahead of `after_cursor` (`ocean-daemon/src/main.rs:6882-6898`).
- **Correct:** The daemon excludes `surface_patch`, Slack canvas, and extension events from the projection journal (`ocean-daemon/src/session_projection.rs:295-305`), while the surface canvas reducer accepts only same-session `surface_patch` frames (`crates/ocean-surface-ui/src/daemon.rs:3950-3967`).

- **High:** **Canvas patches can be lost across session attach.** The projection snapshot is installed before the canvas tail is opened (`crates/ocean-surface-ui/src/daemon.rs:3701-3708`), the daemon intentionally excludes patches from projection (`ocean-daemon/src/session_projection.rs:295-305`), and the canvas URL opens `/v1/agent/events?session_id=...` without an initial cursor or `replay=1` (`crates/ocean-surface-ui/src/daemon.rs:3922-3928`). Any patch emitted during the projection GET/install gap is absent from both sources. This conflicts with the stated separate canvas rail/cursor model.
- **Medium:** **“Unique call ID” enforcement is only local to an outstanding pair.** `component_replay_matches` removes an ID after its result (`crates/ocean-surface-ui/src/daemon.rs:5623-5634`), so `call(c) → success(c) → duplicate call(c) → success(c)` creates two valid matches despite the function’s uniqueness claim at `5602-5603`. The test only covers two duplicate calls before one result (`7178-7221`). Malformed/reused persisted IDs can therefore replay multiple or conflicting component effects.
- **Low:** **Projection-tail lane isolation is asymmetric.** Snapshot reduction ignores canvas events, but live agent frames pass any same-session `AgentEvent` to `apply_event` (`crates/ocean-surface-ui/src/daemon.rs:3829-3860`), including `SurfacePatch`. Exact daemon output prevents this, but a corrupt/incompatible projection frame can cross into canvas state rather than forcing resync.

- **Note:** Requested `plan.md` and `progress.md` were absent at the supplied paths, so they could not be reviewed.
- **Note:** No files were edited or staged.

### Residual live-test risks

- Browser EventSource reconnect behavior—including 30-second daemon connection rotation, browser-generated `Last-Event-ID`, proxy forwarding, and lease renewal—was not exercised end-to-end.
- The proxy test validates URL/header helper extraction, not an observed upstream HTTP request.
- Rapid session switching while projection and canvas sources are simultaneously connecting lacks a browser-level race test.
- WASM validation could not run because `wasm32-unknown-unknown` is not installed; native compilation/tests passed.