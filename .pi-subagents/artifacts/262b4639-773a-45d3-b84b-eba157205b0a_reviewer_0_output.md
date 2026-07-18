## Review

- **Blocker — R3F hook outside Canvas:** `src/game/Game.tsx:238-309` calls `useFrame` directly in `Game`, which renders `<Canvas>` as a child. R3F hooks require a Canvas descendant and will throw at runtime.  
  **Fix:** Move the simulation loop into a component rendered inside `<Canvas>`.

- **Blocker — Barrel-roll clock mismatch:** `src/game/Game.tsx:145` stores `performance.now()/1000`, while `src/game/Arwing.tsx:33-40` compares it with `clock.elapsedTime`. The roll progress remains clamped incorrectly and may never finish.  
  **Fix:** Use one time source consistently.

- **Blocker — Laser collision coordinates are local:** `src/game/Laser.tsx:36-37` updates `mesh.position.z` and copies that local position into the collision handle, while the mesh is inside a group positioned at `origin` (`Laser.tsx:46`). Collision X/Y reset to zero and Z omits the origin.  
  **Fix:** Store world coordinates in the handle, e.g. add `origin` to the mesh-local position or use `getWorldPosition`.

- **Blocker — Reset leaves child runtime stale:** `src/game/Game.tsx:138-142` memoizes `flight` forever, but `reset` replaces `input.current`, `world.current`, and `player.current` at lines 312-314. After redeploy, child components continue reading the old runtime objects.  
  **Fix:** Rebuild `flight` when the run changes, or mutate the existing ref contents instead of replacing them.

- **Note — Carrier can expire without ending the stage:** `src/game/Enemy.tsx:46-48` removes enemies after `z > 12`; `Game.tsx:223-231` only marks victory when carrier health reaches zero. An uncleared carrier disappears and the mission remains live indefinitely.  
  **Fix:** Define a carrier-expiry failure/result path.

- **Note — Teammate state is inert:** `src/game/Game.tsx:122` initializes `teammates`, but no code ever changes it. Encounter `target` metadata only affects status text when enemies are removed.  
  **Fix:** Either implement teammate damage/rescue transitions or remove the unused state until supported.

All files under `games/skyline-ace` were inspected. No source files were modified.

Validation command:
```sh
cd games/skyline-ace && npm install && npm run build
```

The attempted `cd games/skyline-ace && npm run build` exited with `127` because `tsc` is unavailable before dependencies are installed.