## Review

- **Correct:** `npm run build` passed TypeScript and Vite production build.
- **Correct:** Projectile IDs are monotonic (`laserSequence`), and enemy/laser registries use stable IDs with cleanup.
- **Correct:** Reset clears gameplay refs/state and remounts the R3F canvas.
- **Blocker:** `src/game/Game.tsx` (`Simulation` hook) checks collisions before `Enemy` and `Laser` hooks advance their positions. `sweptCapsuleHits` sweeps only the projectile against the target’s stale point, so fast head-on projectile/enemy crossings can tunnel and miss despite the swept helper.
- **High:** `src/game/Laser.tsx` advances using raw `delta`, while `Simulation` caps resumed deltas after visibility pauses. On returning from a hidden tab, a laser can jump/expire across the scene using a multi-second delta, violating the pause/resume bound.
- **Medium:** `src/game/Enemy.tsx` registers the carrier collision point at the group origin, but the visual core is offset to local `z = -4.2` inside a `scale={1.5}` group. The comment claims the core is the hit area, but collision is actually centered on the carrier body.
- **Medium:** `src/game/Game.tsx` clears keyboard/touch input on `window.blur` but does not pause simulation. Losing focus while `document.hidden` remains false leaves the game running without input.
- **Note:** `damagePlayer` performs state updates and mutates `gameOver` inside the `setShield` updater. React Strict Mode may invoke updater functions more than once, causing duplicate terminal cleanup calls. No build failure observed.
- **Note:** Vite reports a production chunk larger than 500 kB; non-blocking performance risk.

No files were changed.