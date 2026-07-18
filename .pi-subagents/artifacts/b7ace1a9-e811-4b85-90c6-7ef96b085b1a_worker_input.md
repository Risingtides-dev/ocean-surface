# Task for worker

Ralph loop 3/5 implementation pass. You are the sole writer; modify only games/skyline-ace and run npm run build. Address the latest reviewers' concrete findings, prioritizing playability and correctness:

1. Fix the terrain rotated-plane world-Z sign so wrapped tiles have no seam.
2. Make the carrier boss killable and fun: its scaled HP, spawn window, fire cadence, and expiry must leave enough time for player fire to defeat it; add at least simple phase thresholds (behavior/visual/cadence) and a clear core hit area. Keep boss HUD truthful.
3. Add swept segment/capsule collision for lasers so low FPS cannot tunnel through enemies/player; use previous and current projectile positions if needed.
4. Let turrets fire with the same deterministic predicted-player targeting model, and add player/enemy-body collision if safe.
5. Pause/clear input on hidden tabs and cap the first delta after resume. Stop/clear scene entities on terminal state.
6. Fix aircraft/projectile orientation so the nose and thrusters point opposite the firing direction correctly; keep emissive plumes and pointLight projectiles.
7. Improve deterministic aim hashing (full ID + shot index) and use a bounded quadratic intercept where practical.
8. Keep density behavior honest, within caps and playable envelope; ensure the HUD/readme match realized behavior.
9. Cancel touch-release timers, prevent narrow-screen HUD/control overlap, and retain accessibility.
10. Add lightweight pure helper tests only if no heavy framework is needed; otherwise export pure helpers and document that build is the check.

Preserve original asset-free art, Bloom/ToneMapping, mobile controls, teammate rescue/expiry, barrel-roll deflection, and modular components. Do not edit Ocean files or generated dist/node_modules. Report algorithm decisions and residual risks. Do not launch subagents.

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