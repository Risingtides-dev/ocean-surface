# Task for worker

Ralph loop 4/5 implementation pass. Sole writer; only games/skyline-ace; run npm run build. Fix the remaining review findings without breaking the already-working targeting requirement (enemies must target the player via bounded deterministic intercept):

- Make swept collision robust despite frame-order staleness: bound Laser delta to the same simulation step and/or perform a true segment-vs-moving-target check using previous/current positions; fix resume overshoot.
- Make the carrier’s visible glowing core the actual collision target (not an invisible body-center sphere), while retaining readable 3-phase behavior and killability.
- Keep enemy fire aimed at the player; explicitly document spawn.target as teammate-rescue objective metadata, or if feasible add a bounded non-player target choice without weakening player targeting. Ensure teammate behavior is truthful.
- Make turrets visually track/indicate their computed shot direction where clean.
- Add an active dynamic point-light cap/pool or safe priority gating while retaining pointLight on each active projectile; reduce mobile DPR/effects cost sensibly and keep 60-FPS intent.
- Cap camera resume delta; ensure blur actually pauses; avoid StrictMode side effects in state updaters; clear active entities at terminal state.
- Fix mobile HUD phase/mission overlap, show actual route progress vs active threat clearly, and expose boss phase.
- Remove remote Google font dependency; add reduced-motion and focus-visible CSS.
- Add a lightweight test script only if practical; otherwise add exported pure helpers and keep README honest. Use npm ci instructions and document touch controls.
- Do not make a broad architecture rewrite; preserve Bloom/ToneMapping, low-poly terrain, thrusters, per-projectile light requirement, original art/legal-safe boundary, all three teammates, and modular components. Do not launch other subagents.

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