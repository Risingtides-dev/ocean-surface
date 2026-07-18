## Review

- **Correct:** `cd games/skyline-ace && npm run build` passed: TypeScript and Vite production build succeed.
- **Correct:** Projectile IDs use a monotonic sequence; reset/remount, pause bounds, boss/teammate state transitions, and keyboard/touch cleanup are coherent.
- **Blocker:** `src/game/Game.tsx` — `CollisionSystem` assumes JSX order guarantees it runs after dynamically mounted `Enemy`/`Laser` `useFrame` callbacks. R3F subscriptions for newly mounted entities can run after the already-mounted collision callback, producing stale sweep endpoints and missed fast collisions.  
  **Minimal fix:** assign explicit negative `useFrame` priorities to simulation/movement (`Simulation: -2`, `Arwing`/`Enemy`/`Laser: -1`) and leave collision at priority `0`, or centralize movement and collision in one frame callback.
- **Blocker:** `src/game/Game.tsx` `CollisionSystem` — enemy lasers and body collisions sweep against `player.position` only. A moving player can cross a projectile/enemy path between frames and tunnel through it.  
  **Minimal fix:** add `PlayerState.previousPosition`, copy it before movement in `Arwing`, reset it, then use `sweptMovingCapsuleHits` for enemy-laser/player and enemy-body/player checks.

- **Note:** Build reports a 1.16 MB minified JS chunk; not a correctness blocker.
- **Fixed:** None; review was read-only.