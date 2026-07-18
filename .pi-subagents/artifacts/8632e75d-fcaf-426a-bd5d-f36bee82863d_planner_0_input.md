# Task for planner

[Read from: /private/tmp/ocean-os-session-cursor-20260716/context.md]

Read-only implementation architecture and test-first execution plan for the approved session projection cursor contract in /private/tmp/ocean-os-session-cursor-20260716, plus Surface consumer in /private/tmp/ocean-surface-tauri-integrate-20260716-v2. Read /private/tmp/ocean-session-cursor-context-20260716.md and current code. Approved semantics: completed thinking durable/in-progress transient; SurfacePatch separate canvas cursor; old daemons best-effort legacy; configurable 256 MiB global journal, 60s attach lease, checkpoint compaction, explicit reset. Design the smallest correctness-preserving implementation that fixes snapshot/SSE switch race without pretending bus UUIDs are atomic. Give exact structs/files/routes/event integration/test seams, transaction/locking invariants, migration/compatibility plan, wire schema, ordered compilable slices, and hidden risks. Do not edit files.

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