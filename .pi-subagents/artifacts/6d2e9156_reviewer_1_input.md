# Task for reviewer

[Read from: /Users/smathdaddy-macbook/ocean-surface/plan.md, /Users/smathdaddy-macbook/ocean-surface/progress.md]

Perform an adversarial UX/product review of the current Island against the user's stated intent. Inspect docs/OCEAN_DYNAMIC_ISLAND_IMPLEMENTATION.md, island.rs, styles/island.css, app.rs, and /tmp/ocean-island-final-wide.png plus /tmp/ocean-island-narrow.png. Identify exactly why it reads as a list/popover with drawers rather than a dynamic agent interaction object. Recommend structural fixes, not cosmetic polish. Separate must-fix interaction problems from useful pieces to retain. Do not modify project/source files.

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