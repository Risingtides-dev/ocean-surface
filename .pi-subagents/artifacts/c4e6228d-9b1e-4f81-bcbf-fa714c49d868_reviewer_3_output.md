## Review

- Correct:
  - Rendering is reasonably modular: `Arwing.tsx`, `Enemy.tsx`, `Laser.tsx`, `Terrain.tsx`, `SceneEffects.tsx`, plus pure `collision.ts` and `difficulty.ts`.
  - `package.json:5-9` provides typechecking before Vite builds.
  - `Game.tsx` correctly updates teammate states when targeted enemies spawn, are destroyed, or escape.
  - Visibility/blur handlers clear held inputs and pause gameplay (`Game.tsx`, input effect).

- Blocker: None.

- Note:
  - **High — HUD threat is misleading:** `Game.tsx` HUD renders `THREAT` from `normalizedLevelProgress(distance)`, which is route distance, not active enemy threat. At mission start, three enemies are spawned while HUD says `THREAT 0%`. Rename it to route progress or implement actual threat tracking.
  - **Medium — Boss phase is not user-visible:** `Enemy.tsx` computes three carrier phases around lines 30–42 and changes cadence/appearance, but `Game.tsx` only displays armor totals. Add `PHASE 1/2/3` and announce phase transitions.
  - **High — Remote font breaks offline/local requirements:** `src/styles.css:1` imports Google Fonts. Remove the network import and use a system stack or checked-in WOFF2 with `@font-face`.
  - **Medium — Reduced motion is unhandled:** `src/styles.css` has no `prefers-reduced-motion` rule. Continuous plume, reticle, barrel-roll, camera/FOV, terrain, bloom, and carrier animations remain active for motion-sensitive users.
  - **Medium — Focus styling is incomplete:** `src/styles.css:17-18` styles only `.game-shell:focus-visible`; overlay and touch buttons have hover styling but no explicit `:focus-visible` treatment.
  - **Medium — README naming is not legal-safe:** `README.md:3-5,28-45` repeatedly names *Star Fox 64*, Nintendo, and links to fan wikis/manual archives. Replace with generic inspiration language and remove branded reference links from the user-facing README.
  - **Low — Mobile controls are undocumented:** `README.md:13` documents only keyboard controls, although `Game.tsx` includes touch controls. Document touch actions and the lack of remapping.
  - **Medium — Testability is weak:** `package.json:5-9` has no `test` or `lint` script, and `README.md` explicitly states there is no test framework. Add unit tests for `collision.ts` and `difficulty.ts`, plus a CI-friendly test command.
  - **Low — Orchestration file is oversized:** `Game.tsx` contains mission data, runtime state, HUD, touch controls, simulation, and scene composition. Extract mission configuration, `Hud`, `TouchControls`, and `Simulation` modules. The top-level `encounterCursor`, `lastShot`, and `lastHudUpdate` refs are unused duplicates.
  - **Low — Reproducibility documentation:** `README.md:10` recommends `npm install` despite the checked-in lockfile. Prefer `npm ci` for deterministic setup and add a lockfile validation step.

- Fixed: None; review was read-only.