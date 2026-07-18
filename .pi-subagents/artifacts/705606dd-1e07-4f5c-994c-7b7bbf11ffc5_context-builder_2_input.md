# Task for context-builder

Create a read-only brand-and-asset-generation context brief for two Ocean design-system documents intended for Claude Design software and ChatGPT image generation. Inspect accepted Ocean brand assets and design docs in ocean-surface plus relevant TUI identity in ../ocean-os. Capture non-negotiable brand constraints, canonical logo usage, color direction, rejected directions, iconography opportunities, prompt-writing guardrails, asset deliverable specs, and cross-surface consistency rules. Do not modify project/source files. Return evidence-backed findings with file paths.

---
**Output:**
Write your findings to exactly this path: /Users/smathdaddy-macbook/ocean-surface/.pi-subagents/artifacts/outputs/705606dd-1e07-4f5c-994c-7b7bbf11ffc5/recon/brand-asset-brief.md
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