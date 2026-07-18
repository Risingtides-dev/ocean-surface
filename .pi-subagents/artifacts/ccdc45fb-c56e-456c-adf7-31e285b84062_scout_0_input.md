# Task for scout

Read-only integration recon for Ocean Surface Tauri work. Inspect /Users/smathdaddy-macbook/ocean-surface current dirty main, handoff.md, docs/OCEAN_DYNAMIC_ISLAND_{IMPLEMENTATION,BUILD_PLAN}.md, the five commits in main..origin/main, and especially overlapping app.rs, daemon.rs, styles/compact.css, styles/composer.css. Determine exact conflict/merge risks and a safe logical-slice port order into a clean origin/main-based worktree. Report file/line or hunk-level evidence, tests affected, and unresolved decisions. Do not modify project/source files.

---
**Output:**
Write your findings to exactly this path: /Users/smathdaddy-macbook/ocean-surface/.pi-subagents/artifacts/outputs/ccdc45fb-c56e-456c-adf7-31e285b84062/context.md
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