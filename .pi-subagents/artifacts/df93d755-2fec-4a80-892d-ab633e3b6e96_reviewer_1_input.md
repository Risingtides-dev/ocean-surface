# Task for reviewer

[Read from: /private/tmp/ocean-surface-session-projection-20260717/plan.md, /private/tmp/ocean-surface-session-projection-20260717/progress.md]

Fresh adversarial protocol/reducer review after fixes in `/private/tmp/ocean-surface-session-projection-20260717`. Focus ordered component call/result matching and transcript insertion, strict folded state, cursor/SSE id handling, retry/fallback exactness, source closure races, canvas isolation, proxy Last-Event-ID. Compare exact daemon wire in `/private/tmp/ocean-os-session-projection-20260717`. No edits; provide precise evidence and residual live-test risks.

## Acceptance Contract
Acceptance level: attested
Completion is not accepted from prose alone. End with a structured acceptance report.

Criteria:
- criterion-1: Return concrete findings with file paths and severity when applicable

Required evidence: review-findings, residual-risks

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