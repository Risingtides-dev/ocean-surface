# Task for reviewer

[Read from: /private/tmp/ocean-os-session-projection-20260717/plan.md, /private/tmp/ocean-os-session-projection-20260717/progress.md]

Fresh read-only integration/compatibility review of complete diff in `/private/tmp/ocean-os-session-projection-20260717`. Verify legacy `/v1/agent/events` and `/v1/events` remain wire-compatible, all production/test AppState bus constructors use/omit store intentionally, route manifest/docs are truthful, no Observatory content or accepted spec edits, tests substantiate claims, and no hidden current-origin conflict. Run narrow checks only if useful. Report exact blocker/fix findings and residual risks; do not modify files.

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