## Review
- Correct: All 14 files under `games/skyline-ace` were inspected. Movement targets are clamped (`Arwing.tsx:6-8, 27-28`), boost changes movement/FOV, encounter milestones use a catch-up `while` loop (`Game.tsx:234-252`), and player shots can be deflected during roll.
- Blocker: `Game.tsx:234` calls `useFrame` in `Game`, which is outside the `<Canvas>` provider. React Three Fiber hooks require a descendant of `Canvas`; the app will fail at runtime before gameplay starts.
- Blocker: `Game.tsx:183` and `Game.tsx:190` assign through optional chains (`map.get(id)?.alive = false`). TypeScript rejects this with TS2779, so `npm run build` should be expected to fail.
- Blocker: Barrel timing uses `performance.now()` in `Game.tsx:145` but `clock.elapsedTime` in `Arwing.tsx:33-40`. The elapsed time comparison never reaches completion, leaving `barrel.active` and roll invulnerability active indefinitely.
- Blocker: Reset replaces `input.current`, `world.current`, and `player.current` (`Game.tsx:307-310`), but the memoized `flight` object is never rebuilt (`Game.tsx:138-142`). After redeploy, rendering and input consume stale state.
- Blocker: Teammate rescue is not implemented. Encounter `target` values only produce a status message when an enemy is removed (`Game.tsx:189-194`); `teammates` is never changed outside reset (`Game.tsx:122, 323`), and no teammate entities or damage/rescue logic exist.
- Note: Heavy enemies declare `health: 3`, but `hitEnemy` immediately removes every non-carrier (`Game.tsx:230-231`). Heavy enemies therefore die in one shot.
- Note: Boss phases are absent. Carrier hits only decrement a single health counter (`Game.tsx:214-228`); carrier behavior/weak points/visuals never change. `bossHealth` is updated but not rendered as a HUD value.
- Note: An un-killed carrier simply expires at `z > 12` (`Enemy.tsx:46-48`), and `removeEnemy` does not fail the mission (`Game.tsx:189-193`). The player can miss the boss and continue indefinitely without a win/loss result.
- Note: Turrets never fire because the firing condition explicitly excludes them (`Enemy.tsx:42`). They are currently visual obstacles only.
- Note: Collision checks cover player lasers versus enemies and enemy lasers versus the player (`Game.tsx:271-298`), but there is no direct player/enemy, player/carrier, terrain, or teammate collision.
- Enhancement priority: P0 fix R3F hook placement and TypeScript assignment errors; P1 unify the clock source, repair reset state ownership, and implement teammate rescue/failure; P2 add heavy health handling, boss phases/weak points and miss handling, turret fire, and direct collision.

- Fixed: None; source changes were explicitly prohibited.
- Tests added: None; no test infrastructure or `test` script is present.
- Commands run: None; this was a read-only inspection.