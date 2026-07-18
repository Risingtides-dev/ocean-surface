# Implementation Plan

## Goal

Replace the current combined activity-and-session popover with one compact Island that morphs into separate agent, session-switching, history-recall, and attention modes.

## Review Findings

1. **High — distinct intents are clobbered into one surface**
   - File: `crates/ocean-surface-ui/src/island.rs:17-22, 813-1230`
   - `IslandMode` distinguishes only `Closed`, `Browse`, and `Search`, while every open state renders search, Activity disclosures, and the session catalogue together. This is exactly the “list with drawers at top” problem described by the user.

2. **High — the current visual behavior is a conventional modal/list dashboard**
   - File: `styles/island.css:112-400`
   - The fixed 560px popover, dimming scrim, independently scrolling Activity section, and long vertical result list produce a dropdown dashboard rather than a shape-changing titlebar object.
   - Evidence: `/tmp/ocean-island-final-wide.png` shows Activity above a session list; `/tmp/ocean-island-narrow.png` shows the surface occupying most of the window height.

3. **High — agent interaction and history recall have no interaction modes**
   - File: `crates/ocean-surface-ui/src/app.rs:273-364, 778-824`
   - Clicking enters Browse and `⌘P` enters session Search. There is no route for focused-agent interaction or history recall, and no separate keyboard semantics for them.

4. **High — current search is session-catalogue filtering, not history search**
   - File: `crates/ocean-surface-ui/src/island.rs:327-407`
   - `island_search_text` indexes title, project, path, branch, origin, state, and turn count. It does not search prompts, assistant summaries, mentioned files, decisions, or unresolved topics.

5. **High — existing specifications now conflict with the corrected direction**
   - Files:
     - `docs/OCEAN_DYNAMIC_ISLAND_IMPLEMENTATION.md:21-39`
     - `docs/OCEAN_DYNAMIC_ISLAND_BUILD_PLAN.md:217-255`
   - Both documents intentionally unify Browse and Search into the same overlay and defer inline agent interaction and semantic history search. The correction requires superseding this V1 interaction model rather than polishing it.

6. **Medium — the compact state emphasizes catalogue counts instead of living work**
   - File: `crates/ocean-surface-ui/src/island.rs:695-751`
   - Falling back to “n recent” makes the compact Island advertise browsing. Quiet state should preserve focused identity; trailing space should appear only for authoritative running or needs-human work.

7. **Blocker dependency — no verified daemon contract exists for hybrid history recall**
   - File: `crates/ocean-surface-ui/src/daemon.rs`
   - No session-history semantic/fuzzy search client was found. Search authority and indexing must be added in `ocean-os`/Bedrock rather than implemented locally in the Surface.

## Target Interaction Architecture

### Principle

**One Island, four mutually exclusive expanded modes. Never render one mode as a section inside another.**

```text
Ambient
 ├─ Engage     focused-agent interaction
 ├─ Orbit      session switching
 ├─ Recall     semantic/fuzzy history retrieval
 └─ Attention  approvals, failures, and running work
```

There are no tabs and no default “everything” view. Each entry gesture answers one question.

### Compact-state anatomy

The capsule remains visually one object but has three semantic hit regions:

1. **Leading Ocean/state mark → Engage**
   - Opens focused-agent interaction.
   - Reflects idle, streaming, or waiting state.

2. **Focused session identity → Orbit**
   - Opens session switching.
   - Never disappears behind status metadata.

3. **Trailing authoritative badge → Attention**
   - Rendered only for `NeedsHuman` or active running work.
   - No “recent sessions” count in the quiet compact state.

History Recall is intentionally absent from permanent chrome. It is invoked by `⌘⇧F` or the command registry.

At narrow widths, retain mark, truncated session title, and attention count. Remove project and prose status first.

### Mode 1: Engage

Purpose: interact with the agent in the currently focused session.

- The capsule expands into a shallow, anchored stage—not a transcript drawer.
- Show only:
  - focused session identity;
  - one bounded latest-agent/status line;
  - one compact prompt field.
- Enter submits through the existing `Daemon::send_prompt`, preserving lazy session creation.
- Background-session replies are prohibited.
- Successful submission collapses to Ambient, where running state becomes visible.
- Submission failure leaves Engage open with the draft preserved.
- Full transcript, permission details, and multiline authoring remain in the center.

### Mode 2: Orbit

Purpose: switch the center transcript to another session.

- Invoked by clicking the session identity or pressing `⌘P`.
- Search filters session metadata only.
- Do not show Activity or history passages.
- Avoid a long list: render a **three-card orbit**:
  - selected session is the dominant center card;
  - previous and next candidates appear as shallow edge peeks;
  - result position appears as `3 of 12`.
- Typing narrows the orbit; arrows rotate it.
- Enter calls `Daemon::switch_session` and collapses to Ambient.
- Mouse users click an adjacent peek to promote it, then click/Enter the center card to switch.

### Mode 3: Recall

Purpose: answer “where did we discuss or decide this?”

- Invoked by `⌘⇧F` or `Search history…` in `⌘K`.
- Use a daemon-owned hybrid lexical/fuzzy plus semantic search contract.
- Results are typed history passages, not session rows:
  - bounded matching excerpt;
  - session/project provenance;
  - speaker or summary kind;
  - date;
  - mentioned file/topic when authoritative.
- Display one dominant passage with previous/next edge peeks and result count.
- Enter opens its source session through the existing focus path.
- Exact turn scrolling is a later gate unless the daemon supplies stable turn/event anchors.
- Recall never submits a prompt or changes sessions merely because the query changed.

### Mode 4: Attention

Purpose: resolve or inspect live work needing operator awareness.

- Invoked only by the trailing badge or a command.
- Render one authoritative work item at a time, ordered:
  `NeedsHuman → Failed → Cancelling → Running`.
- Left/right or up/down cycles items.
- Enter expands the selected item in place.
- Approve, Deny, Stop, and Open Session retain their existing authority rules.
- Do not append the session catalogue beneath the card.

### Geometry and transitions

- Ambient-to-mode transition originates from the invoked capsule region.
- Width expands before content fades in; height follows content.
- No full-window dimming scrim.
- Clicking elsewhere collapses the nonmodal anchored stage.
- Desktop maximum expanded height: approximately `320px`.
- Narrow windows use a safe-width top stage capped near `42dvh`, never the current near-full-height session list.
- Reduced-motion mode removes interpolation but preserves state changes.

### Keyboard model

| Key | Context | Behavior |
|---|---|---|
| `⌘P` / `Ctrl+P` | Tauri, anywhere | Open/toggle Orbit |
| `⌘⇧F` / `Ctrl+Shift+F` | Tauri, anywhere | Open/toggle Recall |
| `⌘K` | Anywhere | Commands; closes any Island mode |
| `Escape` | Any expanded mode | Collapse to Ambient and restore invoking focus |
| `Enter` | Engage input | Submit focused-session prompt |
| `Enter` | Orbit | Switch selected session |
| `Enter` | Recall | Open selected source session |
| `Enter` | Attention card | Expand/collapse selected item |
| `ArrowUp/Down` | Orbit/Recall | Previous/next result |
| `ArrowLeft/Right` | Attention | Previous/next work item |
| `Tab` | Engage/Attention | Traverse only actual controls |
| `Shift+Tab` | Expanded mode | Reverse control traversal |

IME composition must bypass all global and mode-local shortcuts.

## Tasks

1. **Supersede the existing interaction specification**
   - Files:
     - `docs/OCEAN_DYNAMIC_ISLAND_IMPLEMENTATION.md`
     - `docs/OCEAN_DYNAMIC_ISLAND_BUILD_PLAN.md`
   - Changes: Replace Browse/Search unification with Ambient, Engage, Orbit, Recall, and Attention contracts; record keyboard routing, authority boundaries, and phased backend dependencies.
   - Acceptance: Documentation explicitly forbids rendering Activity, session switching, and history recall in one combined surface.

2. **Introduce the mutually exclusive Island state model**
   - File: `crates/ocean-surface-ui/src/island.rs`
   - Changes: Replace `Closed/Browse/Search` with an ambient state plus distinct expanded modes; centralize entry, exit, focus restoration, and mode-transition rules.
   - Acceptance: At most one mode component exists in the DOM at a time.

3. **Rebuild the compact capsule as semantic regions**
   - Files:
     - `crates/ocean-surface-ui/src/island.rs`
     - `styles/island.css`
   - Changes: Add separate accessible hit targets for Engage, Orbit, and Attention while preserving one visual capsule; remove quiet “recent” count.
   - Acceptance: Each region has an accurate accessible name and opens only its assigned mode.

4. **Extract existing attention behavior into its own mode**
   - New file: `crates/ocean-surface-ui/src/island/attention.rs`
   - Changes: Move projections, disclosures, approval/deny, stop, and open-session rendering out of the shared popover; present one selected item at a time.
   - Acceptance: Attention contains no session search input or session catalogue.

5. **Implement focused-agent Engage mode**
   - New file: `crates/ocean-surface-ui/src/island/engage.rs`
   - Files additionally modified:
     - `crates/ocean-surface-ui/src/island.rs`
     - `crates/ocean-surface-ui/src/daemon.rs`
   - Changes: Render bounded focused-agent state and a compact prompt input; reuse `Daemon::send_prompt`; preserve drafts on errors; prohibit background submission.
   - Acceptance: Submitting creates/posts only to the focused session and the center transcript remains authoritative.

6. **Convert session browsing into Orbit**
   - New file: `crates/ocean-surface-ui/src/island/orbit.rs`
   - Files additionally modified:
     - `crates/ocean-surface-ui/src/island.rs`
     - `crates/ocean-surface-ui/src/search.rs`
   - Changes: Retain metadata derivation and fuzzy scoring, but render the selected session plus adjacent peeks instead of a vertical list.
   - Acceptance: `⌘P`, typing, arrows, and Enter switch sessions without mounting Attention or Recall content.

7. **Define the daemon-owned history-search contract**
   - Repository dependency: `../ocean-os`
   - Changes: Add a hybrid history search endpoint returning typed, bounded hits with `session_id`, title, excerpt, provenance, timestamp, match kind, and optional stable turn/event anchor. Index prompts, assistant summaries, mentioned files, topics, and unresolved threads according to daemon policy.
   - Acceptance: The Surface sends a query and renders daemon results; it performs no embeddings, provider calls, or independent history indexing.

8. **Implement Recall mode after the daemon contract exists**
   - New file: `crates/ocean-surface-ui/src/island/recall.rs`
   - Files additionally modified:
     - `crates/ocean-surface-ui/src/daemon.rs`
     - `crates/ocean-surface-ui/src/island.rs`
   - Changes: Add debounced/cancellable search requests, loading/error/empty states, passage carousel, and source-session opening.
   - Acceptance: Recall returns content matches that cannot be found from session metadata alone and never submits prompts.

9. **Update application-level routing**
   - File: `crates/ocean-surface-ui/src/app.rs`
   - Changes: Route `⌘P` to Orbit, `⌘⇧F` to Recall, add command-registry entries for Engage/Orbit/Recall/Attention, preserve `⌘K` precedence, and update Escape ordering.
   - Acceptance: Every shortcut opens exactly one mode and browser/PWA Print/Find behavior remains unchanged off-Tauri.

10. **Replace modal geometry with an elastic anchored stage**
    - File: `styles/island.css`
    - Changes: Remove the dimming scrim and list-dashboard geometry; add per-mode size constraints, orbit/passages peeks, narrow-state caps, focus states, and reduced-motion behavior.
    - Acceptance: Wide and narrow screenshots show a bounded titlebar extension rather than a centered modal or full-height list.

11. **Add interaction and model coverage**
    - Files:
      - `crates/ocean-surface-ui/src/island.rs`
      - `crates/ocean-surface-ui/src/island/engage.rs`
      - `crates/ocean-surface-ui/src/island/orbit.rs`
      - `crates/ocean-surface-ui/src/island/recall.rs`
      - `crates/ocean-surface-ui/src/island/attention.rs`
    - Changes: Test state transitions, exclusive mounting, prompt authority, metadata ranking, history result handling, attention authorization, keyboard selection, and focus restoration.
    - Acceptance: Tests prove no action from one mode can accidentally trigger another mode’s primary action.

12. **Validate in the real Tauri shell**
    - Changes: Capture wide and narrow screenshots for each expanded mode; exercise composer/header/workbench focus origins; test screen-reader names and reduced motion.
    - Acceptance: Four modes are visually and behaviorally distinguishable without tabs, drawers, or a combined dashboard.

## Files to Modify

- `docs/OCEAN_DYNAMIC_ISLAND_IMPLEMENTATION.md` — corrected interaction contract.
- `docs/OCEAN_DYNAMIC_ISLAND_BUILD_PLAN.md` — phased delivery plan.
- `crates/ocean-surface-ui/src/island.rs` — compact shell and exclusive mode state machine.
- `crates/ocean-surface-ui/src/app.rs` — global routing and overlay precedence.
- `crates/ocean-surface-ui/src/daemon.rs` — focused prompt and history-search client integration.
- `crates/ocean-surface-ui/src/search.rs` — Orbit metadata search only.
- `styles/island.css` — elastic stage and per-mode geometry.

## New Files

- `crates/ocean-surface-ui/src/island/engage.rs` — focused-agent interaction.
- `crates/ocean-surface-ui/src/island/orbit.rs` — session switching.
- `crates/ocean-surface-ui/src/island/recall.rs` — history retrieval.
- `crates/ocean-surface-ui/src/island/attention.rs` — authoritative work triage.

## Dependencies

- Tasks 2–3 depend on Task 1’s mode contract.
- Tasks 4–6 depend on the exclusive state shell from Task 2.
- Task 8 is blocked by Task 7’s daemon API.
- Task 9 depends on stable mode entry callbacks from Tasks 2 and 5–8.
- Styling and final validation depend on all mode components being independently functional.

## Risks

- `/Users/smathdaddy-macbook/ocean-surface/context.md` was unavailable, so the correction was taken from the task text itself.
- Semantic history search belongs to `ocean-os`/Bedrock; implementing embeddings or a divergent local index in this repository would violate platform ownership.
- Exact jump-to-turn behavior requires stable daemon-provided anchors; session-level opening should ship first if those identifiers are unavailable.
- Engage must reuse focused-session submission semantics and must not create a second transcript/runtime.
- Removing the modal scrim requires deliberate outside-click and focus-restoration handling.
- Three compact hit regions must remain large enough for pointer access without visually becoming three separate toolbar buttons.