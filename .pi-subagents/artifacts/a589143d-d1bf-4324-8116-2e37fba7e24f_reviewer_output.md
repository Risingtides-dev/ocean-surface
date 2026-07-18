## Review

- **High — SVG normalization can emit `NaN` geometry instead of the documented inline error.** `sample_plot` verifies only that sampled extrema are finite, then computes `span = y_max - y_min` and coordinates without verifying either result (`crates/ocean-surface-ui/src/components/interactive_plot.rs:504-525`). Finite values such as `-1e308` and `1e308` produce an infinite span; normalizing the upper value becomes `inf / inf = NaN`, yet the function returns `Ok(SampledPlot)`. A constant value near `f64::MAX` can similarly overflow while applying padding at lines 507-510. This contradicts the protocol promise that non-finite work renders an error rather than plot geometry (`../ocean-os/docs/AGENT_RENDER_PROTOCOL.md:222-224`). Fix by rejecting non-finite/zero spans and each non-finite normalized coordinate before formatting; add extreme-range regression tests.

- **Medium — declared parameter IDs are silently changed in emitted protocol events.** Configuration lowercases every parameter ID (`crates/ocean-surface-ui/src/components/interactive_plot.rs:367-370`), stores the normalized form, and emits it as both the complete parameter-map key and `changed.id` (`crates/ocean-surface-ui/src/components/interactive_plot.rs:685-690,705-711`). Thus a valid declared ID such as `DecayRate` is returned as `decayrate`. The docs say the event contains the changed ID/value and parameter map, but never specify canonicalization (`../ocean-os/docs/AGENT_RENDER_PROTOCOL.md:207-216`; `../ocean-os/docs/OCEAN_SURFACE_COMPONENT_PROMPT_GUIDE.md:91-95`). Preserve the declared ID for protocol output while using a normalized evaluator key, or reject/document lowercase-only IDs.

- **Medium — `precision: 0` corrupts metric values.** Precision is explicitly accepted and clamped to 0–6 (`crates/ocean-surface-ui/src/components/interactive_plot.rs:441-445`), but `format_value` removes trailing zeroes even when no decimal point exists (`crates/ocean-surface-ui/src/components/interactive_plot.rs:554-559`). Consequently `100` and `10` render as `1`, and `0` renders as an empty string. Trim zeroes only from the fractional portion (or bypass trimming when precision is zero), and cover 0/10/100 with a unit test.

- **Medium — multi-series plots are not identifiable and rely on color alone.** Although each series has a label (`crates/ocean-surface-ui/src/components/interactive_plot.rs:48-50`), successful rendering outputs only unlabeled paths under the generic accessible name `Computed plot` (`crates/ocean-surface-ui/src/components/interactive_plot.rs:607-617`). The only visual distinction is six CSS stroke colors (`styles/components.css:825-838`); no legend or textual series association is rendered. Users cannot determine which curve corresponds to which label, and screen-reader users receive no series identities. Render a visible legend/non-color cue and expose series labels/summary in the SVG accessible description.

- **Low — the documented bounded-work limits omit the implemented metric cap.** The implementation rejects more than 12 metrics (`crates/ocean-surface-ui/src/components/interactive_plot.rs:424-430`), while both limit lists mention parameters, series, samples, and expression length only (`../ocean-os/docs/AGENT_RENDER_PROTOCOL.md:219-223`; `../ocean-os/docs/OCEAN_SURFACE_COMPONENT_PROMPT_GUIDE.md:91-94`). Add “12 metrics” to both references and, ideally, the runtime tool description at `../ocean-os/crates/ocean-runtime/src/tools/component.rs:95,140`.

### Correct

- The kind is wired consistently through the Surface dispatcher and runtime allowlist/schema (`crates/ocean-surface-ui/src/components.rs:14-15,77-79`; `../ocean-os/crates/ocean-runtime/src/tools/component.rs:27-46,124-140`).
- Reactive sampling and metric evaluation track `values.get()` (`crates/ocean-surface-ui/src/components/interactive_plot.rs:601-630,635-649`), while commit events use the established `{type,payload}` component-event shape.
- Parser feature coverage, parameter/series/sample/expression bounds, input clamping, and ordinary SVG bounds have unit coverage (`crates/ocean-surface-ui/src/components/interactive_plot.rs:741-812`). The five targeted tests pass.
- CSS uses design tokens and includes a single-column mobile control layout (`styles/components.css:1001-1017`).

### Validation limitations

- The canonical WASM check could not run because `wasm32-unknown-unknown` is not installed in this environment.
- No browser/manual screen-reader or touch-device validation was performed.
- Existing dirty changes outside the six requested paths were not reviewed.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Performed a read-only review limited to the six requested files; no project/source file was modified and out-of-scope dirty changes were ignored."
    },
    {
      "id": "criterion-2",
      "status": "satisfied",
      "evidence": "Findings include concrete file/line evidence, targeted test output, diff/staging validation, and explicit residual risks."
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
    "crates/ocean-surface-ui/src/components/interactive_plot.rs:741-812 (five new inline unit tests)"
  ],
  "commandsRun": [
    {
      "command": "git diff/status for the six scoped files in ocean-surface and ../ocean-os",
      "result": "passed",
      "summary": "Inspected tracked diffs plus the untracked interactive_plot.rs; confirmed no staged files in either repository."
    },
    {
      "command": "cargo test -p ocean-surface-ui interactive_plot",
      "result": "passed",
      "summary": "5 interactive_plot unit tests passed; 335 unrelated unit tests and 1 integration test were filtered out."
    },
    {
      "command": "cargo test -p ocean-surface-ui expression_engine --no-fail-fast",
      "result": "passed",
      "summary": "2 expression-engine tests passed."
    },
    {
      "command": "cargo check -p ocean-surface-ui --target wasm32-unknown-unknown",
      "result": "failed",
      "summary": "Environment lacks the wasm32-unknown-unknown core/std target; this is an environment limitation, not a diagnosed source failure."
    },
    {
      "command": "git diff --check on all six scoped files",
      "result": "passed",
      "summary": "No whitespace errors reported."
    },
    {
      "command": "python3 reproduction of normalization and precision algorithms",
      "result": "passed",
      "summary": "Confirmed finite extrema can yield span=inf and scaled=nan; confirmed precision 0 renders 100/10 as '1' and 0 as ''."
    },
    {
      "command": "cargo test -p ocean-runtime tools::component --no-fail-fast",
      "result": "failed",
      "summary": "Invoked from the ocean-surface workspace, which does not contain sibling package ocean-runtime; no runtime test result was obtained."
    }
  ],
  "validationOutput": [
    "interactive_plot tests: 5 passed, 0 failed",
    "expression_engine tests: 2 passed, 0 failed",
    "Algorithm reproduction: ymin/ymax finite, span=inf, scaled=nan",
    "Algorithm reproduction: format_value(100,0)='1'; format_value(0,0)=''",
    "No staged files in ocean-surface or ../ocean-os",
    "Scoped diff --check clean"
  ],
  "residualRisks": [
    "WASM-target compilation was not validated because the target is unavailable.",
    "Browser, screen-reader, responsive-container, and physical touch behavior were not manually exercised.",
    "Sibling ocean-runtime tests were not run successfully."
  ],
  "noStagedFiles": true,
  "diffSummary": "Adds interactive_plot dispatch and rendering, a bounded local expression parser/evaluator with reactive controls and SVG output, component CSS, runtime kind/schema exposure, protocol documentation, prompt guidance, and five inline tests.",
  "reviewFindings": [
    "high: crates/ocean-surface-ui/src/components/interactive_plot.rs:504-525 - finite extreme samples can normalize to NaN SVG geometry without returning an error",
    "medium: crates/ocean-surface-ui/src/components/interactive_plot.rs:367-370,685-711 - parameter IDs are lowercased before protocol emission",
    "medium: crates/ocean-surface-ui/src/components/interactive_plot.rs:441-445,554-559 - precision 0 strips significant integer zeroes",
    "medium: crates/ocean-surface-ui/src/components/interactive_plot.rs:607-617 and styles/components.css:825-838 - series labels are not rendered and curves rely on color alone",
    "low: docs limit lists omit the implemented 12-metric cap"
  ],
  "manualNotes": "Fresh read-only review only; the required artifact is the sole file written."
}
```
