## Review

- **Blocker — High:** `design-systems/ocean-tui/DESIGN.md:314-316` misrepresents permission cards. The implementation stores only tool/reason and intentionally renders no key instructions or args (`../ocean-os/crates/ocean-tui/src/shell/components/chat.rs:79-86, 3140-3183, 4753-4773`). **Smallest safe fix:** describe the current two-line tool/reason warning and Ctrl-Y/Ctrl-N bindings; remove claims that args and explicit keys appear on-card.

- **Blocker — High:** Violet semantics contradict both source and the handoff itself. `DESIGN.md:195-197,361,396-398` calls violet the implemented thinking color and prohibits it in navigation, while thinking is muted italic and violet is used for Graph navigation/config nodes (`chat.rs:3185-3194`, `app.rs:4061-4065`, `components/graph.rs:215-219`). **Smallest safe fix:** document muted thinking accurately and identify violet as the current Graph/config accent.

- **Medium:** YAML defines `status-error` as red (`DESIGN.md:139-144`), but all status health/errors map to warning yellow (`../ocean-os/crates/ocean-tui/src/shell/status.rs:42-54,133-140`; `app.rs:4095-4100`). **Smallest safe fix:** use `{colors.warning}` for `status-error`, reserving red for transcript failures/denials.

- **Medium:** Divider-color guidance is inaccurate. `DESIGN.md:178,262,273` directs hairlines to `{colors.edge}`, while panel hairlines use selected-fill `BG_HL`; only splitters/bounded frames use `EDGE` (`panel.rs:75-80`, `app.rs:2438-2440`). **Smallest safe fix:** distinguish panel hairlines from structural splitters/borders.

- **Medium:** Strict ASCII generation is underspecified and internally ambiguous. `DESIGN.md:276-277,370-387,436` requests strict fallback views but gives no exact fallback for progress, bounded frames, trees, charts, or gallery; both complete and error map to `x`. Current render projections also hard-code Unicode (`chat.rs:465-475,490-494,540-567,630-660,727-770,785-787`). **Smallest safe fix:** provide explicit ASCII recipes and require adjacent state text wherever `x` is shared.

- **Low:** The unused `colors.shadow` token (`DESIGN.md:22`) conflicts with repeated no-shadow guidance and is reported unused in `lint-report.json`. **Smallest safe fix:** remove it from the handoff so generators cannot treat it as an available elevation token.

- **Low:** Front matter expresses terminal cells as CSS `rem` values (`DESIGN.md:69-77,92-96`), including `minimum-width-cells: 40rem`, which can invite web-style geometry. **Smallest safe fix:** explicitly state these are interchange aliases for terminal columns/rows, not CSS dimensions.

- **Medium — validation:** The referenced TUI suite is not green: 336 passed, 1 failed, 4 ignored. `shell::app::tests::drag_past_a_pane_edge_clamps_the_head_into_the_rect` fails at `app.rs:4625`, including in isolation. **Smallest safe fix:** repair that dirty sibling-repo selection regression before claiming mouse-selection QA is passing.

- **Correct:** YAML parses, all 23 token references resolve, and the canonical section sequence—Overview, Colors, Typography, Layout, Elevation & Depth, Shapes, Components—is intact. The splash asset exactly matches all eight source lines and xterm indices. The AI prompts strongly prohibit web chrome, pseudo-code, gradients, rounded cards, and invented branding.

- **Note — Low:** Requested `/plan.md` and `/progress.md` were absent, so no plan/progress claims could be checked.