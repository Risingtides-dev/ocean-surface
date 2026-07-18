# Task for context-builder

Build an implementation-ready read-only architecture brief for a stable session snapshot/SSE cursor contract in /private/tmp/ocean-os-session-cursor-20260716, consumed by Ocean Surface. Inspect session persistence/checkpoint order, per-session turn locks, AgentEventBus replay/history/emit order, GET /v1/sessions/{id}, GET /v1/agent/events Last-Event-ID/full replay, event IDs, and client hydration race. The contract must let a client obtain a persisted transcript snapshot plus an authoritative cursor such that subscribing after it yields every event not represented in the snapshot exactly once, without duplicating text deltas or losing tool/permission lifecycle. Identify whether this is implementable with current locks/order, exact files/types/routes/tests, compatibility strategy, failure/eviction semantics, and stop conditions. Do not modify files.

---
**Output:**
Write your findings to exactly this path: /private/tmp/ocean-session-cursor-context-20260716.md
This path is authoritative for this run.
Ignore any other output filename or output path mentioned elsewhere, including output destinations in the base agent prompt, system prompt, or task instructions.

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