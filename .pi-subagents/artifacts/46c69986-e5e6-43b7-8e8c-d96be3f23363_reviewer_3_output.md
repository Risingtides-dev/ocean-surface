## Review

- **Correct**
  - `src/game/difficulty.ts:14-44` matches documented caps: health `1.9x`, fire interval `0.55x`, projectile speed `63`, accuracy `94%`.
  - `src/game/difficulty.ts:55-75` clamps predicted aim to the playable envelope.
  - `src/game/types.ts:3` defines all three teammates: `lyra`, `rook`, and `pip`.
  - Scene geometry is procedurally authored; no image, model, texture, or audio assets are present.
  - `package.json:5-9` provides type-checking before Vite builds.

- **Blocker**
  - None identified.

- **Note**
  - **Medium — `src/game/difficulty.ts:80-113`:** documented wave density reaches `1.5x`, but a three-enemy wave produces only four enemies at maximum (`floor(0.5 × 3) = 1` extra), or `1.33x`. Carrier-selected extra slots can also be skipped entirely. HUD telemetry labeled `1.5x` would not describe actual spawn count.
  - **Medium — `src/game/Enemy.tsx:54-57`:** all `turret` variants are explicitly excluded from firing. The visual is a turret and the variant participates in threat waves; threat telemetry should distinguish spawned hazards from active attackers.
  - **Medium — `README.md:5-8,18-25`, `src/game/Arwing.tsx`:** the claimed “does not ship Nintendo names” boundary is contradicted by the `Arwing` source/component name and README references to *Star Fox 64*, Corneria, and Attack Carrier, plus fan-wiki links. Assets are original, but the naming/documentation boundary is not legally clean.
  - **Low — `package.json:5-9`:** no `test` or `lint` script exists, so difficulty, control, accessibility, and teammate regressions are unaudited automatically.
  - **Low — `src/styles.css:1,82`:** production CSS depends on Google Fonts, and button styling adds `:hover` feedback but no explicit `:focus-visible` treatment. Keyboard accessibility and offline/privacy polish are therefore weaker than the visual treatment.
  - `src/styles.css` has no reduced-motion accommodation; this is a residual accessibility concern for a highly animated game.

Generated `dist/assets/index-WLvu7lS6.js` and `package-lock.json`, along with the 603-line `src/game/Game.tsx`, were identified in the inventory but not semantically reviewed before the instructed browsing cutoff.