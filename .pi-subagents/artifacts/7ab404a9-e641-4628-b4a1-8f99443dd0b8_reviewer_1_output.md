## Review
- Correct: `npm run build` passes (`tsc --noEmit` and Vite production build).
- Correct: Difficulty is deterministic and bounded: progress, health, cadence, projectile speed, aim lead, accuracy, wave density, and formation scale all clamp to documented caps.
- Correct: Enemy fire uses deterministic ID/shot-index seeds, bounded quadratic intercepts, player-envelope clamping, and always targets the player.
- Correct: Turrets render aim direction; carrier has three health phases, a core-only hitbox, and can be defeated.
- Correct: Lyra, Rook, and Pip each have explicit threatened → rescued/down transitions.
- Blocker: none confirmed.
- Note: Build emits only a non-blocking large-chunk warning. No automated gameplay tests exist; validation is typecheck/build plus code audit.