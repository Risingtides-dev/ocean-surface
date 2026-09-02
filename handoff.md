# Ocean Surface — fresh-session handoff

**Rooms, as of 2026-09-01:** this snapshot predates every rooms feature. For
the rooms surface read `docs/OCEAN_ROOMS_PRODUCT.md` (each section carries a
dated status line where the code lags the contract) and the cross-repo finish
line at `ocean-os/docs/specs/2026-09-01-ocean-rooms-definition-of-done.md`.
The dirty-cluster and worktree sections below describe July checkouts and are
not current state; derive it from the commands under "Start here".

**Snapshot:** 2026-07-15, after the daemon-owned permission workflow, corrected
Dynamic Island, daemon Recall, locally reactive `interactive_plot`, and the
transcript/composer pending-state repair.

This exact root file is the **only active state handoff**. Do not discover
handoffs by filename or read archived/test-fixture/spec handoffs as current
state. In particular, ignore `.agentignore/handoff-archived-2026-07-10-1.md`.
The sibling `../ocean-os/HANDOFF.md` is an evergreen routing index, not a second
state snapshot.

This handoff covers work spanning:

- `/Users/smathdaddy-macbook/ocean-surface`
- `/Users/smathdaddy-macbook/ocean-os`

Trust commands over snapshot values. Both repositories are shared, dirty
workspaces with concurrent work. Re-read files immediately before editing and
never reset, clean, stage, commit, rebase, prune worktrees, or overwrite broad
shared files without first identifying ownership and current diffs.

## Start here

```sh
cd /Users/smathdaddy-macbook/ocean-surface
git fetch origin --prune
git status --short --branch
git log -8 --oneline --decorate origin/main
git worktree list --porcelain

git -C ../ocean-os fetch origin --prune
git -C ../ocean-os status --short --branch
git -C ../ocean-os log -8 --oneline --decorate origin/main
git -C ../ocean-os worktree list --porcelain

curl -fsS http://127.0.0.1:4780/health
curl -fsS http://127.0.0.1:4780/v1/settings/permissions
```

Read in this order:

1. `AGENTS.md`
2. this file
3. `docs/OCEAN_DYNAMIC_ISLAND_IMPLEMENTATION.md`
4. `docs/OCEAN_DYNAMIC_ISLAND_BUILD_PLAN.md`
5. `docs/OCEAN_DESKTOP_NORTH_STAR.md`
6. `../ocean-os/AGENTS.md`
7. `../ocean-os/crates/{ocean-agent,ocean-daemon,ocean-runtime,ocean-tui}/AGENTS.md`
8. `../ocean-os/docs/DAEMON_REFACTOR_MISSION.md`
9. the current Git diffs and recent `events.md` tails

The local Surface checkout is behind upstream, so also inspect:

```sh
git diff main..origin/main -- AGENTS.md
git log --name-status --oneline main..origin/main
```

The latest upstream `AGENTS.md` adds the safe Surface launch contract and the
confirmed Voice Planner exception to lazy session creation.

## Product and authority model

Ocean is two repositories, one system:

```text
Ocean Surface (Leptos/WASM UI + thin hosts)
  -> ocean-daemon HTTP/SSE authority
  -> ocean-agent session/history/prompt authority
  -> ocean-runtime permission/tool/execution authority
```

Binding rules:

- `crates/ocean-surface-ui` is the canonical UI for PWA, Tauri, and Chrome
  extension. Tauri remains a thin host.
- `src/host.rs` is the only platform-capability seam.
- Surface code renders daemon state and collects intent. It does not own provider
  calls, reasoning, permissions, session persistence, transcript indexing, or
  tool execution.
- Sessions are explicitly scoped: `Surface -> Session`. Never adopt a session
  from the global event stream or blend activity from another surface.
- `client_type` describes the medium, not session identity.
- Shared `app.rs` changes must be surgical. Never reference an uncommitted module
  from a shared file unless that module and integration compile together.
- Colors live only in `styles/tokens.css`. New stylesheets must be enumerated in
  `index.html`, `extension/sidepanel.html`, and
  `scripts/build-extension.sh`.
- Control density is a defect. Agent, Sessions, Recall, and Actions remain
  distinct intents.

## Current Surface Git state

At snapshot time:

```text
local main:  8459c2c  fix(deploy): reclaim stale watcher locks
origin/main: b09db43  current Voice Planner + launcher integration
relationship: local main is behind origin/main by 5 commits
```

Before this handoff rewrite the checkout had 19 modified tracked paths and nine
untracked roots; `handoff.md` is now one additional intentional tracked
modification. Nothing is staged.

### Critical upstream integration hazard

Upstream added the confirmed, propose-only Voice Planner in five commits:

- safe clean-checkout Surface launcher contract;
- isolated Realtime planner mode;
- confirmed `Create draft` / `Create & start` workflow;
- current clippy fixes;
- integration merge.

It adds `crates/ocean-surface-ui/src/voice/planner.rs` and heavily changes:

- `crates/ocean-surface-ui/src/app.rs`
- `crates/ocean-surface-ui/src/daemon.rs`
- `crates/ocean-surface-ui/src/voice/mod.rs`
- `crates/ocean-surface-ui/src/voice/realtime.rs`
- `styles/compact.css`
- `styles/composer.css`

The current dirty Surface work overlaps upstream in exactly these high-risk
paths:

- `app.rs`
- `daemon.rs`
- `styles/compact.css`
- `styles/composer.css`

Do **not** pull/rebase/merge into this dirty checkout mechanically. Preserve both
Voice Planner and the current Island/component work in a clean integration lane,
with one writer for the shared files and detached-worktree validation before any
landing.

### Current Surface dirty feature clusters

Tracked and untracked WIP includes:

1. **Corrected Dynamic Island**
   - `IslandMode::{Closed, Agent, Sessions, Recall}`.
   - Compact click opens Agent; `Cmd/Ctrl+P` opens Sessions;
     `Cmd/Ctrl+Shift+F` opens Recall; `Cmd/Ctrl+K` remains Actions.
   - Agent, Sessions, and Recall replace one another; they never form one
     dashboard.
   - Agent renders one authoritative work object at a time with direct
     Approve/Deny/Stop/Open Session actions.
   - Permission decisions require global permission identity, focused session,
     focused SSE permission state, and the submitter-owned decision token.
   - Files: `island.rs`, `island_dynamic.rs`, `search.rs`, `styles/island.css`,
     `app.rs`, `daemon.rs`, Tauri host wiring, and the two Dynamic Island docs.

2. **Daemon-owned Recall**
   - Surface calls `GET /v1/agent/history/search`.
   - Results are persisted display transcript text only, user/assistant only.
   - Ranking is truthfully `exact -> lexical -> fuzzy`; semantic/embedding
     fusion remains deferred to daemon/Bedrock authority.
   - Generation guards prevent stale response replacement.

3. **Reusable `interactive_plot` component**
   - Local Leptos/WASM parameter controls recompute curves and metrics without a
     model round trip.
   - Only committed changes emit `parameters_changed` with the complete
     parameter map and changed id/value.
   - Bounded Rust expression DSL; never JS `eval`, arbitrary scripts, provider
     calls, network authority, or runtime execution.
   - Limits: 12 parameters, 6 series, 12 metrics, 512 samples, 512 expression
     characters.
   - `chart` remains display-only. Playback/animated `simulation` is later.
   - Primary file:
     `crates/ocean-surface-ui/src/components/interactive_plot.rs`.

4. **Tauri titlebar and host integration**
   - Tauri-only Dynamic Island mount and shortcut gating.
   - Browser/PWA/extension Print/Find behavior stays unchanged.
   - Native diagnostic hooks exist in `crates/ocean-tauri/src/lib.rs`; do not
     expand them into runtime authority.

5. **Transcript/composer pending-state repair**
   - `.transcript` now explicitly owns remaining column height with
     `flex: 1 1 0; min-height: 0`.
   - `.ocean-composer` stays bottom-docked with `flex: 0 0 auto`.
   - The workspace rail no longer centers the 150px `thinking…` plate as if it
     were a full-width transcript row.
   - Diagnostic geometry at 1280x800 showed a 638px transcript and composer at
     y=702..788.

6. **Command/palette/session cleanup and docs**
   - Palette simplification, Island routing, session state handling, current
     desktop north-star contract, stylesheet registration, and compact layout
     changes are mixed into the same dirty checkout.

### Surface worktrees

| Worktree | State | Meaning |
|---|---|---|
| `/Users/smathdaddy-macbook/ocean-surface` | dirty `main`, 5 behind | current Island/component/native WIP |
| `/private/tmp/ocean-surface-voice-planner-20260714` | missing/prunable registration | superseded Voice Planner feature branch; equivalent work is upstream |
| `/private/tmp/ocean-surface-voice-planner-integrate-20260715` | missing/prunable registration | former integration lane; upstream now contains it |
| `.claude/worktrees/vscode-ext-ui` | heavily dirty `spike/vscode-embed-leptos-wasm` | replacing bespoke VS Code chat implementation with the canonical Leptos/WASM surface |

Do not prune registrations or merge the VS Code worktree without explicit
coordination. The VS Code branch is over 100 commits behind current Surface
main and its dirty work deletes much of the bespoke chat stack; it needs a
fresh integration plan, not a wholesale merge.

## Current Ocean OS Git state

At snapshot time:

```text
main:        827b65b feat(tui): reconcile rails permissions and session tray
origin/main: 827b65b
relationship: synchronized
```

The OS main worktree has 24 modified tracked files plus untracked
`crates/ocean-daemon/src/history_search.rs`; nothing is staged.

### Latest committed OS work

Current main includes:

- TUI rail, permission, and session-tray reconciliation;
- behavior-neutral request-control extraction;
- behavior-neutral model-role extraction;
- propose-only/no-tools Voice Planner safety;
- component-interaction and Slack Canvas fulfillment leaves;
- project/filesystem/settings/catalog/workspace/event-adapter daemon leaves;
- extension schema/tool-lane groundwork;
- Herdr lifecycle projection.

Daemon decomposition remains behavior-neutral. Preserve route parity, layer
order, permission authority, cwd semantics, SSE retention, and 404/405 behavior.
Persistent rooms/Longhouse/calls are later domain waves; turn/SSE orchestration
moves last.

### Current OS dirty feature clusters

1. **Three-mode permission workflow**
   - `manual`, `automatic` (default), `skip_all`.
   - Daemon settings endpoints own persisted/effective policy.
   - Legacy `/v1/settings/yolo` remains compatible; request-wire `yolo` is inert.
   - `OCEAN_YOLO=1` forces skip-all; `=0` prohibits skip-all without collapsing
     manual into automatic.
   - Blocked-turn release is request-scoped and token-bound.

2. **Persisted transcript history search**
   - `ocean-agent` owns bounded deterministic search.
   - Daemon exposes the bounded adapter route.
   - No provider, embedding, raw tool payload, or raw provider-message search.
   - `history_search.rs` is currently untracked and must not be omitted from any
     eventual landing.

3. **Surface component protocol**
   - `interactive_plot` allowlist/schema/help and agent guidance.
   - `surface-tauri` receives canonical Leptos/WASM component guidance.
   - The published roadmap still describes the Tauri mapping as missing; the
     mapping is fixed only in this dirty WIP until landed.

4. **Session-scoped todo state**
   - Bound sessions share one in-memory `TodoTool` across turns.
   - Separate/unbound sessions stay isolated.
   - Soft 1,024-entry target evicts only empty idle tools; non-empty todo state
     is never silently discarded.
   - Optional concise title is display-only; full todo text remains
     authoritative.

5. **TUI interaction tranche**
   - Collapsed consecutive tool bursts with truthful parent counts and nested
     per-call drawers.
   - Safe workspace-local Markdown document links.
   - Stable transcript-row drag selection and pane-edge clamping.
   - Context-meter dithering and session tray refinements.
   - Unicode-cell and terminal-sanitization invariants remain binding.
   - Herdr release timeout adjustment is mixed into the tree.

6. **Docs/devlog**
   - Some uncommitted `events.md` entries are dated July 18 even though this
     snapshot is July 15. Treat them as concurrent WIP claims, not published
     chronology, until reconciled.

### Ocean OS worktrees

| Worktree | State | Meaning |
|---|---|---|
| `/Users/smathdaddy-macbook/ocean-os` | dirty `main`, synced | broad permission/Recall/component/TUI WIP |
| `/private/tmp/ocean-daemon-phase2c-next` | clean, exactly `827b65b` | reserved clean daemon extraction lane |
| `.claude/worktrees/offshore` | dirty `feat/offshore`, 147 behind and 1 ahead | stale pre-merge survivor; equivalent offshore feature already landed on main |

Do not merge `feat/offshore` wholesale. Its unique feature commit is functionally
represented on main by the landed offshore commit. The remaining five dirty
files appear formatting-heavy and overlap current `component.rs`; salvage only
after a semantic comparison.

## Runtime state

At handoff creation:

```text
daemon listener: PID 1019 on 127.0.0.1:4780
health revision: 827b65bc9804
backend: deepseek/deepseek-v4-pro
permissions: persisted=skip_all, effective=skip_all
Tauri process: not running
```

Re-derive before acting. Daemon restarts must target the exact listener PID and
must use an up-to-date main-built binary. Never use a blind `pkill`.

`localhost:8790` serves the immutable release selected by
`~/.config/ocean-surface/current`, not this checkout's `dist/`. Do not report
uncommitted work as live based on a private proxy or stale service worker.

Do not launch/relaunch Tauri unless the operator explicitly asks to see it.
When a native launch is requested, use `./run-tauri.sh` so `dist/` is rebuilt;
direct `cargo tauri dev` can load stale assets.

## Validation already recorded

The current WIP has passed substantial focused validation at earlier checkpoints,
but the combined dirty trees and upstream Voice Planner reconciliation have not
been freshly gated as one landing candidate.

Recorded Surface evidence:

- 342 Surface tests passed;
- 7 focused `interactive_plot` tests passed;
- Surface WASM check passed;
- Surface clippy with `-D warnings` passed;
- Trunk release build passed;
- extension build passed;
- Tauri debug build passed;
- `git diff --check` passed at the relevant checkpoints;
- native wide/720px Island screenshots passed;
- disposable-session `interactive_plot` acceptance passed local slider, SVG,
  metric, and legend updates.

Recorded OS evidence across relevant checkpoints:

- runtime component tests passed;
- 34 `ocean-agent` prompt tests passed;
- daemon permission/history focused tests passed;
- full runtime/daemon/TUI suites were reported green in later dirty-tree event
  entries, including a 341-pass TUI run;
- workspace checks, docs checks, release TUI/daemon builds, rustfmt, and diff
  checks were reported passed.

Treat those as checkpoint evidence, not a substitute for rerunning gates after
integration. Do not trust future-dated uncommitted event claims without command
verification.

## Disposable acceptance session

The reusable plot acceptance used:

```text
session: 06cee094-e9f5-4096-aec8-521b018b8fa0
turn:    930631a0-f811-4728-a756-a9e56030dec6
image:   /tmp/interactive-plot-acceptance.png
```

Future live-agent checks must use a disposable session/fixture and a generous
turn budget. Do not post retries into existing user/TUI sessions, do not delete
or alter user-session turns, and do not use `max_turns: 1` for tool-using agents.

## Deferred/non-goals

- Semantic/embedding Recall remains deferred until a daemon/Bedrock contract
  exists.
- Exact Recall turn jump waits for a stable daemon-provided message/turn anchor.
- Background Island replies wait for a request-scoped SSE/token/cancellation
  contract that cannot blend the focused transcript.
- `simulation`/animated playback remains later; `interactive_plot` is the local
  reactive substrate.
- Native LiveKit remains later and feature-flagged.
- Provider calls, permission policy, transcript storage, and tool execution must
  not move into Surface/proxy/Tauri code.
- Agent, Sessions, Recall, and Actions must not be recombined into one
  dashboard/list.

## Recommended next move

Do not begin with feature expansion. Begin with integration preservation:

1. Re-derive both statuses and identify any changes since this handoff.
2. Inspect the five upstream Surface commits and the four overlapping dirty
   files.
3. Create an isolated clean integration lane from `origin/main`; do not mutate
   dirty main as the first move.
4. Port the current Surface WIP in logical slices while preserving upstream
   Voice Planner exactly:
   - Island model/UI + Recall transport;
   - `interactive_plot` component/style/protocol;
   - Tauri titlebar/host bridge;
   - transcript/composer geometry repair;
   - docs and stylesheet enumeration.
5. Keep one writer for `app.rs`, `daemon.rs`, `compact.css`, and `composer.css`.
6. Reconcile OS work into reviewable authority-aligned slices; ensure untracked
   `history_search.rs` is included.
7. Run fresh review and the complete validation matrix from clean candidate
   trees before committing or deploying.
8. Ask the operator before staging, committing, pruning worktrees, restarting
   Tauri, or altering existing sessions.

## Completion gates after reconciliation

Surface:

```sh
cargo test -p ocean-surface-ui
cargo check -p ocean-surface-ui --target wasm32-unknown-unknown
cargo clippy -p ocean-surface-ui --all-targets -- -D warnings
cargo check -p ocean-surface-proxy
trunk build --release
./scripts/build-extension.sh
(cd crates/ocean-tauri && cargo check)
cargo fmt --all -- --check
git diff --check
```

Ocean OS, scoped first and then full:

```sh
cargo test -p ocean-agent
cargo test -p ocean-runtime
cargo test -p ocean-daemon
cargo test -p ocean-tui
cargo build -p ocean-tui --release
cargo check --workspace --tests
cargo xtask docs-check
cargo fmt --all -- --check
git diff --check
```

Use the stricter root/CI gates from `../ocean-os/AGENTS.md` when preparing an
actual merge. Fresh independent review is required for feature, logic,
security, protocol, or architecture changes.
