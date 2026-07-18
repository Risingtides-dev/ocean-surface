# Interactive plot implementation

Implemented the bounded `interactive_plot` agent-render component without changing the existing display-only `chart`.

## Changed files

- `crates/ocean-surface-ui/src/components.rs` — registers and dispatches `interactive_plot`.
- `crates/ocean-surface-ui/src/components/interactive_plot.rs` — contained Leptos component, safe Rust expression parser/evaluator, local parameter state, sampling/geometry, metrics, commit events, and five unit tests.
- `styles/components.css` — responsive/touch/focus-aware plot, metric, and control styling using Ocean tokens.
- `../ocean-os/crates/ocean-runtime/src/tools/component.rs` — adds the kind to the allowlist, tool enum, schema summary, and selection guidance.
- `../ocean-os/docs/AGENT_RENDER_PROTOCOL.md` — authoritative schema, expression limits, event wire example, and truthful client coverage.
- `../ocean-os/docs/OCEAN_SURFACE_COMPONENT_PROMPT_GUIDE.md` — agent-facing usage guidance and example.

## Schema and event

Representative props:

```json
{
  "title": "Exponential decay",
  "parameters": [
    { "id": "rate", "label": "Decay rate", "min": 0.1, "max": 3, "step": 0.1, "value": 1, "unit": "s⁻¹" }
  ],
  "plot": {
    "x": { "id": "t", "label": "Time", "min": 0, "max": 10, "samples": 160 },
    "y_label": "Amplitude",
    "series": [{ "label": "x(t)", "expression": "exp(-rate*t)" }]
  },
  "metrics": [{ "label": "Half-life", "expression": "ln(2)/rate", "unit": " s", "precision": 2 }]
}
```

Slider/number `input` updates only local reactive state and recomputes the graph. The committed `change` sends:

```json
{
  "type": "parameters_changed",
  "payload": {
    "parameters": { "rate": 1.4 },
    "changed": { "id": "rate", "value": 1.4 }
  }
}
```

The Rust expression engine supports literals, variables, parentheses, unary `+/-`, `+ - * / ^`, `pi`, `e`, and `sin`, `cos`, `tan`, `exp`, `ln`/`log`, `sqrt`, `abs`, `min`, `max`. It rejects invalid/non-finite work and enforces 12 parameters, 6 series, 512 samples, 512 characters/expression (plus 12 bounded metrics).

## Validation

- `cargo test -p ocean-surface-ui` — passed: 340 unit tests plus 1 regression/integration test (341 total).
- `PATH="$HOME/.cargo/bin:$PATH" RUSTUP_HOME="$HOME/.rustup" CARGO_HOME="$HOME/.cargo" cargo +stable check -p ocean-surface-ui --target wasm32-unknown-unknown` — passed. The first unqualified invocation selected Homebrew Rust, whose sysroot lacked the WASM stdlib; the repo-prescribed rustup stable PATH succeeded.
- `cd ../ocean-os && cargo test -p ocean-runtime tools::component` — passed: 14 component-tool tests.
- `cd ../ocean-os && cargo xtask docs-check` — passed: 26 packages, 112 active Markdown files, 118 local links.
- `cargo clippy -p ocean-surface-ui --all-targets -- -D warnings` — passed.
- `cd ../ocean-os && cargo clippy -p ocean-runtime --all-targets -- -D warnings` — passed.
- Targeted stable rustfmt checks and `git diff --check` in both repositories — passed.
- Both repository indexes remain empty; no files were staged.

## Residual risk

No browser/Tauri manual interaction screenshot was captured in this delegated slice. Compilation, parser/sampling tests, bounded malformed-prop behavior, tool integration tests, and documentation checks pass; a parent-side independent review/manual render remains the required review gate.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Added only the distinct interactive_plot renderer, contained implementation module, component CSS, runtime allowlist/schema/help, and two authoritative docs; chart and Island files were not changed."
    },
    {
      "id": "criterion-2",
      "status": "satisfied",
      "evidence": "341 Surface tests, 14 focused runtime tests, stable WASM check, both clippy checks, docs-check, rustfmt, and diff-check passed; exact files and event/schema are recorded above."
    }
  ],
  "changedFiles": [
    "crates/ocean-surface-ui/src/components.rs",
    "crates/ocean-surface-ui/src/components/interactive_plot.rs",
    "styles/components.css",
    "../ocean-os/crates/ocean-runtime/src/tools/component.rs",
    "../ocean-os/docs/AGENT_RENDER_PROTOCOL.md",
    "../ocean-os/docs/OCEAN_SURFACE_COMPONENT_PROMPT_GUIDE.md"
  ],
  "testsAddedOrUpdated": [
    "crates/ocean-surface-ui/src/components/interactive_plot.rs: five inline unit tests covering expression evaluation/rejection, bounds/malformed props, and sampled SVG geometry"
  ],
  "commandsRun": [
    {
      "command": "cargo test -p ocean-surface-ui",
      "result": "passed",
      "summary": "340 unit tests and 1 integration regression test passed"
    },
    {
      "command": "PATH=\"$HOME/.cargo/bin:$PATH\" RUSTUP_HOME=\"$HOME/.rustup\" CARGO_HOME=\"$HOME/.cargo\" cargo +stable check -p ocean-surface-ui --target wasm32-unknown-unknown",
      "result": "passed",
      "summary": "Leptos Surface compiled for wasm32-unknown-unknown"
    },
    {
      "command": "cd ../ocean-os && cargo test -p ocean-runtime tools::component",
      "result": "passed",
      "summary": "14 component-tool tests passed"
    },
    {
      "command": "cd ../ocean-os && cargo xtask docs-check",
      "result": "passed",
      "summary": "26 packages, 112 active Markdown files, and 118 links checked"
    },
    {
      "command": "cargo clippy -p ocean-surface-ui --all-targets -- -D warnings",
      "result": "passed",
      "summary": "Surface native/all-target lint check passed"
    },
    {
      "command": "cd ../ocean-os && cargo clippy -p ocean-runtime --all-targets -- -D warnings",
      "result": "passed",
      "summary": "Runtime all-target lint check passed"
    },
    {
      "command": "targeted rustfmt +stable --check and git diff --check in both repositories",
      "result": "passed",
      "summary": "Owned Rust files are formatted and owned diffs have no whitespace errors"
    }
  ],
  "validationOutput": [
    "Surface: test result ok, 340 passed; integration test result ok, 1 passed",
    "WASM: Finished dev profile successfully for wasm32-unknown-unknown",
    "Runtime component tests: 14 passed, 0 failed",
    "docs-check: PASS (26 packages, 112 active Markdown files, 118 local links)",
    "No staged files in either repository"
  ],
  "residualRisks": [
    "No manual browser/Tauri interaction render was captured; independent parent review/manual acceptance remains required."
  ],
  "noStagedFiles": true,
  "diffSummary": "Adds a safe bounded locally reactive interactive_plot renderer with accessible controls and metrics, wires it into Surface/runtime protocol discovery, and documents schema/events.",
  "reviewFindings": [
    "no blockers in worker self-review; independent parent reviewer gate remains"
  ],
  "manualNotes": "The initial unqualified WASM check used Homebrew rustc 1.96 without a WASM sysroot; the rustup stable PATH invocation passed. Existing dirty work was preserved and no Island files were touched."
}
```
