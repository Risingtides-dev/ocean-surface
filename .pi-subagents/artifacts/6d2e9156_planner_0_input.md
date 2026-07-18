# Task for planner

[Read from: /Users/smathdaddy-macbook/ocean-surface/context.md]

Act as a product interaction architect. Review the user's correction and the current Ocean Dynamic Island implementation/spec: docs/OCEAN_DYNAMIC_ISLAND_IMPLEMENTATION.md, docs/OCEAN_DYNAMIC_ISLAND_BUILD_PLAN.md, crates/ocean-surface-ui/src/island.rs, styles/island.css, app.rs integration, and screenshots /tmp/ocean-island-final-wide.png and /tmp/ocean-island-narrow.png. User says: it must be dynamic; current result is a list with drawers at top; it should support agent interaction, session switching, and semantic/fuzzy history search, but those must not be clobbered into one interaction. Produce a concrete interaction architecture with distinct modes, transitions, compact-state behavior, hierarchy, keyboard model, and phased implementation. Do not edit any files. Be opinionated and avoid a conventional modal/list dashboard.

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