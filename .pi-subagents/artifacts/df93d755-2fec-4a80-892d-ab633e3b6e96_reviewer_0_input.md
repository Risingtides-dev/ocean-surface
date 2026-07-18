# Task for reviewer

[Read from: /private/tmp/ocean-surface-session-projection-20260717/plan.md, /private/tmp/ocean-surface-session-projection-20260717/progress.md]

Read-only re-review of repaired Surface projection consumer in `/private/tmp/ocean-surface-session-projection-20260717`. Prior blockers were: new-daemon legacy downgrade, unbounded restart cycle/stale folded state, unordered component reconstruction, Connecting EventSource ownership, SSE id/body mismatch, permissive folded state. Verify each fix semantically against `/private/tmp/ocean-os-session-projection-20260717/crates/ocean-core/src/lib.rs` and gloo EventSource drop behavior. Look for new regressions in reset count, reconnect, snapshot/tail ordering, Voice Planner. Report only concrete blockers/high-confidence issues with file:line; no edits.

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