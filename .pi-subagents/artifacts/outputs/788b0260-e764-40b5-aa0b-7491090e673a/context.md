# Code Context

## Files Retrieved

1. `AGENTS.md` (root contract supplied in run context) - repository-wide ownership, verification, deployment, and devlog rules.
2. `crates/AGENTS.md` (lines 1-109) - canonical 26-package ownership/entry/test map and cross-crate fanout rules.
3. `HANDOFF.md` (lines 1-25) - confirms this is only an evergreen routing file; live state must come from Git and daemon health.
4. `ROADMAP.md` (lines 1-45) - current open integration, reliability, structure, and platform gaps.
5. `docs/DAEMON_REFACTOR_MISSION.md` (lines 1-112) - active behavior-neutral daemon extraction mission, completed Phase 2C leaves, invariants, and next domains.
6. `docs/ARCHITECTURE.md` (lines 1-160) - current client → daemon → agent/runtime/provider flow, authority boundaries, session flow, retention, and active client/domain inventory.
7. `crates/ocean-daemon/AGENTS.md` (lines 1-79) - daemon route, cwd, permission, voice, history-search, request-control, replay, and validation contracts.
8. `crates/ocean-runtime/AGENTS.md` (lines 1-75) - runtime tool/permission/cancellation contracts and newly documented session-scoped todo behavior.
9. `crates/ocean-tui/AGENTS.md` (lines 1-193) - TUI hard rules and the current interaction/rendering contract, including rails, todo tray, tool bursts, Markdown links, permissions, and launch behavior.
10. `crates/ocean-daemon/src/history_search.rs` (lines 1-120, untracked) - new bounded daemon adapter for persisted transcript search.
11. `events.md` (working-tree diff around lines 3274 onward) - dirty handoff entries for transcript search, context-meter dithering, collapsed tool bursts/todo titles, and stable transcript selection.

## Key Code

### Registered worktrees and Git relationships

`git worktree list --porcelain` reports exactly two registered worktrees:

| Worktree | Branch / HEAD | Upstream | Relative to `main` | State |
|---|---|---|---|---|
| `/Users/smathdaddy-macbook/ocean-os` | `main` at `827b65b` | `origin/main` at the same commit | `0 behind / 0 ahead` | 25 modified tracked files plus 1 untracked file; nothing staged |
| `/Users/smathdaddy-macbook/ocean-os/.claude/worktrees/offshore` | `feat/offshore` at `c9f9fb9` | `origin/feat/offshore` is gone | `147 behind / 1 ahead` | 5 modified tracked files; nothing staged or untracked |

Main's latest commit is `827b65b feat(tui): reconcile rails permissions and session tray`; it exactly matches `origin/main`. Immediately preceding work includes merged request-control extraction (`133a18b`, `87c3599`, `cdc3fb3`) and model-role extraction (`014226e`, `5587854`, `6d1ea01`). Voice planner hardening is also in recent main history.

The offshore branch fork/merge base is `913edac` (`docs: events — realtime voice daemon slice`). Its only branch-unique commit is `c9f9fb9 feat(offshore): native remote-dispatch tool family + /offshore TUI toggle`. A semantically equivalent offshore commit, `ab64d56`, is already in main history (shown by the main-side divergence log), so this worktree is a stale pre-merge survivor rather than a clean unintegrated feature branch.

### Main worktree: committed versus dirty

**Committed:** main and origin/main are synchronized at `827b65b`. The current committed line includes TUI rail/permission/session-tray reconciliation, daemon request-control and model-role leaf extraction, component interaction, Slack Canvas fulfillment, project/filesystem/settings/catalog/workspace/event-adapter daemon leaves, extension Phase 0/Phase 1 work, Herdr projection, and voice planner safety.

**Dirty only (not staged or committed):** tracked diff is 24 files, `+2,235/-230`, plus the 120-line untracked `crates/ocean-daemon/src/history_search.rs`. The status actually names 25 modified tracked files because `events.md` and all listed docs/sources are included; the diff-stat summary reported 24 files and excludes the untracked file. Dirty themes are:

- **Transcript history search:** `ocean-agent` adds bounded deterministic persisted display-transcript search and tests; daemon adds `GET /v1/agent/history/search`, route parity/docs, and the untracked adapter module. The adapter limits concurrent blocking scans to two, rejects empty/oversized queries, uses `spawn_blocking`, clamps result limits in the agent owner, and makes no provider/embedding call.
- **TUI interaction tranche:** collapsed consecutive tool bursts with parent running/done/failed summaries and nested drawers; safe workspace-local Markdown document links; stable transcript-row selection; clipboard/multi-image handling; editor adjustments; late advisor suppression; context-meter dithering; session tray refinements.
- **Todo contract:** runtime todo items gain optional concise titles while authoritative text remains intact; state is session-scoped and softly bounded. TUI compact tray prefers title.
- **Render/component guidance:** component prompt/docs and render protocol are being tightened; `component.rs` is touched.
- **Docs/devlog:** contracts and guides were updated in the dirty tree, including events dated July 18 despite the run date being July 15. Treat these as uncommitted claims, not landed history.

The dirty main tree is broad and cross-cuts `ocean-agent`, `ocean-daemon`, `ocean-runtime`, and `ocean-tui`; it should not be mistaken for the published `827b65b` state. No validation was run during this reconnaissance. The dirty `events.md` claims prior focused/full tests and checks, but those claims were not independently rerun here.

### Offshore worktree: committed versus dirty

**Committed branch-only commit:** `c9f9fb9` adds 1,921 lines across 10 files. It introduces ten `offshore_*` remote-dispatch tools, `[offshore]` config gating, per-job remote Git worktrees, SSH/git transport, and a persistent `/offshore [on|off|status]` TUI toggle/guidance path. The commit message claims 26 offshore unit tests plus config/registry/TUI tests.

**Dirty only:** 5 tracked files, `+94/-32`, no staged/untracked files:

- `crates/ocean-agent/src/project.rs`
- `crates/ocean-runtime/src/artifacts.rs`
- `crates/ocean-runtime/src/tools/component.rs`
- `crates/ocean-runtime/tests/artifact_spill_wiring.rs`
- `crates/ocean-runtime/tests/component_lifecycle.rs`

Inspection shows these changes are overwhelmingly rustfmt-style line wrapping, including the test files and component tests; no clear new feature was established from the sampled diff. They remain uncommitted and should be reviewed before discard or salvage.

### Integration dependencies and likely conflicts

- Do **not** merge `feat/offshore` wholesale: it is 147 commits behind, its upstream is deleted, and main already contains the equivalent merged offshore feature (`ab64d56`). First compare `c9f9fb9` against `ab64d56`; only salvage proven deltas.
- Offshore's committed patch touches `crates/ocean-agent/src/lib.rs` and `crates/ocean-tui/src/shell/app.rs`, both heavily modified in dirty main. A direct cherry-pick/rebase would likely conflict there and risk reverting newer session/TUI behavior.
- Both worktrees have dirty edits to `crates/ocean-runtime/src/tools/component.rs`; this is the direct dirty-overlap conflict. Offshore appears formatting-only here, whereas main includes current render/component behavior, so main should be authoritative unless a semantic diff proves otherwise.
- The history-search tranche spans agent persistence, daemon HTTP/router parity, operator docs, and TUI session discovery. It requires daemon + agent tests, route parity, workspace test compilation, and likely TUI validation before landing.
- The TUI tranche is internally coupled: grouped tool geometry, scrolling/selection, Markdown hit cells, drawer focus, and mouse drag precedence share `chat.rs`/`app.rs`. Splitting commits without preserving those invariants risks visual or click-routing regressions.
- Todo changes couple runtime session state, tool result shape, prompt guidance, and the TUI tray. Preserve backward compatibility for callers omitting `title`.

## Architecture

Ocean clients (TUI/CLI/ACP/Surface) call the daemon over HTTP and session-scoped SSE. `ocean-daemon` is the only first-party HTTP/SSE and turn-orchestration authority; `ocean-agent` owns sessions, persisted history, prompts, and capability assembly; `ocean-runtime` owns provider rounds, tools, permissions, cancellation, and runtime events; providers/protocol own routing and wire behavior. Clients render and steer but do not own sessions or execute tools.

The active daemon theme is a behavior-neutral decomposition of the ~20k-line `main.rs` into private cohesive leaves while preserving 72-route parity (dirty architecture text says 75 after history additions), cwd, permissions, SSE retention, and Axum fallback/layer behavior. Completed recent leaves include request control and model roles; persistent rooms, Longhouse, calls, and remaining registries are next, with agent-turn/SSE orchestration explicitly last.

The most important current runtime/daemon/TUI themes are:

1. **Daemon decomposition without contract redesign** — private leaf modules, characterization first, exact route/doc parity.
2. **Permission and voice safety** — daemon-backed three-state permissions; token-bound approvals; voice turns no-tools; planner mode propose-only.
3. **Session/history authority** — new dirty fuzzy transcript search remains in agent persistence with daemon as bounded adapter.
4. **TUI truthfulness and geometry** — session tray, todo projection, collapsed tool bursts, stable selection coordinates, safe Markdown links, terminal sanitization, and Unicode-cell correctness.
5. **Runtime boundedness** — replay is bounded, but live per-turn MPSC remains an open design issue; large results need artifact backing before payload limits rise.
6. **Extension ownership** — subagent dispatch/orchestration remains extension-owned; core must not grow fleet/task scheduling.
7. **Open cross-repo mismatch** — `surface-tauri` still lacks an authored `ocean-agent::surface_flag` mapping.

## Start Here

Open `git status --short --branch` in `/Users/smathdaddy-macbook/ocean-os`, then read `crates/ocean-tui/src/shell/components/chat.rs`. It is the largest dirty locus and connects tool grouping, Markdown links, stable selection, scrolling, and mouse routing. Before integrating anything, isolate the history-search and todo/TUI tranches into reviewable units and compare offshore's `c9f9fb9` against already-landed `ab64d56` rather than rebasing the stale worktree blindly.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Performed read-only reconnaissance of every worktree reported by git worktree list; repository files, index, refs, and worktrees were not modified. Only the required external report artifact was written."
    },
    {
      "id": "criterion-2",
      "status": "satisfied",
      "evidence": "Report includes exact worktree paths, branch/HEAD/upstream relationships, divergence counts, recent commit identities, staged/dirty/untracked states, diff statistics, retrieved docs, integration risks, and command evidence."
    }
  ],
  "changedFiles": [
    "/Users/smathdaddy-macbook/ocean-surface/.pi-subagents/artifacts/outputs/788b0260-e764-40b5-aa0b-7491090e673a/context.md"
  ],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    {
      "command": "git worktree list --porcelain; git log --oneline --decorate -12",
      "result": "passed",
      "summary": "Found exactly main and offshore worktrees; captured current main history."
    },
    {
      "command": "git -C <each-worktree> status --short --branch; git diff --cached --stat; git diff --stat; git log; git rev-list --left-right --count main...HEAD",
      "result": "passed",
      "summary": "Captured branch/upstream, staged, dirty, recent commit, and divergence state for both worktrees."
    },
    {
      "command": "git show/diff-tree/merge-base/branch --contains for feat/offshore",
      "result": "passed",
      "summary": "Established offshore's unique commit, merge base, touched files, stale upstream, and branch containment."
    },
    {
      "command": "git diff --numstat; comm overlap checks; selective git diff inspection",
      "result": "passed",
      "summary": "Quantified dirty changes and identified committed and dirty overlap/conflict paths."
    },
    {
      "command": "read/grep/find inspection of AGENTS, HANDOFF, ROADMAP, architecture, daemon mission, events, and dirty source",
      "result": "passed",
      "summary": "Mapped current ownership, plans, invariants, and dirty feature intent without changing repository state."
    }
  ],
  "validationOutput": [
    "main == origin/main at 827b65b; divergence 0/0; no staged files.",
    "feat/offshore at c9f9fb9; origin branch gone; 147 behind and 1 ahead of main; no staged files.",
    "Main tracked diff: 24-file diff-stat, +2235/-230, plus one 120-line untracked daemon module; status lists 25 modified tracked paths.",
    "Offshore dirty diff: 5 files, +94/-32; no untracked files.",
    "No tests or builds were run because the task was read-only reconnaissance."
  ],
  "residualRisks": [
    "Dirty working-tree claims in events.md were not independently validated.",
    "Offshore dirty changes were selectively inspected and appear formatting-heavy, but a full semantic diff review is still required before discard.",
    "The required report is outside ocean-os; no post-write Git status was needed for ocean-os, and all inspected staged-file lists were empty."
  ],
  "noStagedFiles": true,
  "diffSummary": "No ocean-os repository changes made; wrote only this reconnaissance artifact. Existing main and offshore dirty states are documented above.",
  "reviewFindings": [
    "blocker: main worktree contains a broad unstaged cross-crate tranche plus an untracked daemon source file; do not integrate or deploy as though 827b65b were a clean checkout.",
    "blocker: feat/offshore is stale, upstream-gone, and functionally already merged on main; do not merge wholesale.",
    "warning: events.md contains uncommitted entries dated after the stated current date; treat them as working notes until reconciled.",
    "review gate remains required by the acceptance contract."
  ],
  "manualNotes": "Repository reconnaissance was intentionally inspection-only. Runtime daemon health was not queried because the requested scope centered on repository/worktree state and no deployment validation was requested."
}
```
