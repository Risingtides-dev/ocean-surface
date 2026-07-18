# Task for worker

You are reviving a previous subagent conversation.

Original run: 1e7a3ecd-0ecf-4e62-9f87-f1060d4187b4
Original agent: worker
Original session file: /Users/smathdaddy-macbook/.pi/agent/sessions/--Users-smathdaddy-macbook-ocean-surface--/2026-07-15T11-44-24-347Z_019f6597-a31b-7383-872c-98870f02eca1.jsonl

Use the stored session context as background. Answer the orchestrator's follow-up below. Do not assume the original child process is still alive.

Follow-up:
You hit the turn cap after implementing most of the slice. Resume only to finish and verify; do not broaden scope or re-audit unrelated files. First inspect your current owned diff/new module. Complete the two docs updates still pending (`../ocean-os/docs/AGENT_RENDER_PROTOCOL.md`, `../ocean-os/docs/OCEAN_SURFACE_COMPONENT_PROMPT_GUIDE.md`), then run targeted rustfmt/check/tests. Fix only compile/test issues in your owned files. Confirm the runtime component tool edits are complete and consistent. Do not touch Island or other dirty files. Return a concise final report with files changed, schema/event example, exact validations/results, and residual risks. If full tests are too slow, run focused next-best checks and state that truthfully.

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