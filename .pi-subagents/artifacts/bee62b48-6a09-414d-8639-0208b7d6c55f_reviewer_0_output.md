## Review

- **High — Activity rows can retain stale state and details across polls.** `crates/ocean-surface-ui/src/island.rs:856-873` keys `<For>` rows only by `item.id`, then captures `item_class`, `state_label`, `session_title`, `summary_detail`, and detail fields as non-reactive values. Leptos keyed iteration retains an existing row when its key is unchanged; it does not rerun the child closure for updated data. A request therefore can remain visibly `Running` after the same request ID changes to `Cancelling` or `Errored`, and updated messages/session metadata can remain stale. This breaks the Phase 2A requirement that the polling view truthfully project authoritative state (`docs/OCEAN_DYNAMIC_ISLAND_IMPLEMENTATION.md:69-78,100-108`). The chip counts are separately reactive, so the chip can say attention is needed while the corresponding retained card still says Running.

- **Medium — The composer IME guard does not cover slash-menu keyboard handling.** In `crates/ocean-surface-ui/src/app.rs:1464-1534`, slash-menu ArrowUp/ArrowDown and Enter/Tab handling executes before any `is_composing()` check. The new guard is only applied to ordinary Enter submission at `app.rs:1535-1543`. If composition text begins with `/` and matches a slash command, Enter used to accept an IME candidate can execute the selected command and clear the input (`app.rs:1493-1518`); composition arrow keys are also intercepted. The composition guard needs to precede the slash-menu key switch (while preserving the intended non-composing behavior).

### Verified correct

- `crates/ocean-surface-ui/src/daemon.rs:767-817,2625-2647` matches the current daemon wire types for `GET /v1/requests` and `GET /v1/permissions`, ignores raw permission `args`, and only replaces each signal after a decoded `ok: true` response.
- `crates/ocean-surface-ui/src/island.rs:455-463,476-480,531-543` implements the specified 3-second mounted polling plus immediate mount/open refresh. The current in-flight guard at `daemon.rs:2602-2610` prevents overlapping polls.
- `crates/ocean-surface-ui/src/island.rs:190-203,226-232` implements the required projection and ordering: pending permission de-duplication, queued/running, cancelling, newest three errors, and omission of completed/cancelled requests.
- The Phase 2A UI remains read-only apart from the approved focus action: the only card action is `Open session` via `Daemon::switch_session` at `island.rs:939-960`; no approve, deny, reply, resume, or stop endpoint is called.
- Explicit EventSource retirement closes both focused-session transports before reconnecting (`daemon.rs:1775-1785`) and stores/retire-closes the newly opened agent and permission sources (`daemon.rs:1864-1870,2023-2029`). No concrete cancellation race was found in inspection.
- The dialog traps Tab among the input, disclosure summaries, and enabled Open buttons (`island.rs:597-641`), scopes Arrow/Enter session navigation to the search input (`island.rs:644-695`), and stops composing/Escape handling appropriately inside the Island (`island.rs:586-595`).

### Validation notes

- Native Rust tests pass, but the WASM check could not run because `wasm32-unknown-unknown` is not installed. Consequently, the new `#[cfg(target_arch = "wasm32")]` EventSource handle code and browser DOM behavior were not compiler/runtime validated in this environment.
- The requested repository-root `plan.md` and `progress.md` do not exist. Review used `docs/OCEAN_DYNAMIC_ISLAND_BUILD_PLAN.md` and `docs/OCEAN_DYNAMIC_ISLAND_IMPLEMENTATION.md` instead.
- No project/source files were modified by this review.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "not_satisfied",
      "evidence": "The Phase 2A implementation stays within the approved read-only endpoint/action scope, but island.rs:856-873 can retain stale keyed card content and app.rs:1464-1534 still permits IME keystrokes to execute slash commands. Review gate should remain closed until corrected."
    },
    {
      "id": "criterion-2",
      "status": "satisfied",
      "evidence": "Inspected the current diff, exact daemon wire definitions in ../ocean-os, Leptos keyed-list behavior, focused source/docs/CSS, tests, formatting, staging state, and build limitations; findings include file/line evidence."
    }
  ],
  "changedFiles": [
    "crates/ocean-surface-ui/src/app.rs",
    "crates/ocean-surface-ui/src/daemon.rs",
    "crates/ocean-surface-ui/src/main.rs",
    "crates/ocean-surface-ui/src/palette.rs",
    "crates/ocean-surface-ui/src/sessions.rs",
    "crates/ocean-surface-ui/tests/voice_realtime_regressions.rs",
    "crates/ocean-tauri/src/lib.rs",
    "docs/OCEAN_DESKTOP_NORTH_STAR.md",
    "docs/OCEAN_WEB_SURFACE_DESIGN.md",
    "extension/sidepanel.html",
    "index.html",
    "scripts/build-extension.sh",
    "styles/compact.css",
    "styles/composer.css",
    ".pi-subagents/ (untracked directory)",
    "crates/ocean-surface-ui/src/island.rs",
    "crates/ocean-surface-ui/src/search.rs",
    "docs/OCEAN_DYNAMIC_ISLAND_BUILD_PLAN.md",
    "docs/OCEAN_DYNAMIC_ISLAND_IMPLEMENTATION.md",
    "styles/island.css"
  ],
  "testsAddedOrUpdated": [
    "crates/ocean-surface-ui/src/island.rs",
    "crates/ocean-surface-ui/src/daemon.rs",
    "crates/ocean-surface-ui/src/app.rs",
    "crates/ocean-surface-ui/tests/voice_realtime_regressions.rs"
  ],
  "commandsRun": [
    {
      "command": "cargo test -p ocean-surface-ui",
      "result": "passed",
      "summary": "323 unit tests and 1 integration regression test passed."
    },
    {
      "command": "cargo fmt --all -- --check",
      "result": "passed",
      "summary": "Formatting check produced no output."
    },
    {
      "command": "git diff --check",
      "result": "passed",
      "summary": "No whitespace errors."
    },
    {
      "command": "cargo check -p ocean-surface-ui --target wasm32-unknown-unknown",
      "result": "blocked",
      "summary": "Environment lacks the wasm32-unknown-unknown target; WASM-only code was not compiled."
    },
    {
      "command": "git status --porcelain=v1 && git diff --cached --name-only",
      "result": "passed",
      "summary": "Captured all modified/untracked paths and confirmed no staged files."
    }
  ],
  "validationOutput": [
    "cargo test: 323 passed, 0 failed; integration regression: 1 passed, 0 failed",
    "cargo fmt --check: clean",
    "git diff --check: clean",
    "Daemon contract verified against ../ocean-os/crates/ocean-core/src/lib.rs RequestStatus/RequestsResponse and PermissionStatus/PermissionsResponse",
    "No approve/deny/reply/stop action found in the Island; Open session is the sole attention-card action"
  ],
  "residualRisks": [
    "WASM-only EventSource cancellation code was not compiled because the target is unavailable.",
    "No browser/Tauri manual run was performed, so focus restoration, live announcements, layout, and actual transport closure remain unverified at runtime.",
    "Current pure tests do not exercise keyed DOM reconciliation, which is why the stale-card defect passes the suite."
  ],
  "noStagedFiles": true,
  "diffSummary": "Uncommitted tree contains Dynamic Island/session-search work, Phase 2A read-only request/permission polling and cards, explicit focused-session EventSource cancellation, composer/IME and palette changes, styling/docs, plus other Tauri/composer changes.",
  "reviewFindings": [
    "high: crates/ocean-surface-ui/src/island.rs:856-873 - keyed rows capture non-reactive item data, so same-ID request state/detail changes can remain stale",
    "medium: crates/ocean-surface-ui/src/app.rs:1464-1534 - slash-menu key handling runs before the IME guard and can execute a command on composition Enter"
  ],
  "manualNotes": "Review gate: not approved until both concrete findings are fixed and WASM/Tauri validation is completed. Repository-root plan.md and progress.md were absent; the checked-in Dynamic Island plan/spec documents were reviewed instead."
}
```
