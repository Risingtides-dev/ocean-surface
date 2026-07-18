# Task for researcher

Creator discovery frontier 3 (crossover). Before any searching, read /Users/smathdaddy-macbook/ocean-surface/campaign-research/auto-research/excluded-handles.txt and exclude every listed handle; do not return any listed handle. Search broadly with follow-up queries for personality-led TikTok creators at the cars x EDM crossover: night drives, car meets, festival travel, cinematic beat-drop edits, and related culture. Avoid celebrities, mega-creators, labels, publishers, fan/repost/faceless pages, dealerships, and generic brand accounts. Shared gates: candidates must eventually be <=150K followers and average >=3K views across a reasonable sample of recent non-pinned posts; never claim a gate passed without direct visible evidence, and every result remains candidate-for-verification. Each candidate requires a direct TikTok profile URL plus a direct TikTok evidence URL or public search-result URL visibly supporting fit. Return up to 20 NEW handles as JSONL objects only, one per line, exactly keys handle,profile_url,evidence_url,observable_fit,confidence. Do not modify project/source files; write only the configured output artifact.

---
**Output:**
Write your findings to exactly this path: /Users/smathdaddy-macbook/ocean-surface/campaign-research/auto-research/frontier-1/crossover.md
This path is authoritative for this run.
Ignore any other output filename or output path mentioned elsewhere, including output destinations in the base agent prompt, system prompt, or task instructions.

## Acceptance Contract
Acceptance level: checked
Completion is not accepted from prose alone. End with a structured acceptance report.

Criteria:
- criterion-1: Implement the requested change without widening scope

Required evidence: changed-files, tests-added, commands-run, residual-risks, no-staged-files

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