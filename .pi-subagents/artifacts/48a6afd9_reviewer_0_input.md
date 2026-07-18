# Task for reviewer

Read-only final review of the Dynamic Island + Cmd+P implementation and the two late fixes. Inspect docs/OCEAN_DYNAMIC_ISLAND_IMPLEMENTATION.md and diffs for crates/ocean-surface-ui/src/{app.rs,island.rs,search.rs,palette.rs,sessions.rs,main.rs}, styles/island.css, index.html, extension/sidepanel.html, scripts/build-extension.sh, and the opt-in diagnostics in crates/ocean-tauri/src/lib.rs. Focus on correctness, regressions, accessibility, overlay/focus/IME behavior, responsive workspace geometry, and security of the env-gated debug hook. The parent has live evidence of 39/39 core assertions, 17/17 deep assertions, 720/840/900/1280 plus max screenshots, and all builds/tests passing. Do not edit files. Report only actionable findings ordered by severity, with file/line references; say 'no blockers' if clean. Note that unrelated pre-existing dirty changes exist and should not be treated as part of this review.

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