# Code Context

## Files Retrieved
1. `handoff.md` (lines 93-209, 322-441) - declares the dirty clusters, upstream Voice Planner overlap, prior checkpoint validation, deferred work, and intended port order.
2. `crates/ocean-surface-ui/src/main.rs` (lines 15-31) - shared module declarations; currently references three untracked modules.
3. `crates/ocean-surface-ui/src/app.rs` (lines 340-367, 466-483, 788-851, 1042-1076, 1466-1571, 1680-1680, 1774-1802) - shared integration point for Island state/mount/shortcuts, palette ownership, and composer geometry/IME behavior.
4. `crates/ocean-surface-ui/src/daemon.rs` (lines 767-850, 1360-1395, 1852-2120, 2680-2817, 2975-3051, 5052-5158) - Island DTO/signals, attention polling, Recall transport, cancellation, SSE lifecycle repair, and tests.
5. `crates/ocean-surface-ui/src/island.rs` (lines 1-500 and 501-905) - untracked pure Island session/attention projection, search ranking, action gating, and unit tests.
6. `crates/ocean-surface-ui/src/island_dynamic.rs` (lines 1-1011) - untracked Tauri titlebar UI, four-mode state, polling, focus/keyboard behavior, permission/cancel/session/Recall actions, and tests.
7. `crates/ocean-surface-ui/src/search.rs` (lines 1-133) - untracked shared fuzzy scorer extracted from palette and used by Island.
8. `styles/island.css` (lines 1-841) - untracked complete Island styling and responsive geometry.
9. `crates/ocean-surface-ui/src/components.rs` (lines 8-82) - shared component dispatch references the untracked plot module.
10. `crates/ocean-surface-ui/src/components/interactive_plot.rs` (lines 1-867) - untracked bounded expression parser/evaluator, local reactive renderer, committed-event transport, and seven tests.
11. `styles/components.css` (lines 10-55, 768-1062) - plot container integration and 294 lines of plot styling.
12. `crates/ocean-tauri/src/lib.rs` (lines 269-284, 1050-1082, 1135-1284, 1340-1346) - diagnostic resize/script hooks plus expanded native application menus.
13. `styles/chrome.css` (lines 17-37) - titlebar geometry, including full-window header width when workspace is collapsed.
14. `styles/transcript.css` (lines 161-173, 880-891), `styles/composer.css` (lines 4-110, 465-505, 613-634), `styles/workspace.css` (lines 69-86), `styles/compact.css` (lines 96-115, 235-270) - transcript/composer geometry and compact-control changes.
15. `docs/OCEAN_WEB_SURFACE_DESIGN.md` (lines 311-324), `docs/OCEAN_DESKTOP_NORTH_STAR.md` (lines 1-457), `docs/OCEAN_DYNAMIC_ISLAND_IMPLEMENTATION.md` (lines 1-340), `docs/OCEAN_DYNAMIC_ISLAND_BUILD_PLAN.md` (lines 1-202) - style contract and Island design/build documentation.
16. `index.html` (lines 37-43), `extension/sidepanel.html` (lines 10-17), `scripts/build-extension.sh` (lines 20-27) - required stylesheet enumeration.
17. `crates/ocean-surface-ui/src/palette.rs` (lines 8-15, 191-280, 242-477) - fuzzy scorer extraction and externally-owned palette-open state needed for Island overlay exclusion.
18. `crates/ocean-surface-ui/src/sessions.rs` (lines 13-18, 148-213, 1235-1276) - concurrent behavior change that restores zero-turn sessions to the panel.
19. `crates/ocean-surface-ui/tests/voice_realtime_regressions.rs` (lines 79-95) - composer Stop-slot selector expectation.
20. `design-systems/ocean-leptos/**`, `design-systems/ocean-tui/**` - untracked exported design-system documents/assets/zips, including `.DS_Store`; not runtime inputs.

## Key Code

### Inventory by logical slice

**Island + Recall (port as one dependency-aware feature slice)**
- New/untracked: `island.rs`, `island_dynamic.rs`, `search.rs`, `styles/island.css`, both `docs/OCEAN_DYNAMIC_ISLAND_*.md`.
- Shared wiring: `main.rs`, Island hunks in `app.rs`, scorer/open-state hunks in `palette.rs`, and Island/Recall hunks in `daemon.rs`.
- `IslandMode::{Closed, Agent, Sessions, Recall}` is at `island_dynamic.rs:18-24`. `DynamicIsland` polls daemon attention every 3s and derives session/attention views at `island_dynamic.rs:342-405`.
- Daemon contract: `RequestSnapshot`, `PermissionSnapshot`, and `HistorySearchHit` at `daemon.rs:767-850`; `fetch_attention` at 2680-2748; generation-guarded `search_history` at 2750-2817; request cancellation at 2975-3051.
- App contract: Tauri-only commands and shortcuts at `app.rs:466-483,788-825`, mutual-exclusion/Escape state at 340-367,827-851, and mount at 1042-1076.
- Cross-repo hard dependency: daemon endpoints `GET /v1/requests`, `GET /v1/permissions`, `GET /v1/agent/history/search`, and `POST /v1/requests/{id}/cancel`. Recall is not independently useful until the corresponding `ocean-os` authority slice lands.

**interactive_plot (independent after its two shared-file hunks)**
- `components/interactive_plot.rs` implements a bounded DSL (12 parameters, 6 series, 12 metrics, 512 samples, 512-char expressions), local recomputation, SVG/metrics, and `parameters_changed` only on committed changes.
- Requires `components.rs:14-15,77-80` and plot CSS in `styles/components.css:768-1062`.
- No dependency on Island/Tauri. Runtime protocol/allowlist and agent guidance live in sibling `ocean-os` and must land before agents can reliably emit it.
- Seven local unit tests are embedded at `interactive_plot.rs:769-866`.

**Tauri host/titlebar**
- Actual Island placement is mostly Surface code (`app.rs:1042-1076`, `styles/island.css`, `styles/chrome.css:17-37`) and existing `host::running_in_tauri`; there is no new `host.rs` wrapper.
- `ocean-tauri/src/lib.rs` changes are separable: (a) diagnostic-only resize/script injection (`269-284`, `1058-1082`, invoke registration), and (b) expanded native App/File/Edit/Window menus (`1145-1284`). Neither is required to compile or mount Dynamic Island. Treat them as native acceptance/native-feel follow-ups, not a prerequisite for Island.

**Transcript/composer geometry**
- Core repair: `styles/transcript.css` (`flex:1 1 0; min-height:0`), `styles/composer.css` (`flex:0 0 auto`, compact two-row grid, 32px input), `styles/workspace.css` (left-align 150px pending plate), and `app.rs` composer minimum/`rows=1`.
- `compact.css` scales voice controls and row gaps; `voice_realtime_regressions.rs` adds Stop to the hidden-control selector contract; `docs/OCEAN_WEB_SURFACE_DESIGN.md` documents geometry.
- IME/palette Escape changes in `app.rs` and `palette.rs` are useful correctness fixes but mixed into shared files; preserve them intentionally rather than assuming they are geometry.
- High conflict risk: `app.rs`, `daemon.rs`, `compact.css`, and `composer.css` overlap the five upstream Voice Planner commits (`handoff.md:107-137`). Reapply hunks onto current `origin/main`; do not wholesale copy these files.

**Docs/style enumeration**
- Stylesheet registration is a three-file atomic set: `index.html`, `extension/sidepanel.html`, and `scripts/build-extension.sh`, per repository contract. Add `styles/island.css` in the same patch.
- The two Dynamic Island docs belong with Island. The small web design doc composer edit belongs with geometry.
- `docs/OCEAN_DESKTOP_NORTH_STAR.md` is a 611-line rewrite (406 additions/205 deletions) spanning strategy and future work; review/port separately after code behavior is settled rather than hiding it in the feature patch.

**Unrelated/concurrent; do not blindly port**
- `sessions.rs`: changes product behavior to show all zero-turn drafts, contrary to the binding AGENTS.md web session rule that lazy New Session should avoid/prune litter. It is not required by Island because Island consumes daemon sessions directly. Treat as concurrent/stale pending product decision; default recommendation: do not port.
- `daemon.rs` explicit EventSource handle/close repair (`daemon.rs:33-61,1360-1370,1852-2120`) fixes stale browser connection slots but is orthogonal to Island/Recall. Extract as its own reviewed transport patch; do not bury it in Island.
- `daemon.rs:5903-5912` rewrites a match as `if let` (likely clippy cleanup), unrelated.
- `crates/ocean-tauri/src/lib.rs` standard menus and diagnostic scripting are concurrent native work, not Island host authority. Diagnostic hooks are explicitly test-only and must not expand into runtime authority.
- `design-systems/**` exports/zips and `.DS_Store` are artifact/reference material, not product runtime. Do not port `.DS_Store` or generated zips; only land curated docs/assets if separately requested.
- `.pi-subagents/**` is orchestration output and must never be included in a product patch (except this requested report artifact).
- `handoff.md` is operational snapshot prose, not feature implementation; do not cherry-pick it into a product slice unless intentionally refreshing handoff state.

### Shared-file dependency hazards
- `main.rs` currently declares untracked `island`, `island_dynamic`, and `search`; landing it without those files breaks compilation.
- `components.rs` declares untracked `components/interactive_plot.rs`; land together.
- `palette.rs` imports untracked `search::fuzzy_score`; land `search.rs` first/same commit. Its `PaletteView` signature changed, so `app.rs` caller must land simultaneously.
- `app.rs` imports untracked `island_dynamic`; never land that import/mount before Island modules compile.
- `index.html` and extension HTML reference untracked `styles/island.css`; missing CSS does not Rust-fail but produces incomplete packaging/UI.
- Island model uses helpers from `sessions.rs`, but does not require the zero-turn visibility behavior hunk.

## Architecture
The canonical Leptos app owns all product UI. `app.rs` conditionally mounts `DynamicIsland` only when `host::running_in_tauri()` is true. `island_dynamic.rs` renders and coordinates modes; `island.rs` is the pure projection/ranking layer; `search.rs` supplies shared fuzzy ranking. `Daemon` remains a transport/client facade: global request/permission snapshots feed Agent mode, daemon-owned persisted-history search feeds Recall, and focused SSE state plus permission identity gates mutations. The Tauri backend remains thin and is not required for search/permission authority.

`interactive_plot` enters through `ComponentView`; it parses declarative props, computes locally in WASM, renders SVG and controls, and sends only committed parameter events through the existing component event daemon method. It is intentionally unrelated to the Island.

## Suggested cherry-pick / patch sequence
1. Start a clean integration lane from current `origin/main` (dirty HEAD was `8459c2c`, documented five commits behind). Preserve upstream Voice Planner first.
2. Optional standalone transport repair: extract EventSource ownership/closure plus its tests from `daemon.rs`. Review independently.
3. Island pure foundation: add `search.rs` and `island.rs`; move palette fuzzy tests to `search.rs` but defer `PaletteView` signature wiring until step 5.
4. Island daemon client contract: selectively port attention/Recall/cancel DTOs, signals, constructors, methods, and focused tests from `daemon.rs`; coordinate with `ocean-os` endpoints.
5. Island UI integration atomically: add `island_dynamic.rs`, `styles/island.css`; then smallest hunks in `main.rs`, `app.rs`, and `palette.rs`. Include the three stylesheet enumeration files. This obeys the shared-file rule: new modules compile before/with references.
6. `interactive_plot`: add its module, `components.rs` dispatch, and only its CSS block. Coordinate the sibling daemon/runtime allowlist/schema/guidance patch.
7. Tauri/native follow-up: titlebar-width CSS first; native menus second; diagnostic hooks only if acceptance automation still needs them. Keep these commits separate.
8. Geometry repair: selectively reapply transcript/composer/workspace/compact CSS, composer min/rows/IME hunks, realtime regression test, and web design doc onto Voice Planner-aware files.
9. Documentation: Dynamic Island docs, then separately review the large North Star rewrite. Exclude handoff, `.pi-subagents`, `.DS_Store`, generated zips, and zero-turn session behavior by default.
10. Run the complete fresh gate matrix from `handoff.md:409-441`; prior checkpoint results are evidence only, not validation of the reconciled tree.

## Start Here
Open `handoff.md` lines 93-209, then diff current `origin/main` against the four high-conflict files. The first implementation file to port should be `crates/ocean-surface-ui/src/search.rs`: both palette and Island depend on it, and it can land/test without touching the high-conflict app/daemon files.

## Residual risks / review findings
- **Blocker for direct cherry-pick:** dirty base is behind upstream and four core files overlap Voice Planner. Whole-file copying can silently delete confirmed planner behavior.
- **Protocol blocker for complete live behavior:** Island Recall/attention and plot emission depend on sibling `ocean-os` WIP not inventoried here.
- **Potential stale product change:** `sessions.rs` restores zero-turn drafts despite the repository session contract; exclude absent explicit approval.
- **Partial titlebar naming:** the Tauri backend diff contains menus/debug automation, not a dedicated Island host bridge. Do not infer these are mandatory Island dependencies.
- No files were staged at inspection time (`git diff --cached --quiet` returned success). `git diff --check HEAD` produced no output.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Read-only inventory grouped every tracked product diff and untracked root into the requested slices without modifying project/source files."
    },
    {
      "id": "criterion-2",
      "status": "satisfied",
      "evidence": "Report cites exact files/line ranges, shared-module dependencies, cross-repo contracts, stale/concurrent exclusions, conflict risks, and a patch sequence."
    }
  ],
  "changedFiles": [
    ".pi-subagents/artifacts/outputs/ccdc45fb-c56e-456c-adf7-31e285b84062/context.md"
  ],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    {
      "command": "git status --short; git diff --stat/name-status/numstat HEAD; git ls-files --others --exclude-standard",
      "result": "passed",
      "summary": "Enumerated 20 modified tracked files, all untracked roots, and confirmed no staged diff."
    },
    {
      "command": "git diff --unified=2 HEAD -- <all modified source/style/host/docs files>",
      "result": "passed",
      "summary": "Inspected tracked changes versus HEAD by logical area."
    },
    {
      "command": "git diff --check HEAD",
      "result": "passed",
      "summary": "No whitespace errors reported."
    },
    {
      "command": "git diff --cached --quiet",
      "result": "passed",
      "summary": "Exit status 0; no staged files."
    }
  ],
  "validationOutput": [
    "Read-only inspection only; no build or test suite was rerun.",
    "Handoff records earlier checkpoints (342 Surface tests, 7 plot tests, WASM/clippy/Trunk/extension/Tauri checks), but explicitly says the reconciled candidate has not been freshly gated."
  ],
  "residualRisks": [
    "Dirty local main is documented five commits behind origin/main with conflicts in app.rs, daemon.rs, compact.css, and composer.css.",
    "Live Recall/attention and interactive_plot protocol require coordinated ocean-os changes.",
    "Zero-turn sessions behavior appears stale or policy-conflicting and should not be ported without approval."
  ],
  "noStagedFiles": true,
  "diffSummary": "Read-only feature inventory and recommended selective-port sequence; only the required report artifact was written.",
  "reviewFindings": [
    "blocker: handoff.md:107-137 - direct merge/cherry-pick risks overwriting upstream Voice Planner in four shared files.",
    "warning: crates/ocean-surface-ui/src/sessions.rs:148-213 - zero-turn session visibility is unrelated and conflicts with the documented lazy-session discipline.",
    "warning: crates/ocean-tauri/src/lib.rs:269-284,1058-1082 - diagnostic hooks are test-only and not Island runtime authority."
  ],
  "manualNotes": "Reviewer gate remains required. Use a clean origin/main integration lane and one writer for shared files."
}
```
