## Review

- **Correct:** Terrain tile wrapping and shader world-Z reconstruction in `src/game/Terrain.tsx:68-88` preserve contiguous seams despite rotation.
- **Correct:** Laser orientation in `src/game/Laser.tsx:34-39` aligns local +Z with the supplied direction. ACES tone mapping follows Bloom in `src/game/SceneEffects.tsx`.
- **Correct:** Simulation delta is bounded after tab resume in `src/game/Game.tsx` (`Simulation`), and terminal states stop the render loop.

- **Blocker — `src/game/Enemy.tsx:136-166`:** Carrier’s visible glowing core is at local `z=-4.2` (scaled to approximately `-6.3`), behind the carrier from the player/camera at positive Z. Collision uses a radius-2.35 sphere centered at the carrier origin, so shots aimed at the displayed core do not hit it; shots hit the invisible center instead.
- **High — `src/game/Arwing.tsx:90-99`, `src/game/Enemy.tsx:95`, `:116`, `:163`, `src/game/Laser.tsx:70-71`:** No active `pointLight` cap exists. Every enemy, laser, player plume, and reticle adds a dynamic point light. This scales lighting cost with projectile/entity count and can cause shader/light-limit or 60-FPS regressions on mobile.
- **High — `src/game/Enemy.tsx` enemy render components and `src/game/Laser.tsx:62-71`:** Repeated entities instantiate separate geometries/materials; every laser additionally allocates and disposes a geometry, material, and point light. Sustained fire at 0.16 seconds per shot creates avoidable allocation/GC churn.
- **High — `src/game/Game.tsx` Canvas props and `src/game/SceneEffects.tsx:6-11`:** DPR allows up to `1.75` with no mobile quality tier or adaptive reduction, while mipmap Bloom runs continuously at a low threshold (`0.18`). High-density phones render roughly three times the pixels of DPR 1 under a costly full-screen postprocess.
- **Medium — `src/game/Arwing.tsx:55-72`:** `CameraRig` uses uncapped `delta`, unlike the capped simulation/player updates. The first frame after tab visibility resumes can snap camera position and FOV directly to their targets instead of smoothly recovering.
- **Medium — `src/styles.css:79-86`:** On screens ≤640px, `.hud__mission` moves to `top:58px` while `.boss-hud` remains at `top:96px`; the mission has three text rows, so the carrier HUD overlaps the mission telemetry during the boss encounter.
- **Low — `src/game/Terrain.tsx:93-104`:** All terrain tiles use `frustumCulled={false}`, `DoubleSide`, and distinct shader materials. This forces unnecessary rasterization/material work; geometry/material sharing and normal front-face rendering would reduce cost.

No source files were changed.