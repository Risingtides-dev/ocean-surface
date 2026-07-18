# Code Context

## Files Retrieved

1. `crates/ocean-core/src/lib.rs` (lines 714-764) - canonical three-mode permission wire DTOs.
2. `crates/ocean-daemon/src/main.rs` (lines 592-641, 11737-11749, 12592-12680, 19053-19120) - route registration and permission/history route tests.
3. `crates/ocean-daemon/src/history_search.rs` (lines 1-158) - bounded daemon adapter, response DTO, error/status behavior, capacity test.
4. `crates/ocean-agent/src/lib.rs` (lines 99-128) - canonical `HistoryMatchKind` and `HistorySearchHit` wire shape.
5. `crates/ocean-agent/src/session/mod.rs` (lines 510-655, 1058-1210) - bounded persisted-transcript search, ranking/filtering, and tests.
6. `crates/ocean-runtime/src/tools/component.rs` (lines 35-140) - `interactive_plot` allowlist, selection guidance, schema, limits, and commit-event guidance.
7. `docs/OCEAN_SURFACE_COMPONENT_PROMPT_GUIDE.md` (lines 62-110, 288-292) - agent-facing interactive plot usage guidance.
8. `docs/AGENT_RENDER_PROTOCOL.md` (lines 179-240, 348-352) - protocol example and supported-kind documentation.
9. `crates/ocean-runtime/src/capability.rs` (lines 32-79, 172-270, 614-850) - session-keyed `TodoTool`, empty-only soft eviction, and coverage.
10. `/Users/smathdaddy-macbook/ocean-surface/handoff.md` (lines 1-300) - prerequisite claims and dirty-client integration hazards.
11. `/Users/smathdaddy-macbook/ocean-surface/crates/ocean-surface-ui/src/daemon.rs` (lines 823-857, 2753-2817, 5091-5145) - dirty Recall DTO/client/generation guards and tests.
12. `/Users/smathdaddy-macbook/ocean-surface/crates/ocean-surface-ui/src/components.rs` (lines 1-85) and `src/components/interactive_plot.rs` (dirty/untracked) - dirty client registration and implementation.

## Key Code

### Audit result: all four OS prerequisites are landed on current `main`

Current Ocean OS is `4712fdb` and matches `origin/main`. The relevant commits are ancestors of HEAD:

- `827b65b feat(tui): reconcile rails permissions and session tray`
  - lands `PermissionMode::{Manual, Automatic, SkipAll}`, default `Automatic`, settings request/response DTOs, daemon settings behavior/tests, and session-scoped todo storage/tests.
- `fae71b0 feat: reconcile agent runtime and TUI improvements`
  - lands persisted history search, daemon route, `interactive_plot` runtime/tool guidance, and protocol docs.
- `4b2d7ea fix: close runtime and TUI reconciliation review`
  - hardens history search with the 64 MiB store budget/capacity error mapping and additional review tests.

`git blame` directly attributes permission DTOs and todo cache to `827b65b`, and the initial history adapter plus interactive plot guidance to `fae71b0`; capacity review changes are attributed to `4b2d7ea`.

### Permission modes

Canonical wire contract (`crates/ocean-core/src/lib.rs:724-764`):

```rust
#[serde(rename_all = "snake_case")]
pub enum PermissionMode { Manual, Automatic, SkipAll }

pub struct PermissionSettingsResponse {
    pub ok: bool,
    pub error: Option<String>,
    pub persisted: Option<PermissionMode>,
    pub effective: PermissionMode,
    pub env_override: Option<PermissionMode>,
}
```

Daemon registers GET/POST `/v1/settings/permissions` and retains legacy YOLO compatibility. Tests cover all three round trips, default/effective behavior, `OCEAN_YOLO=1` forcing `skip_all`, `OCEAN_YOLO=0` preventing saved skip-all without collapsing manual to automatic, persistence failure, and the runtime gating boundary (`main.rs:11737-11749`, `12592-12680`).

### History route/DTO behavior

`GET /v1/agent/history/search?q=...&limit=...` returns:

```json
{"ok":true,"query":"...","hits":[...],"error":null}
```

Hit fields are `hit_id`, `session_id`, `session_title`, `role`, `excerpt`, optional `timestamp_ms`, always-present nullable `workspace_root`, numeric `score`, and lowercase `match_kind` (`exact|lexical|fuzzy`). Search reads only persisted display projections, filters to non-empty user/assistant text, deterministically ranks exact before lexical before fuzzy, clamps/bounds query/results, excludes tool payloads, runs in `spawn_blocking`, limits concurrent scans to two, and rejects over-capacity stores with 503. Route tests include limit=0, missing/oversized query, and adapter behavior (`main.rs:19053-19120`); agent tests cover capacity, display-only filtering, deterministic fuzzy ranking, Unicode and limits (`session/mod.rs:1058-1210`).

### `interactive_plot` guidance

The landed runtime allowlist and schema require lowercase ASCII ids and bound parameters/series/metrics/samples/expression size to 12/6/12/512/512. Guidance says local recomputation and `parameters_changed` only on commit. Protocol and Surface prompt guide are landed. This is guidance/protocol authority only; actual Leptos renderer remains Surface-owned.

### Session-scoped todo

`BuiltinProvider` retains a `TodoTool` per authoritative bound `SessionContext.session_id`; repeated turns share it, other sessions are isolated, and unbound/ad-hoc turns receive fresh tools. The 1,024-entry soft target evicts only least-recently-touched empty tools; non-empty state is pinned and temporary overgrowth is reclaimed after entries clear. Tests at `capability.rs:614-850` cover cross-turn retention/isolation, LRU empty eviction, non-empty survival, overgrowth/shrink, and cleared-entry eviction.

## Architecture

Surface should deserialize daemon/core wire contracts, not recreate authority. The daemon delegates Recall to `ocean-agent` persisted session storage. Runtime capability assembly receives the daemon-resolved session id and rebinds the stateful todo tool to that key. Runtime/system prompt documentation advertises `interactive_plot`; Surface performs bounded local rendering and sends committed component events.

## Dirty Surface mismatch/risk audit

- **Permission settings are not integrated in the dirty Surface client.** Search found no `PermissionMode`, `PermissionSettingsResponse`, or `/v1/settings/permissions` consumer in `ocean-surface-ui/src`. The dirty code does consume `/v1/permissions` for pending-request attention, which is a different endpoint. Tauri settings UI must add the exact snake_case DTO and GET/POST settings route; it must distinguish `persisted`, `effective`, and `env_override` rather than treating pending permissions or legacy YOLO as the mode.
- **Recall is wire-compatible.** Dirty Surface `HistorySearchHit` uses `String` for `match_kind` rather than an enum, but lowercase daemon values decode safely. It makes response `query` absent from its local DTO; Serde ignores the daemon's extra field, so this is compatible. `workspace_root` and `timestamp_ms` optionality match. Generation guards correctly prevent stale replacements. It fixes `limit=20`; daemon accepts that.
- **Recall error handling has a small diagnostics gap.** The dirty client treats transport success based on body `ok` and ignores HTTP status itself. It still displays daemon `error`, including 400/429/503 payloads, so behavior works, but status-specific UX/retry is unavailable.
- **`interactive_plot` appears aligned but is not landed in Surface.** `components.rs` registers the kind and the implementation is an untracked file. Therefore current Surface `HEAD` does not contain it and a merge/rebase can omit it. Verify its emitted payload includes the complete parameter map plus changed id/value and remains commit-only before landing.
- **Surface is behind upstream by five commits and heavily dirty.** It is at `8459c2c`, with overlapping dirty `app.rs`/`daemon.rs` and untracked modules, while upstream Voice Planner changes touch the same files. Do not mechanically pull/rebase. This audit intentionally ignored unrelated Longhouse/Rooms changes.
- **Ocean OS worktree is also dirty**, but the prerequisite files audited above are committed. Current unrelated modifications are `crates/ocean-daemon/src/main.rs`, `crates/ocean-providers/src/lib.rs`, and `crates/ocean-tui/src/shell/app.rs`; none alter the committed prerequisite conclusions in the inspected diffs.

## Start Here

Open `/Users/smathdaddy-macbook/ocean-surface/crates/ocean-surface-ui/src/daemon.rs` first. Recall already lives there, and the missing permission-settings DTO/client should be added against the canonical contract in `crates/ocean-core/src/lib.rs:724-764` without conflating it with `/v1/permissions`.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Completed a read-only prerequisite audit limited to permission modes, Recall history search, interactive_plot guidance, and session-scoped todo; unrelated Longhouse/Rooms work was excluded."
    },
    {
      "id": "criterion-2",
      "status": "satisfied",
      "evidence": "Recorded exact commits, files/line ranges, tests, DTO behavior, dirty Surface mismatches, repository state, and residual integration risks."
    }
  ],
  "changedFiles": [
    "/Users/smathdaddy-macbook/ocean-surface/.pi-subagents/artifacts/outputs/ccdc45fb-c56e-456c-adf7-31e285b84062/context.md"
  ],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    {
      "command": "git status --short --branch && git log -20 --oneline --decorate",
      "result": "passed",
      "summary": "Confirmed ocean-os main at 4712fdb equals origin/main and identified three pre-existing dirty files."
    },
    {
      "command": "git log/blame/show plus targeted grep/read across ocean-core, ocean-daemon, ocean-agent, ocean-runtime and dirty ocean-surface",
      "result": "passed",
      "summary": "Attributed prerequisites to 827b65b, fae71b0, and 4b2d7ea and compared the dirty client contract."
    },
    {
      "command": "git diff --cached --stat",
      "result": "passed",
      "summary": "Produced no output; no staged files in ocean-os."
    }
  ],
  "validationOutput": [
    "All four requested Ocean OS prerequisites are committed ancestors of current main.",
    "Dirty Surface Recall DTO is compatible; permission-mode settings consumption is missing; interactive_plot remains dirty/untracked Surface work.",
    "No tests were executed because this was a read-only prerequisite audit; committed test locations and coverage were inspected."
  ],
  "residualRisks": [
    "Independent reviewer gate remains required.",
    "Surface is five commits behind upstream with overlapping dirty files; integration must use a controlled clean lane.",
    "The untracked Surface interactive_plot implementation can be omitted accidentally and needs payload/commit-event validation.",
    "Ocean OS contains unrelated unstaged modifications, although inspected prerequisite code is committed."
  ],
  "noStagedFiles": true,
  "diffSummary": "No project/source files modified; added only this requested audit artifact.",
  "reviewFindings": [
    "blocker: ocean-surface-ui/src - no client for GET/POST /v1/settings/permissions, so Tauri cannot yet render or persist the landed three-mode policy",
    "warning: ocean-surface-ui/src/components/interactive_plot.rs - implementation is untracked and therefore absent from Surface HEAD",
    "no blocker: dirty Recall DTO and route usage are compatible with current daemon history-search wire behavior"
  ],
  "manualNotes": "Reviewed acceptance level requested; final acceptance still requires the designated independent reviewer."
}
```
