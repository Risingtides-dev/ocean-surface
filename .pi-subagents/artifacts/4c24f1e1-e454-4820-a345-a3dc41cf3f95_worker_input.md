# Task for worker

Ralph loop 1/5 implementation pass. You are the sole writer for this active worktree. Inspect all files under games/skyline-ace and apply the round-1 review findings below, plus the new product requirement: enemies must get harder over the level and use a deterministic targeting algorithm that aims at the player's predicted position rather than firing randomly. Keep the original asset-free homage boundary; do not touch files outside games/skyline-ace. Preserve the modular JSX architecture and mandatory Bloom/ToneMapping, projectile point lights, thruster plumes, and scrolling terrain.

Required correctness fixes:
- Move the simulation useFrame into a component rendered inside Canvas.
- Fix optional-chain assignment TypeScript errors.
- Use one time source for barrel roll.
- Fix laser world-space collision coordinates.
- Make reset preserve or rebuild runtime objects.
- Resolve carrier expiry and implement teammate state transitions.
- Fix terrain tile recycling/order and plume orientation.

New gameplay requirement:
- Add a single documented difficulty/targeting model: scale enemy health, fire cadence, projectile speed, aim lead, wave density, and formations from normalized level progress (0..1), with caps.
- Each enemy shot should compute a lead intercept against player position/velocity or a bounded prediction, clamp aim to avoid impossible hits, and be more accurate at later milestones. Make barrel-roll invulnerability still able to deflect shots.
- Surface difficulty/targeting in the HUD or mission status so it is testable.

Make the smallest coherent edits, avoid overengineering, and run `npm install` then `npm run build` from games/skyline-ace if network/dependencies allow. Report changed files, the algorithm, commands, and any residual issues. Do not launch other subagents.

## Acceptance Contract
Acceptance level: reviewed
Completion is not accepted from prose alone. End with a structured acceptance report.

Criteria:
- criterion-1: Implement the requested change without widening scope
- criterion-2: Return evidence sufficient for an independent acceptance review

Required evidence: changed-files, tests-added, commands-run, validation-output, residual-risks, no-staged-files

Review gate: required by reviewer.

Finish with a fenced JSON block tagged `acceptance-report` in this shape:
Use empty arrays when no items apply; array fields contain strings unless object entries are shown.
```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "specific proof"
    }
  ],
  "changedFiles": [
    "src/file.ts"
  ],
  "testsAddedOrUpdated": [
    "test/file.test.ts"
  ],
  "commandsRun": [
    {
      "command": "command",
      "result": "passed",
      "summary": "short result"
    }
  ],
  "validationOutput": [
    "validation output or concise summary"
  ],
  "residualRisks": [
    "none"
  ],
  "noStagedFiles": true,
  "diffSummary": "short description of the diff",
  "reviewFindings": [
    "blocker: file.ts:12 - issue found, or no blockers"
  ],
  "manualNotes": "anything else the parent should know"
}
```