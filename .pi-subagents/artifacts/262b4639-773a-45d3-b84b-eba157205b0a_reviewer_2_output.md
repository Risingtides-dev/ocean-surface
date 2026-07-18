## Review

Inspected every file under `games/skyline-ace` (HTML, config, README, CSS, entrypoint, and all game modules).

- **Correct:** Bloom precedes ACES tone mapping in `SceneEffects.tsx:4-14`; low-poly materials, smooth camera interpolation, and registries for active enemies/lasers are coherent.
- **Blocker:** `Laser.tsx:33-45` moves the mesh in local coordinates but copies that local position into the world-space collision handle. Laser collision and expiry are offset from their actual rendered positions. Store world-space position or move the group itself.
- **Blocker:** `Game.tsx` `triggerBarrel` stores `performance.now()` while `Arwing.tsx:35-43` compares it with `clock.elapsedTime`. Barrel-roll progress remains negative indefinitely, so the roll never completes.
- **Blocker:** `Terrain.tsx:63-68` wraps tiles in the wrong ordering. Near distance 128, tile centers become approximately `[36, 164, 292]`, leaving the forward view without terrain before snapping back. Use decreasing tile indices or recycle tiles once they pass the camera.
- **High:** Thruster plumes in `Arwing.tsx` are rotated by `Math.PI` around X, so their cone axis is vertical rather than along the aircraft’s longitudinal axis. Pulsing Y scale therefore stretches them vertically, not backward.
- **High:** Static water and landmark meshes in `Terrain.tsx:76-89` do not scroll with terrain tiles, producing world-motion inconsistencies.
- **High:** Every laser creates a `PointLight` (`Laser.tsx:50-58`). Continuous fire can maintain roughly 10 player lights plus enemy lights, with React state/geometry/material churn per shot. Pool lasers and replace projectile lights with emissive geometry or a capped light pool.
- **High:** `Game.tsx` configures `dpr={[1, 1.75]}` and full-resolution `EffectComposer` bloom with `mipmapBlur` (`SceneEffects.tsx:4-12`). This is expensive on mobile GPUs. Add adaptive DPR and a reduced/no-bloom mobile preset.
- **Medium:** `Terrain.tsx:74` sets `frustumCulled={false}` on all terrain tiles, forcing vertex processing for offscreen tiles. Custom bounds or correct displacement bounds would permit culling.
- **Medium:** Terrain uses `DoubleSide` and independent geometry/material instances for each tile. Reuse resources and use front-side rendering unless underside visibility is intentional.
- **Medium:** Terrain shader’s `uTime` is unused, and height calculations use tile-local coordinates, causing seams/repetition at tile boundaries. Include world/tile Z in the shader inputs.
- **Medium:** `Game.tsx` memoizes `flight` with `[]`, then `reset()` replaces `input`, `world`, and `player` refs. After redeploy, rendered components retain stale runtime objects while simulation uses new ones. Mutate existing state objects or recreate `flight` when resetting.
- **Note:** `CameraRig` currently provides smooth follow and boost FOV, but no banking, speed-based camera offset, impact shake, or target lead. These are prioritized visual enhancements after correctness fixes.
- **Note:** Mobile has no touch/pointer controls; the CSS mobile breakpoint only rearranges HUD elements.

### Prioritized enhancements

1. Fix laser world-space transforms and barrel-roll clock consistency.
2. Correct terrain tile recycling and synchronize water/landmarks with scroll.
3. Reorient thruster plumes and scale along their exhaust axis.
4. Pool laser entities/lights; cap dynamic point lights.
5. Add adaptive DPR and mobile postprocessing quality tiers.
6. Restore frustum culling and share terrain resources.
7. Add camera banking, speed response, and impact feedback.
8. Add touch controls or explicitly mark mobile as display-only.