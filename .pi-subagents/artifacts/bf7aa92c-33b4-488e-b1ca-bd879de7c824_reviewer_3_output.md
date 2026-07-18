## Review
- Correct: `npm run build` passes; lockfile matches declared dependencies. Scene geometry/assets are procedural, with no shipped proprietary game assets.
- Blocker: `Game.tsx:149-174`, `README.md:17-18` — controls are keyboard-only; no touch/pointer controls exist, leaving mobile users unable to play.
- Blocker: `Game.tsx:149-174` — no blur/visibility handler clears held keys. Losing focus while holding Space, Shift, or movement can leave input latched.
- Blocker: `Game.tsx:118,245-248,266-280` — `bossHealth` is updated but never rendered. The carrier has no persistent health indicator.
- Blocker: `Game.tsx:266-284`, `src/game/difficulty.ts:24-67` — non-carrier health is scaled but ignored; every heavy/turret dies on one hit, contradicting the difficulty model.
- Blocker: `Game.tsx:344-383` — HUD/status/mission and end overlay lack accessible landmarks, live-region semantics, dialog semantics, and shield value semantics.
- Note: `package.json:5-9` has no test script and no tests cover difficulty, input cleanup, or reset behavior.
- Note: `styles.css:1` imports Google Fonts remotely, so the README’s standalone/asset-free claim is not fully offline/self-contained.
- Note: Build emits a bundle-size warning: JS output is 1,154.63 kB minified.