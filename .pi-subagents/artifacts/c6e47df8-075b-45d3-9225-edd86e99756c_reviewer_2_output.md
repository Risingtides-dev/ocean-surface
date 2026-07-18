## Review

- **Correct:** The Dynamic Island is runtime-mounted only in Tauri via `running_in_tauri()` and `<Show when=move || in_tauri>` (`crates/ocean-surface-ui/src/app.rs:1198,1940-1947`).
- **Correct:** Browser Print/Find shortcuts are not prevented off-Tauri: `island_shortcut_mode` returns `None` unless `in_tauri`, and `prevent_default()` occurs only for a returned mode (`crates/ocean-surface-ui/src/app.rs:54-71,1658-1680`). Unit coverage exists at `app.rs:2928-2957`.
- **Correct:** Island modes are mutually exclusive in the rendered tree, while `open_island` also closes palette, council, rooms, sessions, and an active Voice Planner (`app.rs:1200-1232`; `island_dynamic.rs:665-850`).
- **Correct:** Non-color state cues accompany color: “Needs attention”, “Failed”, “Stopping”, and “Running” remain visible text (`island.rs:42-67`; `island_dynamic.rs:214-250`). Focused sessions also receive a visible “Focused” label (`island_dynamic.rs:731-735`).
- **Correct:** Pending-composer geometry repair is coherent: composer is fixed out of flex negotiation, transcript is the only flexible child with `min-height:0`, and pending Soundings aligns to the assistant rail (`styles/composer.css:9-28`; `styles/transcript.css:163-171,884-890`; `styles/workspace.css:71-79`).
- **Correct:** Voice Planner and Island selectors are distinct, and stylesheet order places `island.css` before composer/Planner styles consistently in Trunk and the extension (`index.html:36-43`; `extension/sidepanel.html:9-16`).
- **Correct:** Session/Recall comboboxes expose `aria-controls`, `aria-activedescendant`, listbox/option roles, and selected state (`island_dynamic.rs:679-758,772-845`).

- **Blocker (high): Browser/extension Sessions behavior was widened contrary to the Island contract.**  
  `group_for_panel` now exposes every stale zero-turn session on all hosts (`crates/ocean-surface-ui/src/sessions.rs:150-164`), and its test was changed to require that behavior (`sessions.rs:1236-1273`). This directly conflicts with “browser/extension Sessions behavior is unchanged” (`docs/OCEAN_DYNAMIC_ISLAND_BUILD_PLAN.md:82-90`) and the project’s lazy-session contract.  
  **Smallest safe fix:** restore the prior active-zero-turn-only filter in `group_for_panel`. The Island already derives directly from `daemon.session_list`, so its required zero-turn discoverability remains intact. Revert/update the shared-panel test accordingly.

- **Note (medium): Keyboard selection does not scroll-follow in bounded result panes.**  
  Arrow keys update `session_selected`/`history_selected` (`island_dynamic.rs:474-534`), but there is no `scrollIntoView` or row-ref effect. Once selection moves beyond the visible portion of the bounded containers (`styles/island.css:167-173,720-748`), keyboard users can operate an off-screen option.  
  **Smallest safe fix:** after selected ID changes, locate that option and call `scroll_into_view` with block `"nearest"` and no smooth animation.  
  **Manual validation:** populate more than eight/viewport-height results and hold ArrowDown/ArrowUp in both Sessions and Recall at wide and 720px widths.

- **Note (medium): Agent prompt submission does not fully implement the documented IME bypass.**  
  The Island host key handler returns during composition, but the idle Agent is a native form whose submit handler has no composition guard (`island_dynamic.rs:122-163`). A composition-confirming Enter can still reach form default submission on affected WebKit/IME combinations. The implementation contract explicitly says all local handling must bypass IME (`docs/OCEAN_DYNAMIC_ISLAND_IMPLEMENTATION.md:285-294`).  
  **Smallest safe fix:** track `compositionstart`/`compositionend` for the Agent field and reject form submission while composing; add a pure/unit seam similar to the main composer helper.  
  **Manual validation:** macOS Japanese and Chinese IMEs in Tauri: confirming a candidate must not send or close the Island; a subsequent plain Enter must send once.

- **Note (medium): Recall results lose the matched content from their accessible names.**  
  `history_result_label` supplies only “Open history match from {session}” (`island_dynamic.rs:48-56`), and that `aria-label` overrides the visible excerpt/role metadata (`island_dynamic.rs:805-833`). Multiple matches from one session are therefore indistinguishable to a screen-reader user.  
  **Smallest safe fix:** use `aria-labelledby`/`aria-describedby` tied to visible title, excerpt, role, and provenance, or include a bounded excerpt and role in the label.

- **Note (medium): Recall loading, empty, and error updates are not reliably announced.**  
  “Searching history…”, “No transcript matches”, and errors are ordinary `<div>` children inside the `role="listbox"` (`island_dynamic.rs:772-799`). They have neither status/alert semantics nor valid option roles.  
  **Smallest safe fix:** move status/error content outside the listbox; use `role="status" aria-live="polite"` for searching/empty and `role="alert"` for errors.

- **Note (medium): Focus restoration can race a newly opened surface.**  
  Closing Island unconditionally schedules restoration of the previous element (`island_dynamic.rs:391-399`), while Palette and Voice Planner independently request/autofocus their controls (`palette.rs:267-277`; `app.rs:879-910,1780-1806`). “Close Island then open another intent” can therefore produce competing asynchronous focus writes.  
  **Smallest safe fix:** distinguish dismissal (restore) from replacement by another explicit opener (do not restore), or verify before restoring that focus is still inside/at the closing Island.  
  **Manual validation:** from composer, open Island and then use Cmd/Ctrl+K; final focus must remain in Palette. Repeat Island→Planner and verify the Planner project control owns focus.

- **Note (low): Island CSS is not entirely inert off-Tauri.**  
  Although the component does not mount, `styles/island.css:9-17` globally changes `.ocean-header` and `.ocean-header__left`. This is broader than the documented “selectors stay inert when the Island is absent” contract.  
  **Smallest safe fix:** move general header wrapper styling to `chrome.css` and scope Island-only header positioning to `.is-titlebar`.

- **Note:** No native wide/720px screenshots, screen-reader run, IME run, extension package build, or live browser shortcut validation were available in this review. The requested `plan.md` and `progress.md` do not exist at the supplied paths.
- **Note:** Scope cannot be cleanly accepted: the integrated tree also introduces an unrelated 867-line `interactive_plot` component and associated shared component/CSS work, while the available Dynamic Island plan’s file map does not identify that feature.

## Validation

- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Staging area is empty.
- WASM check failed because `wasm32-unknown-unknown` is not installed.
- UI tests and Tauri check timed out while compiling/contending for build locks; neither produced a completed validation result.