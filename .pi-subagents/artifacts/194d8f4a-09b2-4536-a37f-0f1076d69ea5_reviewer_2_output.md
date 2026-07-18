## Review

Not clean; no blockers.

- **High — Fix impossible control geometry in normative YAML.** `design-systems/ocean-leptos/DESIGN.md:122-162` applies all-side padding of 14px/12px/8px to controls only 34px/28px high. It also assigns the primary button regular 13px/400 typography, contradicting the required semibold weight at `:335`. Use horizontal padding such as `"0 14px"` and add/reference a semibold control typography token.

- **Medium — Resolve cross-system semantic-token drift.** `ocean-leptos/DESIGN.md:7-9` defines `on-primary: #03181A` and `primary-deep: #0087AF`, while `ocean-tui/DESIGN.md:7-8` gives the same names `#060606` and `#005FAF`. The TUI values are undocumented and unused. Align shared tokens or rename the TUI-specific deep-logo role and explain the mapping.

- **Medium — Correct the TUI thinking-state claim.** `ocean-tui/DESIGN.md:195,397` says thinking currently uses violet. The active renderer uses muted `theme::COMMENT` for thinking (`../ocean-os/crates/ocean-tui/src/shell/components/chat.rs:3191-3194`); violet is used for Graph/config accents. Either describe violet thinking as proposed or document the current muted treatment.

- **Medium — Make the “copy-ready” image prompts satisfy their own output contract.** The Leptos contract requires canvas, alpha, exact state colors, and deliverables at `ocean-leptos/DESIGN.md:457-465`, but the component prompt at `:475` omits exact canvas/alpha/export details and semantic success/warning/error colors. The TUI component prompt at `ocean-tui/DESIGN.md:436` likewise requests running/error states without supplying their token values. Add explicit placeholders for pixel dimensions, transparency/background, formats/source, and all requested state colors.

- **Low — Triage and justify packaged lint warnings.** The bundled reports still contain 34 and 18 warnings (`ocean-leptos/lint-report.json:178-181`, `ocean-tui/lint-report.json:98-101`). Some identify genuinely ambiguous tokens: Leptos water colors are undocumented and unused (`ocean-leptos/DESIGN.md:51-53`), while TUI defines an unused shadow token despite prohibiting shadows (`ocean-tui/DESIGN.md:24,266,355`). Remove dead tokens or include a warning rationale and regenerate the reports.