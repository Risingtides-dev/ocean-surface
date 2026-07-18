# Task for reviewer

Fresh read-only review of the newly implemented `interactive_plot` slice. Inspect the actual current diff only in: `crates/ocean-surface-ui/src/components.rs`, `crates/ocean-surface-ui/src/components/interactive_plot.rs`, `styles/components.css`, `../ocean-os/crates/ocean-runtime/src/tools/component.rs`, `../ocean-os/docs/AGENT_RENDER_PROTOCOL.md`, and `../ocean-os/docs/OCEAN_SURFACE_COMPONENT_PROMPT_GUIDE.md`. Do not modify project/source files. Look for concrete correctness, Leptos reactivity/lifecycle, expression parser/evaluation safety, bounded-work, SVG geometry, accessibility/touch, protocol/event, responsive UI, and documentation mismatches. Report only evidence-backed fixes worth doing now, with severity and file/line references; say PASS if none. Existing dirty changes outside this slice are out of scope.

---
**Output:**
Write your findings to exactly this path: /Users/smathdaddy-macbook/ocean-surface/.pi-subagents/artifacts/outputs/a589143d-d1bf-4324-8116-2e37fba7e24f/.pi-subagents/interactive-plot-review.md
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