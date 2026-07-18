## Review

- **Correct**
  - `Terrain.tsx` uses three reusable tile slots and modulo-based reassignment.
  - Plume animation mutates refs imperatively, avoiding React state churn.
  - `SceneEffects.tsx` disables multisampling and applies tone mapping after bloom.
  - Canvas DPR is capped rather than using unrestricted device DPR.

- **Blocker**
  - None identified from static review.

- **Note**
  - **P1 — terrain seam:** `games/skyline-ace/src/game/Terrain.tsx:12-16,54-63`. `heightAt()` uses tile-local `position.y`; adjacent wrapped tiles evaluate opposite local edges (`+64` vs `-64`), producing visible height discontinuities. Pass a continuous world/tile coordinate into the shader.
  - **P1 — excessive dynamic lights:** `games/skyline-ace/src/game/Laser.tsx:47-54`, `Enemy.tsx:84-147`. Every projectile creates a `PointLight`, in addition to enemy, player, and reticle lights. This increases per-fragment cost and can exceed low-end WebGL light uniform limits. Replace with emissive geometry or a capped/shared light pool.
  - **P1 — per-instance GPU resource churn:** `Enemy.tsx:84-147`, `Laser.tsx:48-52`, `Terrain.tsx:69-80`. Geometry and materials are recreated for every enemy/projectile/tile mount. Cache or share immutable geometries/materials, with explicit disposal strategy for shared resources.
  - **P2 — stopped games still render:** `Game.tsx:297-331,~492-514`. `running` only pauses `Simulation`; terrain, camera, reticle, enemies, lasers, bloom, and lights continue rendering after win/loss. Pause the Canvas render loop or propagate the stopped state to animated scene components.
  - **P2 — projectile state churn:** `Game.tsx:169-179`. Each shot and expiry replaces the entire `laserSpecs` array, causing React reconciliation of the Canvas subtree. Use an imperative projectile pool or cap active projectiles.
  - **P2 — mobile postprocessing cost:** `Game.tsx:297`. `dpr={[1, 1.75]}` combined with mipmap bloom can still render over 3× CSS pixel area on high-DPR phones. Consider a 1.25–1.5 cap or adaptive DPR/effects.
  - **P3 — unnecessary projection work:** `Arwing.tsx:~111-124`. `updateProjectionMatrix()` runs every frame even after FOV stabilizes; update only when the interpolated FOV changes materially.
  - **P3 — laser visual alignment:** `Laser.tsx:48-52`. Enemy projectiles can fly diagonally, but their box geometry is never rotated to `direction`, so the streak does not visually follow its trajectory.
  - Generated `dist/` and `node_modules/` were not treated as review targets; Git reports the entire game directory as untracked, so an exact worker diff is unavailable.