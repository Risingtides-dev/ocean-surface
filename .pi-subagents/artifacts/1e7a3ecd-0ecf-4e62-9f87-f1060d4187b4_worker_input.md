# Task for worker

You are a delegated subagent running from a fork of the parent session. Treat the inherited conversation as reference-only context, not a live thread to continue. Do not continue or answer prior messages as if they are waiting for a reply. Your sole job is to execute the task below and return a focused result for that task using your tools.

Task:
Implement the first reusable Leptos/WASM live computational component for Ocean: a new `interactive_plot` agent-render component. This is an approved bounded implementation handoff. Work in the existing dirty repos without resetting, staging, committing, or modifying unrelated changes.

Product contract:
- Keep existing `chart` display-only. Add a distinct `interactive_plot` kind.
- It renders an embedded graph plus bound parameter controls. Moving a slider/number control must recompute the graph and optional derived metrics locally and immediately; no model/daemon round trip is required for preview.
- On committed control change (`change`, not every animation/input frame), call the existing `Daemon::send_component_event` with `type: "parameters_changed"` and payload containing the complete current parameter map plus the changed parameter id/value.
- Use a bounded declarative math expression engine implemented in Rust. Never use JavaScript `eval`, arbitrary script, network calls, or provider/runtime authority. Support enough safe math for physical/economic curves: numeric literals, named variables, parentheses, unary +/-; + - * / ^; constants pi/e; functions sin, cos, tan, exp, ln/log, sqrt, abs, min, max. Invalid expressions/data must produce a truthful inline fallback instead of panic/NaN geometry.
- Suggested props schema:
  `{ title?, description?, parameters:[{id,label?,min,max,step?,value,unit?}], plot:{ x:{id?,label?,min,max,samples?}, y_label?, series:[{label,expression}] }, metrics?:[{label,expression,unit?,precision?}] }`.
  The x variable defaults to `x`. Expressions can reference x and parameter ids.
- Bound work: max 12 parameters, 6 series, 512 samples, 512 expression chars; sanitize/clamp non-finite/range data. No playback or animated simulation schematic in this slice—that is the later `simulation` composite.
- UI must be responsive, touch/keyboard accessible, expose range plus numeric value editing, use Ocean tokens only for colors, respect reduced motion, and render a useful textual error/empty state. Do not add a new stylesheet; use `styles/components.css`.

Integration scope:
- Prefer a contained new module at `crates/ocean-surface-ui/src/components/interactive_plot.rs`, referenced from clean `crates/ocean-surface-ui/src/components.rs`, rather than touching dirty `main.rs`.
- Add dispatch in `ComponentView` and tests for expression parsing/evaluation, bounds, sampled output/geometry helpers, and malformed props as practical.
- Update the daemon-owned component tool allowlist/schema/help in `../ocean-os/crates/ocean-runtime/src/tools/component.rs` and the authoritative docs `../ocean-os/docs/AGENT_RENDER_PROTOCOL.md` plus `../ocean-os/docs/OCEAN_SURFACE_COMPONENT_PROMPT_GUIDE.md`. Keep client coverage claims truthful.
- You may add a focused Surface regression test if useful.

Hard file-safety constraints:
- Do not modify `app.rs`, `daemon.rs`, `main.rs`, island/search/session/palette files, `styles/island.css`, `styles/compact.css`, any HTML/extension stylesheet enumerations, or unrelated daemon/TUI/history/permission work.
- Inspect current contents before every edit. Preserve all pre-existing dirty changes. Do not run formatters over entire dirty repos; format only files you own where possible.
- You are the sole writer for this delegated slice. Do not run subagents.

Validation:
- `cargo test -p ocean-surface-ui` (or focused next-best test if target constraints intervene)
- `cargo check -p ocean-surface-ui --target wasm32-unknown-unknown`
- focused ocean-runtime component tool tests/check, plus rustfmt/diff-check for owned files.
- Report changed files, event/schema example, validation commands/results, and any remaining risks. If a required architecture decision falls outside this contract, stop and ask through intercom rather than guessing.

---
**Output:**
Write your findings to exactly this path: /Users/smathdaddy-macbook/ocean-surface/.pi-subagents/artifacts/outputs/1e7a3ecd-0ecf-4e62-9f87-f1060d4187b4/.pi-subagents/interactive-plot-worker.md
This path is authoritative for this run.
Ignore any other output filename or output path mentioned elsewhere, including output destinations in the base agent prompt, system prompt, or task instructions.

## Acceptance Contract
Acceptance level: reviewed
Completion is not accepted from prose alone. End with a structured acceptance report.

Criteria:
- criterion-1: Implement the requested change without widening scope
- criterion-2: Return evidence sufficient for an independent acceptance review

Required evidence: changed-files, tests-added, commands-run, validation-output, residual-risks, no-staged-files

Review gate: optional by reviewer.

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