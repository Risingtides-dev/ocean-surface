## Review
- Correct:
  - `src/game/difficulty.ts:21-36` correctly clamps normalized progress to `[0,1]`; endpoint multipliers are health `1.9x`, cadence `0.55x`, projectile speed `63`, lead `1.0`, accuracy `94%`, density `1.5x`, formation `1.22x`.
  - Targeting is deterministic: `src/game/Enemy.tsx:51-60` uses player position/velocity, bounded prediction, and seeded error; no runtime randomness.
  - Boss expiry and teammate rescue paths are wired in `src/game/Game.tsx:217-229`; kills restore threatened teammates and expiry marks them down.
  - `npm run build` passed (`tsc --noEmit` and Vite build).

- Blocker:
  - `src/game/difficulty.ts:53-64`: `ceil((waveDensity - 1) * enemies.length)` exceeds the declared density cap. A 3-enemy wave at maximum density becomes 5 enemies (`1.67x`, not `1.5x`). At any positive progress, a 3-enemy wave already gets one extra enemy because `ceil(positive fraction) = 1`.
  - `src/game/difficulty.ts:48-62`: formation scaling is not bounded to the playable envelope. Late waves can spawn at approximately `x = ±9.04`, while `src/game/Arwing.tsx:6-8` limits the player to `±7.2`. These enemies may be impossible to hit under `src/game/Game.tsx:469-475`’s `1.1` collision radius.
  - `src/game/Enemy.tsx:53-58`: the prediction is clamped relative to the current player position, not to the absolute playable envelope, and seeded error is added after clamping. Predictions can therefore leave the envelope.

- Note:
  - `src/game/difficulty.ts:39-40`: `ceil` causes immediate health jumps. A base-1 scout becomes 2 HP at any progress above zero; this is much sharper than the documented smooth `1.9x` curve.
  - `src/game/Enemy.tsx:51-52` is not a true intercept solve. It uses `range / projectileSpeed`, then leads by `velocity * t * aimLead`. A true solve should use the quadratic relative-motion equation `|r + vt| = speed*t`; current math under/over-leads at high player velocity.
  - `src/game/Enemy.tsx:56-58`: all enemies with equal-length IDs share the same deterministic error sequence, causing synchronized miss/hit patterns.
  - `src/game/Enemy.tsx:46-47`: turret cadence is calculated but turrets are explicitly prevented from firing; verify this is intentional.
  - No algorithm tests are present. Add pure tests for endpoint curves, density counts, envelope bounds, deterministic wave output, intercept edge cases, teammate rescue, and boss expiry.
  - Collision is discrete rather than swept (`src/game/Game.tsx:466-495`), so high-speed projectiles can tunnel through targets at low frame rates.