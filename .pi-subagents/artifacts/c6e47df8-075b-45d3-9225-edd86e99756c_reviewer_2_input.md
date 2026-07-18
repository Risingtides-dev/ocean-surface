# Task for reviewer

[Read from: /private/tmp/ocean-surface-tauri-integrate-20260715/plan.md, /private/tmp/ocean-surface-tauri-integrate-20260715/progress.md]

Fresh read-only product UI/accessibility review of the integrated diff in /private/tmp/ocean-surface-tauri-integrate-20260715. Focus on Dynamic Island one-intent contract, keyboard/IME/focus restoration and scroll-follow, screen-reader semantics, non-color cues, compact/wide geometry, composer pending-state repair, CSS cascade with Voice Planner, Tauri-only mounting, and browser/extension Print/Find/session behavior. Inspect actual Rust/CSS/tests/docs. Report evidence-backed findings with severity and file:line plus smallest safe fix/manual validation. Do not modify project/source files.

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