Implemented `interactive_plot` as a bounded, locally reactive Leptos component.

Changed files:
- Surface component dispatch and new contained renderer module
- `styles/components.css`
- Runtime component allowlist/schema/help
- Render protocol and Surface prompt guide

Validation:
- Surface: 341 tests passed
- WASM stable target check passed
- Runtime component tests: 14 passed
- Surface/runtime clippy passed with warnings denied
- Docs check passed
- Rustfmt and diff checks passed
- No staged files

Residual risk: manual browser/Tauri interaction acceptance remains for the parent review gate.

Full report: `.pi-subagents/artifacts/outputs/1e7a3ecd-0ecf-4e62-9f87-f1060d4187b4/.pi-subagents/interactive-plot-worker.md`