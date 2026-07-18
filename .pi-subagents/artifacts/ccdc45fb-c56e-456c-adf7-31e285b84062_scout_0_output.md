# Code Context

## Files Retrieved

1. `handoff.md` (lines 1-594) - authoritative dirty-tree inventory, upstream warning, prior validation, and proposed integration lane.
2. `docs/OCEAN_DYNAMIC_ISLAND_IMPLEMENTATION.md` (lines 1-340) - binding Island state, authority, shortcut, Recall, and geometry contract.
3. `docs/OCEAN_DYNAMIC_ISLAND_BUILD_PLAN.md` (lines 1-202) - intended phases, file map, hardening commands, and stop conditions.
4. `crates/ocean-surface-ui/src/app.rs` (dirty hunks at current lines 14, 24, 44-47, 273-274, 342-367, 466-483, 788-846, 1045-1076, 1466-1576, 1681-1682, 1776-1800) - Island routing, palette ownership, shortcuts, header mount, composer/IME repair, and tests.
5. `crates/ocean-surface-ui/src/daemon.rs` (current lines 15-55, 767-850, 1362-1395, 1854-1954, 2680-2817, 2975-3050, 5054-5155) - dirty EventSource closure mechanism, Island/Recall DTOs and transport, cancellation, and tests.
6. `styles/compact.css` (dirty base-file hunks at lines 98, 106, 236, 244, 261; upstream append after base line 609) - compact composer/orb changes versus appended Voice Planner responsive rules.
7. `styles/composer.css` (dirty base-file hunks at lines 6-24, 37-50, 96-103, 466-508, 615-621; upstream append after base line 738) - composer geometry/IME presentation versus appended planner card styles.
8. `crates/ocean-surface-ui/src/main.rs` (lines 21-22, 30) - module registration for `island`, `island_dynamic`, and `search`.
9. `crates/ocean-surface-ui/src/components.rs` (lines 14-15, 77-79) and `crates/ocean-surface-ui/src/components/interactive_plot.rs` (lines 1-867; tests 769-866) - interactive plot registration and implementation.
10. `crates/ocean-surface-ui/src/island.rs` (lines 1-905; tests 516-904), `crates/ocean-surface-ui/src/island_dynamic.rs` (lines 1-1011), and `crates/ocean-surface-ui/src/search.rs` (lines 1-133; tests 75-132) - pure projections/scoring and active Island UI.
11. `crates/ocean-surface-ui/tests/voice_realtime_regressions.rs` (lines 78-96) - dirty assertion adding the Stop selector to realtime-hidden composer controls.
12. `styles/transcript.css` (lines 160-172), `styles/workspace.css` (lines 70-78), and `styles/composer.css` (lines 10-24) - pending-state geometry repair.
13. `index.html` (lines 39-45), `extension/sidepanel.html` (lines 12-18), and `scripts/build-extension.sh` (stylesheet list containing `island.css`) - three-way stylesheet enumeration.
14. `crates/ocean-tauri/src/lib.rs` (dirty native diagnostic/titlebar hunks; diagnostic command begins lines 269-287 and setup around 1058) - thin native acceptance hooks.
15. Upstream commits `20fc55b`, `f51d859`, `c12b679`, `b845a9a`, `b09db43` inspected by log, stats, name-status, and per-file diffs.

## Key Code

### Repository state

- `main` is exactly 5 commits behind `origin/main` (`0 5` from `git rev-list --left-right --count main...origin/main`), with no local commits ahead.
- Dirty tree: 20 modified tracked paths plus untracked Island, search, plot, docs, style, and design-system material. Nothing is staged. `.pi-subagents/` is also untracked due to the required report artifact.
- Upstream five commits are logically three changes plus clippy/merge:
  - `20fc55b`: launcher/docs only; no feature overlap.
  - `f51d859`: planner contract, `voice/planner.rs`, realtime isolation, and 272 lines in `daemon.rs`.
  - `c12b679`: confirmed planner workflow; very large `app.rs`/`daemon.rs` additions and appended CSS.
  - `b845a9a`: clippy-only edits in `app.rs` and `daemon.rs`.
  - `b09db43`: merge commit with no resulting file delta in the inspected log.

### Exact textual conflict evidence

A three-way `git merge-file` simulation used `main` as base, current working files as local, and `origin/main` blobs as remote:

- `app.rs`: **1 textual conflict only**, in the test module import list. Dirty imports `should_submit_composer_key`; upstream imports planner workflow helpers/types. Both lists must be retained. The merged-output conflict was lines 2652-2660.
- `daemon.rs`: **5 textual conflicts**:
  1. imports: dirty `Arc, Mutex` versus upstream `Rc` (merged output 17-22);
  2. `Daemon` stream fields: dirty explicit `agent_event_source` / `permission_event_source` slots versus upstream readiness/revision/`planner_stream_sources` (1432-1449);
  3. production constructor initialization of those fields (1729-1741);
  4. dummy constructor initialization (1800-1812);
  5. generation transition: dirty closes both old streams, upstream clears both planner readiness markers (1980-1989).
- `compact.css`: **textually clean**. Dirty edits are near base lines 98-264; upstream only appends planner responsive CSS after base line 609.
- `composer.css`: **textually clean**. Dirty edits are near base lines 6-621; upstream only appends planner card CSS after base line 738.

Textual cleanliness does not imply semantic safety for `app.rs` or CSS.

### High-risk semantic overlap

#### `daemon.rs` — blocker-level integration seam

Dirty code explicitly closes prior browser streams on focus/generation changes (`daemon.rs:35-55`, `1854-1863`) to prevent two long-lived EventSources per stale session exhausting browser HTTP/1.1 origin slots. Upstream planner code instead retains both active sources in `PlannerStreamSources`, monitors `EventSourceState::Open`, and requires exact session/generation readiness before `start_planner_turn` (upstream `daemon.rs:1294`, `1758-1797`, `2942-2978`, `3352-3373`).

A mechanical “keep both sides” resolution risks:

- two independent owners/handles for the same EventSource;
- closing a stream without synchronously invalidating upstream planner readiness/lifecycle;
- planner believing streams are open after dirty slots closed them;
- stale stream installation racing a newer generation;
- browser connection leakage if only readiness markers are cleared.

Safe resolution should use **one stream ownership model**. Start from upstream planner lifecycle and extend its source replacement/generation-reset path to explicitly close/take the old `agent` and `permission` sources while also clearing readiness/lifecycle. Do not bolt the dirty `Arc<Mutex<Option<SendWrapper<EventSource>>>>` slots alongside upstream `Rc<RefCell<PlannerStreamSources>>` without proving a single owner and atomic state transition.

Also reconcile the dirty permission snapshot type with upstream `PermissionsResponse`/`PermissionStatusWire`: dirty `PermissionSnapshot` deliberately excludes raw args for Island display (`daemon.rs:801-817`), while upstream needs args to rebuild focused `PendingPermission`. These should remain distinct DTOs or share a wire DTO with explicit projection; do not weaken the Island boundary by exposing raw args.

#### `app.rs` — low textual, high behavioral overlap

Upstream inserts roughly 805 lines of planner workflow before `App` and mounts `VoicePlannerCard` around upstream line 2081. Dirty integration adds global overlay state and changes `PaletteView` from one argument to controlled `open=palette_open` (`app.rs:342-367`, `1681-1682`). It also changes Escape priority and Cmd/Ctrl routing (`app.rs:788-846`) and header layout (`1045-1076`). Risks:

- planner card is another reveal surface but dirty `open_island` currently closes palette/council/rooms/sessions only, not planner;
- upstream planner opening/closing may not close Island, allowing stacked titlebar/planner UI;
- dirty controlled palette API requires preserving `palette.rs` changes; simply porting `app.rs` will not compile against upstream `PaletteView` signature;
- Voice Planner's deliberate eager session creation exception must survive; Island idle submission must continue through ordinary focused/lazy `Daemon::send_prompt` and must not reuse planner creation APIs;
- upstream test-import conflict is trivial syntactically but indicates both test suites must remain.

Before editing, identify the upstream planner-open signal near `VoicePlannerCard` and explicitly decide overlay precedence/mutual exclusion. The current Dynamic Island docs do not mention Voice Planner.

#### CSS

The CSS combines cleanly because planner rules append, but validate cascade and geometry:

- retain upstream `.voice-plan*` blocks in `composer.css` and responsive blocks in `compact.css` verbatim;
- retain dirty composer dock ownership (`flex: 0 0 auto`) and transcript ownership (`styles/transcript.css:167`, `flex: 1 1 0; min-height:0`);
- dirty compact orb shrinks 38px to 32px and changes gaps; this can affect planner/realtime compact screenshots even without selector collision;
- dirty realtime selector adds `.ocean-composer__halt`; keep its regression test in the same slice;
- stylesheet order must stay tokens → … → composer → panels/call/canvas → compact → float, with `island.css` enumerated identically in all three packaging lists.

## Architecture

`app.rs` owns application-level overlays, shortcuts, and host-conditional mounting. `island_dynamic.rs` owns per-mode interaction and calls `Daemon`; `island.rs` and `search.rs` provide pure projection/ranking. `daemon.rs` owns HTTP/SSE client state but not runtime authority. Upstream Voice Planner also enters through `app.rs`, depends on `voice/planner.rs` and `voice/realtime.rs`, and adds strict session/SSE readiness operations to `Daemon`.

The dirty work is not one portable patch. It contains at least five coupled slices:

1. Dynamic Island + Recall UI/transport.
2. Interactive plot protocol/UI/style.
3. Tauri titlebar and diagnostics.
4. Transcript/composer pending geometry and IME/realtime regression.
5. Palette/session/docs/style-enumeration cleanup.

Recall's endpoint authority is not implemented in this repository: the handoff says the sibling `ocean-os` daemon/history work is still dirty and includes an untracked `history_search.rs`. Surface Recall can compile before that lands, but live Recall cannot pass end-to-end acceptance against an upstream-only daemon.

## Safe logical-slice port order

Use a clean worktree rooted at `origin/main`; never merge/rebase the dirty checkout. Keep one writer for the four overlap files.

1. **Establish pristine upstream baseline.** Run Surface tests/WASM/clippy (at minimum) before ports so planner failures are distinguishable from WIP regressions. Preserve all five upstream commits as the base rather than cherry-picking around them.
2. **Port self-contained pure modules first, without shared-file references:** `search.rs`, `island.rs`, `island_dynamic.rs`, `interactive_plot.rs`, `styles/island.css`, and additive plot styles. This follows the shared-file rule: do not add `mod` declarations until each module compiles in its integration commit.
3. **Port interactive plot as its own compiling slice:** `components.rs` + `components/interactive_plot.rs` + its `styles/components.css` additions. Run the 7 plot tests and component rendering tests. This is independent of planner/Island transport and reduces later diff noise.
4. **Integrate `daemon.rs` once, manually on upstream:** first unify EventSource lifecycle/closure as described above; then add Island request/permission/Recall DTOs/signals, `fetch_attention`, generation-guarded search, and cancellation. Preserve upstream planner hydration, readiness, permission reconciliation, and clippy fix. Run all daemon unit tests before touching `app.rs`.
5. **Integrate Island modules into `main.rs`, then `app.rs`:** add module declarations only now; retain upstream planner card/workflow wholesale, add Island mode/commands/header mount, reconcile controlled palette state, and make planner/Island overlay precedence explicit. Merge both test import lists. Preserve Tauri-only Print/Find interception and IME guards.
6. **Port palette/session cleanup only as required by the `app.rs` contract:** `palette.rs` controlled `open` API and `sessions.rs` changes should be reviewed separately from Island visuals. Avoid opportunistically carrying broad deletions unless each is required and tested.
7. **Port Tauri thin-host slice:** `crates/ocean-tauri/src/lib.rs`, `styles/chrome.css`, titlebar geometry, and diagnostic hooks. Confirm no runtime/provider/session authority moved host-side.
8. **Port composer/transcript geometry as a standalone visual/regression slice:** dirty `composer.css`, `compact.css`, `transcript.css`, `workspace.css`, composer constants/IME helper in `app.rs`, and `voice_realtime_regressions.rs`. Apply dirty CSS edits onto upstream files so appended `.voice-plan*` rules remain. This slice should be separately screenshot-tested at wide/720/extension widths.
9. **Port docs and packaging last:** `index.html`, `extension/sidepanel.html`, `scripts/build-extension.sh`, Dynamic Island docs, and narrowly reconcile north-star/design docs against current upstream wording. Do not port `handoff.md` as product code. Do not blindly port `design-systems/` without an ownership/scope decision.
10. **Fresh full candidate gate and detached-worktree review:** run the complete Surface matrix below plus native wide/720 interaction screenshots. Only then consider staging/committing.

## Tests affected

Existing/new dirty tests that must survive:

- 16 tests in `island.rs` (projection, ordering, permissions/actions/modes).
- 9 tests in `search.rs` (deterministic fuzzy behavior).
- 7 tests in `components/interactive_plot.rs`.
- 4 new/affected daemon test groups: request snapshot decoding, cancellation body semantics, stale Recall invalidation/response decoding, permission raw-args exclusion; plus upstream planner wire, stream lifecycle/readiness, reconciliation, and retry tests.
- `app.rs` composer IME/newline test plus all upstream planner workflow/context tests.
- `tests/voice_realtime_regressions.rs:78-96` selector expectation for Stop.

Required fresh gates after integration:

```text
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

Also manually exercise Voice Planner `Create draft` and `Create & start`, planner permission readiness, repeated session switching (to expose EventSource leaks), Island permission/cancel authority, browser Print/Find, extension packaging, and wide/720 layouts.

## Unresolved decisions / residual risks

1. **EventSource ownership is the key unresolved engineering decision.** Recommended: one upstream-derived `PlannerStreamSources` owner enhanced with explicit close/take semantics. It needs focused review because it protects both planner permission correctness and connection-slot exhaustion.
2. **Planner-versus-Island overlay precedence is unspecified.** Decide whether opening either closes the other; stacking contradicts the Island “one topmost surface” intent.
3. **Recall backend availability:** live acceptance is blocked until the sibling daemon's history route lands; Surface must show truthful unavailable/error state meanwhile.
4. **Broad dirty docs/palette/session changes exceed the minimal Island port unless proven necessary.** Reconcile rather than copy wholesale.
5. **`design-systems/` is untracked and not described as a required runtime slice.** Exclude unless explicitly approved.
6. Prior “342 tests passed” and build claims are checkpoint evidence only; no combined origin/main-based candidate exists yet.

## Start Here

Open upstream `crates/ocean-surface-ui/src/daemon.rs` first, specifically the planner stream fields and `connect`/`connect_permission_stream` lifecycle (upstream lines approximately 1213-2138), because this is the only blocker-level merge seam and determines whether planner safety and stale-stream closure can coexist.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Read-only integration recon only; no project/source files were modified. Findings were written solely to the mandated artifact path."
    },
    {
      "id": "criterion-2",
      "status": "satisfied",
      "evidence": "Report includes commit inventory, exact three-way conflict counts and conflict contents, hunk/line evidence, port order, affected tests, validation matrix, and residual decisions."
    }
  ],
  "changedFiles": [
    ".pi-subagents/artifacts/outputs/ccdc45fb-c56e-456c-adf7-31e285b84062/context.md"
  ],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    {
      "command": "git status --short --branch; git log; git rev-list; git log main..origin/main",
      "result": "passed",
      "summary": "Confirmed dirty main is 0 ahead/5 behind and enumerated the five upstream commits."
    },
    {
      "command": "git diff/log --stat/--name-status and per-file unified/hunk diffs",
      "result": "passed",
      "summary": "Mapped dirty and upstream changes, especially app.rs, daemon.rs, compact.css, and composer.css."
    },
    {
      "command": "git merge-file -p using main base, dirty local, origin/main remote blobs",
      "result": "passed",
      "summary": "Found 1 textual conflict in app.rs, 5 in daemon.rs, and clean textual merges for both CSS files."
    },
    {
      "command": "git diff --check; git diff --cached --name-only",
      "result": "passed",
      "summary": "Existing tracked diff has no whitespace errors and there were no staged files."
    }
  ],
  "validationOutput": [
    "main...origin/main divergence: 0 5",
    "Three-way simulation: app.rs CONFLICT (1 region); daemon.rs CONFLICT (5 regions); compact.css CLEAN; composer.css CLEAN",
    "git diff --check produced no output",
    "git diff --cached --name-only produced no output"
  ],
  "residualRisks": [
    "EventSource ownership/closure must be unified with upstream planner readiness rather than retaining two independent handle systems.",
    "Voice Planner versus Dynamic Island overlay precedence is not specified.",
    "Live Recall requires sibling ocean-os dirty/untracked backend work that is not yet on its clean upstream.",
    "No combined origin/main-based candidate was built or tested during this read-only recon."
  ],
  "noStagedFiles": true,
  "diffSummary": "Added only the requested reconnaissance artifact; project/source tree was not edited.",
  "reviewFindings": [
    "blocker: crates/ocean-surface-ui/src/daemon.rs stream lifecycle - dirty explicit closure and upstream planner readiness have competing ownership/state models; mechanical conflict resolution can leak streams or falsely authorize planner start.",
    "high: crates/ocean-surface-ui/src/app.rs overlay routing - planner and Island mutual exclusion/precedence is currently undefined.",
    "medium: styles/compact.css and styles/composer.css - textual merge is clean, but compact geometry and realtime selectors require planner/extension screenshot regression checks."
  ],
  "manualNotes": "Independent review is required. Use a clean origin/main-based worktree and one writer for app.rs, daemon.rs, compact.css, and composer.css."
}
```
