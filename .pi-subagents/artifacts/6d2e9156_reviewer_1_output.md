## Review

### Must-fix interaction problems

- **High — The implementation specification itself encodes a list/popover product.**  
  `docs/OCEAN_DYNAMIC_ISLAND_IMPLEMENTATION.md:25-31,100-110,405-417` explicitly prescribes an overlay, session list, dialog, scrim, section headers, summary rows, and disclosures. That conflicts with the North Star’s distinction between an ambient attention router and utility browse surfaces (`docs/OCEAN_DESKTOP_NORTH_STAR.md:73-82,100-119,180-191`). Cosmetic refinement cannot resolve this; the interaction specification must change first.

- **High — Opening the Island replaces the living object with a conventional modal dialog.**  
  `island.rs:829-917` keeps the chip as one button while mounting a separate scrim and `aria-modal` popover below it. `styles/island.css:129-153` dims the entire application and renders a fixed elevated rectangle. There is no structural continuity or shared work object between collapsed and expanded states. Both screenshots consequently read as “button opens dropdown/modal,” not “work object expands.”

- **High — The expanded information architecture is catalogue-first, not work-first.**  
  `island.rs:571-577` returns every session for Browse, and `island.rs:1151-1225` renders them as a standard listbox of metadata rows. In `/tmp/ocean-island-final-wide.png`, one running item is followed by a long session history including stale Longhouse sessions and an untitled zero-turn draft. The browse catalogue dominates the active work, directly reversing the North Star rule that browsing is utility rather than the signature interaction.

- **High — One agent job is split into duplicate records instead of represented as one interaction object.**  
  Attention and sessions are derived independently at `island.rs:558-568`, then rendered in separate sections at `island.rs:918-1149` and `1151-1225`. Both screenshots show `status?` once as a Running activity row and again as the Focused session row. This destroys object identity: state, context, and destination should be facets of one workstream, not adjacent list entries.

- **High — “Activity” is explicitly an accordion with drawers.**  
  Each attention item is a summary button with chevron and hidden detail panel (`island.rs:1004-1143`); CSS gives it a row grid and disclosure treatment (`styles/island.css:195-327`). Opening or acting on background work therefore requires: open Island → find row → expand drawer → find action. That is the interaction grammar of a settings/list popover, not a direct attention router.

- **High — Browse and Search are functionally almost the same mode.**  
  The search field is always first and receives focus whenever either mode opens (`island.rs:579-608,895-917`). The only meaningful mode difference is that Search truncates to 20 results (`island.rs:571-575`). Click-to-browse therefore launches a command palette rather than exposing active agent work. Search should be a deliberate `⌘P` transformation, not the default anatomy of the Island.

- **Medium — Search and activity are disconnected.**  
  The query filters only `sessions` (`island.rs:571-577`), while `attention_items` remain unfiltered (`island.rs:918-934`). A project/session query can leave unrelated activity pinned above the matching result. This reinforces the impression of two stacked drawers and prevents `⌘P` from answering “what active work needs me?” as required by `OCEAN_DESKTOP_NORTH_STAR.md:199-206`.

- **Medium — Browse does not implement its specified bounded scope.**  
  The specification requires eight initial Browse rows (`docs/OCEAN_DYNAMIC_ISLAND_IMPLEMENTATION.md:442-445`), but only Search is truncated (`island.rs:571-575`). The screenshots show the resulting long scrolling history. At narrow width, `/tmp/ocean-island-narrow.png` becomes an almost full-screen session browser, visually overwhelming both the titlebar object and workspace.

- **Medium — The compact state aggregates away the “orbiting work” model.**  
  `island.rs:797-879` exposes only focused title plus one aggregate status/count. It provides no representation or targeting of the highest-priority background work despite the North Star asking for focused and active-background sessions (`docs/OCEAN_DESKTOP_NORTH_STAR.md:130-137`). “1 running” is telemetry, not an agent interaction object.

### Structural recommendation

1. **Make click-open a work stack, not a session browser.**
   - Build one `IslandWorkObject` per session/workstream.
   - Merge focused-session identity, authoritative request state, permission state, concise current activity, and available action into that object.
   - Show only focused work plus urgent/running/failed background work.
   - Never show duplicate Activity and Session records for the same session.

2. **Separate the two postures.**
   - **Click/Browse:** connected, nonmodal active-work expansion; no default search field or full catalogue.
   - **`⌘P` Search:** transform the same surface into search mode, temporarily replacing the work stack with ranked results.
   - Put inactive history behind a clear “Find another session…” transition or retain the existing Sessions utility surface.

3. **Preserve continuity between compact and expanded states.**
   - Expanded content should grow from the chip’s bounds and retain the focused object as its anchor.
   - Do not use a full-window scrim or modal focus posture for ambient click-open behavior.
   - A modal posture may remain appropriate for explicit keyboard search.

4. **Expose the state-relevant next move directly.**
   - Clicking a work object should focus/open that session in one move.
   - Needs-human objects should expose the authorized decision affordance immediately.
   - Running objects should present current work plus a secondary Stop action.
   - Disclosure can remain for optional evidence, but must not be the gateway to the primary action.

5. **Bound the object at narrow sizes.**
   - Show the focused object plus the highest-priority one or two background items and a remainder count.
   - Do not turn narrow expansion into a full-height history browser.

### Useful pieces to retain

- **Correct:** Authoritative request/permission projection and explicit prohibition on timestamp-inferred activity are sound (`docs/OCEAN_DYNAMIC_ISLAND_IMPLEMENTATION.md:50-83`; `island.rs:112-209`).
- **Correct:** Needs-human/running compact count priority is truthful (`island.rs:797-809`).
- **Correct:** Permission actions retain submitter token and focused-session authorization boundaries; Stop tracks in-flight cancellation without inventing state (`island.rs:972-1003,1051-1139`).
- **Correct:** `Daemon::switch_session` remains the focus path (`island.rs:1061-1069,1197-1203`).
- **Correct:** Shared fuzzy search, deterministic ordering, keyboard handling, focus restoration, IME guards, and overlay mutual exclusion are reusable foundations (`island.rs:579-787`; `app.rs:341-366,778-832`).
- **Correct:** The visual treatment is restrained and token-based; the fundamental problem is structure, not color or polish.

### Residual risks

- `plan.md` and `progress.md` were absent at the requested paths, so stated intent was assessed from the implementation specification, build plan, North Star, source, and supplied screenshots.
- Existing request snapshots provide generic details such as “agent turn running.” A compelling work object may require richer daemon-owned typed summaries/progress rather than UI inference.
- Screenshots cover only a Running state; permission, failure, cancellation, and expanded-action states were assessed from source/CSS rather than live visual captures.
- Targeted unit tests validate projection/search logic, not the product interaction or native focus behavior.