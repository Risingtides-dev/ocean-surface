# Task for reviewer

[Read from: /private/tmp/ocean-os-session-projection-20260717/plan.md, /private/tmp/ocean-os-session-projection-20260717/progress.md]

Final independent read-only review of Ocean OS session projection implementation in `/private/tmp/ocean-os-session-projection-20260717` against `/private/tmp/ocean-session-projection-implementation-20260717.md` and Surface consumer `/private/tmp/ocean-surface-session-projection-20260717/crates/ocean-surface-ui/src/daemon.rs`. Focus atomic persistence revision/checkpoint emission, projection inclusion/exclusion, folded cancellation/permissions, call/result checkpoint wire, snapshot alignment, lease ack/retention, restart epochs, SSE framing/Last-Event-ID, shutdown/backpressure, Observatory separation. Report concrete blockers with file:line; no edits.

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