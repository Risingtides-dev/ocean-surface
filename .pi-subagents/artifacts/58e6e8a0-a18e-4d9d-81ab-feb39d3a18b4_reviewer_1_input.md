# Task for reviewer

[Read from: /private/tmp/ocean-surface-tauri-integrate-20260716-v2/plan.md, /private/tmp/ocean-surface-tauri-integrate-20260716-v2/progress.md]

Fresh read-only correctness/security review of complete v2 diff in /private/tmp/ocean-surface-tauri-integrate-20260716-v2. interactive_plot is in scope and Recall backend exists. Inspect authority boundaries, shortcut/focus behavior, session labels, EventSource ownership, permission/cancellation, styles/package wiring, Tauri debug/menu code, and tests. Treat intentional untracked product files as pending atomic commit, not a finding. Report evidence-backed blockers/fixes with file:line and residual manual risks. Do not modify files.

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