## Review

- **Correct:** The session picker uses a coherent combobox/listbox pattern: the search input owns `aria-controls`/`aria-activedescendant` (`crates/ocean-surface-ui/src/island.rs:820-839`), and each non-Tabbable session option exposes `role="option"` plus reactive `aria-selected` (`crates/ocean-surface-ui/src/island.rs:1003-1015`). Arrow/Enter handling is correctly scoped to the search input, so Enter on a disclosure cannot switch a session (`crates/ocean-surface-ui/src/island.rs:644-695`). Disclosure summaries are native buttons with `aria-expanded` and `aria-controls` (`crates/ocean-surface-ui/src/island.rs:893-924`). The popdown remains one dialog/material surface, uses domain tokens rather than literal colors, avoids pulses, kills its transitions under reduced motion, and keeps row controls conditional.

- **Blocker:** The Tab trap is attached only to `.island-host` and therefore only handles key events whose focused target is still inside that subtree (`crates/ocean-surface-ui/src/island.rs:586-640,754`). Activity is polled and keyed rows are removed when a request completes (`crates/ocean-surface-ui/src/island.rs:491-493,856-967`). If the removed row or its `Open session` button held focus, focus falls outside the host; the next Tab never reaches this handler and can escape the `aria-modal` dialog. **Smallest fix:** install the Tab trap at `window`/`document` while the Island is open (and remove it on cleanup), or add an effect after attention updates that detects `activeElement` no longer contained by `popover_ref` and immediately focuses the search input. For a window-level trap, an outside current target must wrap to the first item (last for Shift+Tab), not default to index 0 and advance to item 2. Add a browser-level regression that removes the focused attention row and then presses Tab.

- **Important:** Compact counts do not implement the binding projection. `needs_human_count` includes `Failed`, while `running_count` includes `Cancelling` (`crates/ocean-surface-ui/src/island.rs:494-517`), so the chip and announcement can claim a failed item “needs attention” or a cancelling item is “running” (`crates/ocean-surface-ui/src/island.rs:711-734`). Neither claim matches the specified compact priority, and no mutation exists in the Island to resolve a failed item. **Smallest fix:** count only `NeedsHuman` for attention and only `Running` for running; failed/cancelling remain visible in Activity but do not alter those compact counts. Add pure tests for failed-only and cancelling-only snapshots.

- **Important:** The compact status is visually truncated at supported desktop widths. At `<=840px` it is capped to `7ch`, and at the Tauri minimum width of 720px it is hidden entirely (`styles/island.css:540-575`; `crates/ocean-tauri/tauri.conf.json:19`). This drops the required title-plus-count state exactly at the supported narrow boundary, including the highest-priority “need attention” count. **Smallest fix:** remove the `max-width: 7ch` and `display: none` rules for `.island-chip__count`; keep project/separators hidden and let the title be the flex item that truncates first. Validate failed/running/recent strings at 720, 840, split-workspace, and wide widths.

- **Important:** The live region does not announce the same trailing state as the chip. It emits separate wording only for attention/running and becomes empty for the recent fallback (`crates/ocean-surface-ui/src/island.rs:711-734,805-807`); clearing a live region generally does not announce that attention ended, so assistive-technology users receive no replacement “N recent” state. **Smallest fix:** render the exact `chip_status()` text (or one shared status formatter) in the polite atomic live region, after correcting the state filters above. Keep the focused title only in the chip label to avoid duplicate verbose announcements.

- **Important:** The default focused-session dot is accent cyan (`styles/island.css:65-91`), the same treatment required for actual running work. That makes idle/focused and running states visually indistinguishable by dot and spends the design system’s live accent on a non-live state. **Smallest fix:** give the base dot a neutral token (for example `--fg-3`), retain `--warn` only for attention, `--accent` only for `.is-running`, and the existing empty treatment for no session.

- **Important:** The bounded disclosure reason creates an inner scroll area that is not keyboard-focusable (`styles/island.css:320-331`). At narrow/split widths, 240 characters can exceed `5.4em`; mouse/touch users can scroll the paragraph, but keyboard users cannot focus it, while the surrounding focus trap deliberately enumerates only the input and buttons (`crates/ocean-surface-ui/src/island.rs:603-606`). **Smallest fix:** remove the paragraph’s `max-height`/`overflow` and let the already bounded text expand inside the focusable attention-list scroll container. Avoid adding another Tab stop unless the nested scroll area must remain.

- **Note:** `/Users/smathdaddy-macbook/ocean-surface/plan.md` and `progress.md` were requested inputs but do not exist, and no same-named files were found within three repository levels. Review proceeded against the two binding docs and current source. No live Tauri/VoiceOver run was available, so focus restoration, screen-reader announcements, and 720px/split geometry still require manual validation.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Performed a review-only inspection of island.rs and styles/island.css against the two requested design/spec documents; no project or source files were modified."
    },
    {
      "id": "criterion-2",
      "status": "satisfied",
      "evidence": "Findings cite exact source/CSS lines, explain observable impact, and provide bounded smallest fixes; focused and full host-target test results are recorded below."
    }
  ],
  "changedFiles": [],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    {
      "command": "cargo test -p ocean-surface-ui --bin ocean-surface-ui island",
      "result": "passed",
      "summary": "11 Island unit tests passed; 0 failed."
    },
    {
      "command": "cargo test -p ocean-surface-ui --bin ocean-surface-ui",
      "result": "passed",
      "summary": "323 UI binary tests passed; 0 failed."
    },
    {
      "command": "cargo check -p ocean-surface-ui --target wasm32-unknown-unknown",
      "result": "blocked",
      "summary": "Could not run because wasm32-unknown-unknown is not installed in this environment (E0463)."
    },
    {
      "command": "cargo test -p ocean-surface-ui island --lib",
      "result": "failed",
      "summary": "Invalid initial invocation: this Trunk package has no library target; rerun correctly with --bin and passed."
    },
    {
      "command": "grep -nE '#[0-9a-fA-F]{3,8}|rgb\\(|rgba\\(' styles/island.css; grep -nE 'animation:|transition:' styles/island.css",
      "result": "passed",
      "summary": "No literal hex/rgb colors or animations found; all four transitions have a reduced-motion override."
    },
    {
      "command": "git diff --check -- crates/ocean-surface-ui/src/island.rs styles/island.css; git diff --cached --name-only",
      "result": "passed",
      "summary": "No whitespace errors in reviewed files and no staged files."
    }
  ],
  "validationOutput": [
    "Island tests: 11 passed, 0 failed.",
    "Full ocean-surface-ui binary tests: 323 passed, 0 failed.",
    "Reviewed listbox/combobox ownership, disclosure ARIA, focus loop, live-region output, compact priority, responsive media rules, token use, motion fallback, and conditional control density.",
    "No staged files were present."
  ],
  "residualRisks": [
    "No live Tauri keyboard, VoiceOver, or accessibility-tree validation was run.",
    "The wasm check remains unverified because the wasm32 target is unavailable.",
    "Requested plan.md and progress.md inputs were absent."
  ],
  "noStagedFiles": true,
  "diffSummary": "Review-only task; no project/source diff was created. Findings were written only to the required /tmp artifact.",
  "reviewFindings": [
    "blocker: crates/ocean-surface-ui/src/island.rs:586-640 - host-scoped Tab handler loses the trap if polling removes the focused activity control",
    "important: crates/ocean-surface-ui/src/island.rs:494-517 - failed and cancelling states are miscounted as attention and running",
    "important: styles/island.css:540-575 - required compact status is truncated/hidden at narrow supported widths",
    "important: crates/ocean-surface-ui/src/island.rs:724-734 - live region omits the recent fallback and does not share the chip status",
    "important: styles/island.css:65-91 - idle focused-session dot incorrectly uses the running/live accent",
    "important: styles/island.css:320-331 - inner disclosure reason overflow is not keyboard-focusable"
  ],
  "manualNotes": "No source edits were made. Review artifact: /tmp/ocean-phase2-a11y-review.md."
}
```
