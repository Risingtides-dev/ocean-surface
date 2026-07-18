---
version: alpha
name: Ocean TUI
description: A restrained, near-black terminal workbench whose hierarchy is built from cells, solid color, typography modifiers, and one-row interaction patterns.
colors:
  primary: "#00D7D7"
  on-primary: "#03181A"
  logo-depth: "#005FAF"
  ocean-1: "#00005F"
  ocean-2: "#0000AF"
  ocean-3: "#005FAF"
  ocean-4: "#0087AF"
  ocean-5: "#00AFD7"
  ocean-6: "#00D7D7"
  ocean-7: "#00FFD7"
  ocean-8: "#5FFFFF"
  background-deep: "#060606"
  background: "#0A0A0A"
  panel: "#141414"
  current-line: "#121317"
  hover: "#1B1C21"
  selected: "#23252B"
  edge: "#2E323C"
  text-primary: "#FAFCFF"
  text-muted: "#909098"
  info: "#6AA6FF"
  success: "#1ED760"
  warning: "#FFB224"
  error: "#FF4D67"
  thinking: "#B794F6"
  orange: "#FF9E64"
  badge-claude: "#241830"
  badge-codex: "#14210F"
  badge-pi: "#0A1A2A"
  badge-ocean: "#04252B"
typography:
  body:
    fontFamily: ui-monospace
    fontSize: 1rem
    fontWeight: 400
    lineHeight: 1
  body-bold:
    fontFamily: ui-monospace
    fontSize: 1rem
    fontWeight: 700
    lineHeight: 1
  metadata:
    fontFamily: ui-monospace
    fontSize: 1rem
    fontWeight: 400
    lineHeight: 1
  panel-title:
    fontFamily: ui-monospace
    fontSize: 1rem
    fontWeight: 700
    lineHeight: 1
    letterSpacing: 0em
  code:
    fontFamily: ui-monospace
    fontSize: 1rem
    fontWeight: 400
    lineHeight: 1
  status:
    fontFamily: ui-monospace
    fontSize: 1rem
    fontWeight: 400
    lineHeight: 1
rounded:
  none: 0px
spacing:
  cell: 1rem
  status-separator: 2rem
  panel-title-zone: 2rem
  minimum-width-cells: 40rem
  minimum-height-cells: 8rem
components:
  root:
    backgroundColor: "{colors.background}"
    textColor: "{colors.text-primary}"
    typography: "{typography.body}"
    rounded: "{rounded.none}"
  title-bar:
    backgroundColor: "{colors.background-deep}"
    textColor: "{colors.text-primary}"
    typography: "{typography.body-bold}"
    rounded: "{rounded.none}"
    height: 1rem
  panel:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.text-primary}"
    typography: "{typography.body}"
    rounded: "{rounded.none}"
    padding: 1rem
  panel-title-focused:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.info}"
    typography: "{typography.panel-title}"
    rounded: "{rounded.none}"
    height: 1rem
  panel-title-idle:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.text-muted}"
    typography: "{typography.panel-title}"
    rounded: "{rounded.none}"
    height: 1rem
  selected-row:
    backgroundColor: "{colors.selected}"
    textColor: "{colors.text-primary}"
    typography: "{typography.body-bold}"
    rounded: "{rounded.none}"
    height: 1rem
  hover-row:
    backgroundColor: "{colors.hover}"
    textColor: "{colors.text-primary}"
    typography: "{typography.body}"
    rounded: "{rounded.none}"
    height: 1rem
  overlay:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.text-primary}"
    typography: "{typography.body}"
    rounded: "{rounded.none}"
    padding: 1rem
  code-well:
    backgroundColor: "{colors.background-deep}"
    textColor: "{colors.text-primary}"
    typography: "{typography.code}"
    rounded: "{rounded.none}"
    padding: 1rem
  status-bar:
    backgroundColor: "{colors.background-deep}"
    textColor: "{colors.text-muted}"
    typography: "{typography.status}"
    rounded: "{rounded.none}"
    height: 1rem
  status-warning:
    backgroundColor: "{colors.background-deep}"
    textColor: "{colors.warning}"
    typography: "{typography.status}"
    rounded: "{rounded.none}"
    height: 1rem
  status-error:
    backgroundColor: "{colors.background-deep}"
    textColor: "{colors.warning}"
    typography: "{typography.status}"
    rounded: "{rounded.none}"
    height: 1rem
  provider-badge-ocean:
    backgroundColor: "{colors.badge-ocean}"
    textColor: "{colors.primary}"
    typography: "{typography.body-bold}"
    rounded: "{rounded.none}"
    width: 2rem
---

# Ocean TUI

## Overview

Ocean TUI is a **dark operations console built as a precise field of terminal cells**. It is compact, session-first, and information-dense without becoming noisy. It should feel like a serious native terminal workbench, not a web app translated into box characters and not a retro-computing theme.

Its identity is the eight-row ASCII OCEAN banner rising through deep indigo into bright aqua. Once the splash clears, brand energy becomes scarce. The workbench is mostly near-black panels, muted metadata, clear white content, single-cell dividers, and a cyan focus/selection signal.

The terminal is an equal first-party surface attached to the same daemon-owned project, workspace, session, turn, and event model. It renders truth, gathers intent, and exposes keyboard-efficient action. It never moves runtime authority into presentation.

Terminal constraints are part of the design language: one-cell geometry, solid colors, mono type, Unicode display width, graceful ASCII fallback, and predictable redraw order. Do not imitate CSS radii, blur, translucent panels, or soft shadows.

## Colors

### Base field

- `{colors.background-deep}` is the deepest void: title/status bars, gutters, code beds, and full-screen fallback.
- `{colors.background}` is the root and editor field.
- `{colors.panel}` is the panel and overlay bed.
- `{colors.current-line}` is a restrained editor current-line fill.
- `{colors.hover}` is blurred selection or mouse hover.
- `{colors.selected}` is focused selection and segmented emphasis.
- `{colors.edge}` is the structural divider color for pane splitters, overlay borders, table rules, and bounded component frames. Panel title hairlines use the quieter `{colors.selected}` so they do not compete with structure.

Primary text is `{colors.text-primary}`. Context, age, hints, paths, and inactive chrome use `{colors.text-muted}`. Focused panel titles and directory-level information use `{colors.info}`. Ocean action/focus emphasis uses `{colors.primary}`.

### Identity ramp

The splash uses xterm indices **17, 19, 25, 31, 38, 44, 50, 87**, corresponding to the eight RGB Ocean tokens in front matter. Paint one solid color per row. Do not use terminal escape tricks to simulate gradient text within a row.

The fade uses xterm 17, 18, 19, 20, DarkGray, 238, and 236. The banner holds centered for 700ms, then slides upward and fades for 700ms. Any key skips it; terminals too small for the banner show no partial logo.

### Semantic state

- `{colors.success}`: complete, connected, checked task, live session marker.
- `{colors.warning}`: running, degraded, permission waiting, dirty or uncertain state.
- `{colors.error}`: failed, denied, destructive, invalid.
- `{colors.info}`: focused structural title, directory, or informational heading.
- `{colors.thinking}`: current Graph/config accent. Thinking prose itself is muted italic `{colors.text-muted}`. Violet is a terminal-specific semantic exception, not an Ocean brand color; never use it for identity or general navigation chrome.
- `{colors.orange}`: rare secondary semantic distinction, not a substitute accent.

Healthy/recovered status disappears instead of painting permanent green success chrome.

### Provider badge beds

Claude, Codex, Pi, and Ocean may use two-character filled badge beds. These are compact provenance markers, not rounded brand pills and not a palette for general components.

## Typography

The TUI inherits the operator's terminal monospace font and scale. Do not prescribe Poppins, pixel-specific hierarchy, or a display font. Every UI role remains one terminal text size.

Hierarchy comes from:

1. bold versus regular,
2. primary versus muted color,
3. one-cell indentation and rules,
4. uppercase labels,
5. underlining only for links or active text semantics,
6. reverse video only for final mouse selection or intra-line diff emphasis.

Focused panel titles and Markdown headings are blue bold. Unfocused titles are muted. Primary transcript text is near-white. Metadata and contextual URLs are muted. Inline code is cyan on the deepest bed. Code fences use Syntect syntax colors on `{colors.background-deep}`; syntax highlighting is content, not UI chrome, and may keep its own palette.

ATX headings remain flat-size and bold rather than growing. Lists use cyan markers. Checked tasks use green. Links use cyan underline followed by a muted, copyable URL. Quotes use a dim vertical rail. Bold, italic, strikethrough, and word-level diff reverse video are valid content modifiers.

## Layout

The DESIGN.md spacing values express an interchange alias: **1rem means one terminal cell** in this document. `40rem × 8rem` means 40 columns × 8 rows, not a CSS viewport prescription. The Ratatui implementation remains cell-native.

The full frame is:

```text
one-row title bar
flexible body
one-row status/control bar
```

The body is:

```text
SESSIONS rail │ center workspace │ FILES rail
```

Each separator is exactly one cell. The center contains a breadcrumb plus Chat, Editor, or Graph. A resizable embedded PTY may dock beneath the center behind one horizontal splitter. The Files rail may divide into file tree and a session-bound component tray.

Side rails collapse when width is constrained; the center retains the useful workspace. Below 40×8 cells, replace the workbench with one yellow `window too small` message. Do not let components smear, wrap unpredictably, or paint partial chrome.

### Panel anatomy

A panel uses a one-cell horizontal inset and a two-row header zone:

1. plain title with optional exceptional state aligned right,
2. one `{colors.selected}` hairline.

The flexible body follows, with one reserved footer row. A panel does not draw its own stacked left highlight, right shadow, and outer border. Adjacent regions share one splitter.

### Width behavior

Measure Unicode display cells and grapheme clusters, not bytes or scalar character counts. Keep selected rows visible by scrolling. Truncate with a cell-safe ellipsis. Preserve useful suffixes such as file extensions. Status and metadata must drop whole optional segments rather than wrap.

## Elevation & Depth

Terminal depth is created by **tonal beds and draw order**:

- deepest background for global frame and code,
- root background for workspace/editor,
- panel bed for rails and overlays,
- selected fill for focused rows,
- one edge color for dividers and bounded artifacts.

Overlays call Clear, paint after the workbench, use an all-side edge border, and retain the panel bed. A full-screen image viewer paints last. Mouse text selection is the final reverse-video overlay.

Do not emulate browser shadows with extra columns, half-block gradients, faux blur, or multiple nested boxes. Do not use transparency. No visual layer should require a specific terminal background image or color theme to remain readable.

## Shapes

All TUI geometry is rectangular and cell-aligned. Radius is zero.

- Vertical splitter: `▏` with `|` fallback.
- Horizontal splitter and panel hairline: `─` with `-` fallback.
- Focused row marker: `▎` with `|` fallback.
- Expanded/collapsed: `▾`/`▸` with `v`/`>` fallback.
- Tables: `│`, `─`, and `┼` with ASCII alternatives.
- Progress: `█` and `░` with readable numeric percentage.
- Two-character provider beds are the only filled badge geometry.

Avoid decorative glyphs in panel titles. Selection is a full-row tonal change plus a one-cell marker, not a rounded chip. The circular web wave mark does not replace the ASCII terminal identity.

## Components

### Splash

Eight fixed ASCII lines form OCEAN. Center when the terminal can contain the full banner. Never crop, wrap, add a slogan, or mix the circular mark into the splash. Motion is skippable and brief.

### Title bar and breadcrumb

The title bar is `{workspace basename} › {chat|editor|graph}`. The workspace is bold primary text; the separator and current surface are quieter. Chat breadcrumb shows a short session identifier; Editor shows the project-relative file path; Graph may remain blank when no additional location is needed.

### Shared panels

Sessions, Files, Editor, Graph, and component trays share one panel skin: panel bed, plain title, hairline, content body, and muted footer. Focused title is blue bold; unfocused title is muted. Exceptional state such as `unsaved` appears as plain right-aligned metadata, never as a badge row.

### Rails and selection

Sessions and Files use one-row list items. Focused selection has `{colors.selected}` across the full row, a cyan single-cell marker, and bold primary text. Blurred selection uses `{colors.hover}` and a muted marker. Normal rows remain on the panel bed.

Directory nodes are blue, branch nodes cyan, files and sessions near-white, and metadata muted. Depth guides are tonal rather than bright. Reveal row-level actions only on the selected header.

### Transcript

The transcript is structured into user prompts, assistant Markdown, thinking, tool drawers, permissions, error notices, diffs, and bounded render-protocol components. It is not a field of chat bubbles.

Assistant Markdown uses flat-size blue headings, readable white body, cyan list markers and links, dark code beds, and muted quote/table rules. URLs remain visible and copyable. Image references render as textual cards; Kitty graphics may enhance them but cannot be the only representation.

### Tool and diff drawers

Collapsed tool rows show tool name, a cell-safe salient argument preview, and state. Expanded bodies show sanitized terminal-safe args/output with a bounded recent tail. Running activity names the active tool when known. Keyboard traversal and collapse/expand must not steal ordinary composer Space or Enter.

Diff rows use red deletion gutters, green addition gutters, muted context, and cyan hunk information. Intra-line changes may use reverse video. Tabs and control bytes are normalized before rendering.

### Permissions and confirmations

The current pending permission surface is a compact two-line warning card: tool name, then plain-language reason. Approval and denial are handled by **Ctrl-Y** and **Ctrl-N**; the card does not currently render args or key instructions. Waiting uses warning color; allow resolves green; deny/failure resolves red. The decision must remain keyboard reachable and must not leak typed Y/N into the composer. A future richer card may add args or visible key hints only when the implementation does so truthfully.

### Composer and palettes

The composer is a multi-line terminal field with a block cursor and one cyan leading/accent cue. Enter submits; the configured newline chord inserts a newline. Bracketed paste preserves content and never synthesizes submission.

Slash commands, file mentions, model/provider selection, and history search appear as floating overlays directly above the composer. Each overlay has one border, panel bed, blue bold title, cyan selection marker, and a scroll window that follows selection. Disabled or future commands are muted and labeled honestly rather than styled as active.

### Editor

The editor uses a six-cell gutter for Git mark plus line number. Added is green, modified amber, deleted red. Source code scrolls horizontally; prose formats wrap softly. Cursor-following pauses after mouse-wheel scrolling and resumes after keyboard input. Dirty state is plain title metadata.

### Status and bottom navigation

Bottom navigation exposes Sessions, Chat, Editor, Graph, Terminal, and Files as compact mouse/keyboard targets. Active surface receives its semantic color; inactive targets are muted. Keep labels or tooltips discoverable through documented keys; do not rely on obscure glyph meaning alone.

Status order is model, branch, health/error, activity, and token rate. Model survives every width reduction. Drop token rate first, then activity, then branch, then health/error. Never wrap the status line. Healthy state renders nothing.

### Render-protocol components

Progress, stat, chart, timeline, callout, confirm, table, code, diff, file tree, and gallery projections use a narrow bounded frame, one-cell rules, compact rows, and semantic color. Limit rows and columns. Keep the underlying payload authoritative; terminal layout is a local projection.

## Do's and Don'ts

### Do

- Use exact RGB tokens and the canonical xterm splash indices.
- Keep brand cyan scarce and structural.
- Measure display cells and preserve grapheme clusters.
- Sanitize control characters, tabs, carriage returns, and terminal escapes from untrusted text.
- Pair every fancy glyph with an ASCII fallback.
- Keep healthy/idle chrome quiet and status truthful.
- Drop optional status segments whole under width pressure.
- Preserve keyboard operation, bracketed paste, mouse selection, and terminal restoration on exit.
- Keep URLs copyable and text alternatives visible when Kitty graphics are used.
- Treat Syntect syntax colors as content highlighting, not UI palette expansion.

### Don't

- Don't translate web shadows, radii, glass, blur, Poppins, or glow into terminal decoration.
- Don't stack multiple separator, edge, and faux-shadow columns.
- Don't use decorative glyphs in panel titles.
- Don't wrap the one-row title or status bars.
- Don't show permanent healthy/success messages.
- Don't assume Nerd Font, Kitty graphics, enhanced keyboard protocol, truecolor, or mouse support without fallback.
- Don't use violet `{colors.thinking}` as Ocean identity or general accent.
- Don't use emoji as required interface symbols.
- Don't let image generation invent a terminal logo, typeface, or unreadable pseudo-code.
- Don't treat archived mockups as stronger authority than the active Ratatui implementation.

## Terminal Identity and Glyphs

The ASCII banner is the canonical TUI identity and is reproduced in `assets/OCEAN_ASCII_BANNER.txt`. It is not a low-resolution version of the circular web mark. Keep the two expressions related through the same depth ramp, not by combining their geometry.

Fancy glyphs are enhancements. Production design always specifies the pair:

| Meaning | Preferred | ASCII fallback |
|---|---:|---:|
| hierarchy separator | `›` | `>` |
| focused row | `▎` | `|` |
| blurred row | `▏` | `|` |
| expanded | `▾` | `v` |
| collapsed | `▸` | `>` |
| complete | `✓` | `x done` |
| active/live | `●` | `* live` |
| pending | `○` | `o pending` |
| error | `✗` | `! error` |
| unchecked task | `☐` | `[ ]` |
| checked task | `☑` | `[x]` |
| bullet | `•` | `-` |

A new glyph must remain unambiguous at one cell, avoid width-2 emoji presentation, and have a tested fallback. When ASCII symbols overlap, adjacent state text disambiguates them. A future implementation should detect capability at runtime rather than compile time.

Strict ASCII component recipes:

- Progress: `[####......] 40%`.
- Bounded frame: `+--- title ---+`, `| body |`, `+--------------+`.
- Tree: `|-- child`, `` `-- last ``, and `> / v` for collapsed/expanded.
- Chart: `#` bars plus a numeric value; never rely on block-height glyphs alone.
- Gallery/image: `[img] caption  path`.
- Success/error: `x done` versus `! error`; never a bare shared `x`.

Some current render-protocol projections still hard-code Unicode. Treat full ASCII projection coverage as implementation debt, not as evidence that fallback is already complete.

## Interaction and State Map

- Focused panel: blue bold title.
- Unfocused panel: muted title.
- Focused row: selected bed, cyan marker, bold primary text.
- Blurred selection: hover bed, muted marker, regular text.
- Live session: green dot independent of selection.
- Running tool: tool name plus amber state.
- Thinking: muted italic text.
- Graph/config accent: violet, terminal-specific and not brand.
- Permission wait: amber bounded decision.
- Completed: green marker, then quiet.
- Failed/denied: red notice and recoverable action.
- Dirty editor/Git uncertainty: amber.
- Text selection: reverse video over exact bounded cells.
- Terminal too small: one yellow fallback message on deepest background.

## Terminal Adaptation and Accessibility

A terminal cannot provide the same semantic tree as a browser, so accessibility depends on robust keyboard behavior, plain text, contrast, fallbacks, and safe resizing.

- Keep all core actions keyboard operable.
- Do not encode state through color alone; pair it with text or a glyph.
- Avoid wide emoji and combining sequences in fixed chrome.
- Fit and truncate by grapheme/display width.
- Restore raw mode, alternate screen, mouse capture, bracketed paste, and keyboard flags on normal exit and panic.
- Preserve visible URLs and textual image descriptions.
- Permit operator-controlled terminal font size and theme, but paint explicit RGB UI backgrounds so hierarchy remains stable.
- If truecolor is unavailable, map to the closest xterm-256 role while preserving semantic distinctions.

## AI Asset Generation Brief

Image generation is useful for **glyph concept sheets, Kitty-compatible illustration experiments, terminal component mockups, and palette studies**. It does not produce production terminal text, Unicode assignments, or final cell geometry by itself.

A production TUI icon is ultimately one of:

1. a one-cell Unicode glyph plus ASCII fallback,
2. a short bracketed text label,
3. a Kitty-compatible raster enhancement with a complete text alternative.

Do not ask an image model to generate the ASCII OCEAN banner, readable source code, final labels, or a replacement terminal font. Generated mockup text is layout placeholder only.

### Copy-ready TUI glyph concept prompt

> Design a **[pixel dimensions]** transparent RGBA concept sheet for **[N] Ocean TUI glyphs** representing **[features]**. Each concept must reduce to a single terminal cell or a two-character ASCII label. Use flat one-color geometry, no gradients, no shadows, no rounded containers, no emoji rendering, and no decorative wave motif. The family should feel like a precise operations console, with clear silhouettes at 12×12 pixels. Show a preferred Unicode-style glyph beside a plain ASCII fallback for each concept. Use `{colors.primary}` `#00D7D7` only for focus/active examples on `{colors.background-deep}` `#060606`; use `{colors.text-muted}` `#909098` for inactive examples. No terminal prompt/cursor brand symbolism, sparkles, mascots, purple branding, or fake code text. Deliver a transparent PNG sheet plus a table of semantic name, preferred glyph concept, ASCII fallback, and expected cell width.

### Copy-ready TUI component prompt

> Create a high-fidelity **[pixel dimensions]** opaque terminal-grid study for the **Ocean TUI [component]** at **[columns]×[rows]**. Use a monospaced one-cell grid, square corners, `{colors.background-deep}` `#060606`, `{colors.background}` `#0A0A0A`, `{colors.panel}` `#141414`, `{colors.selected}` `#23252B`, `{colors.edge}` `#2E323C`, `{colors.text-primary}` `#FAFCFF`, `{colors.text-muted}` `#909098`, `{colors.info}` `#6AA6FF`, scarce active `{colors.primary}` `#00D7D7`, success `{colors.success}` `#1ED760`, running/degraded `{colors.warning}` `#FFB224`, and transcript failure `{colors.error}` `#FF4D67`. Use one-cell dividers, one-row title/status bars, full-row selection, and explicit focused/blurred/disabled/running/error states with a non-color text or glyph cue. No shadows, blur, transparency, rounded cards, glassmorphism, giant ASCII art, emoji, generated pseudo-code, or web-style buttons. Supply both preferred Unicode and strict ASCII-fallback PNG views plus a cell-by-cell reconstruction map.

### Copy-ready Kitty image prompt

> Create a **[image role]** for optional Kitty terminal display, **[pixel dimensions]**, on transparent or `{colors.background-deep}` `#060606`. It must remain useful when replaced by the textual caption **[caption]**. Use the Ocean deep-indigo-to-aqua ramp as restrained solid regions, not gradient text. No generated labels, terminal prompt symbols, sparkles, mascots, magenta/purple branding, glassmorphism, cyberpunk neon, or device frame. Deliver a clean PNG plus a one-color thumbnail and state the exact text fallback.

## Asset Delivery and QA

For glyph work, deliver a table containing semantic name, preferred glyph, Unicode code point when known, ASCII fallback, expected cell width, and screenshots in at least two terminal fonts. Test under truecolor and xterm-256, with Nerd Font present and absent, and at narrow/short terminal sizes.

For Kitty raster work, provide PNG dimensions, alpha behavior, terminal-cell target size, fallback caption, and clearing/redraw behavior. Verify the UI remains complete when image support is disabled.

For component concepts, include 80×24, 120×36, 160×48, and 40×8 fallback views. Generated output remains a visual proposal until reconstructed as Ratatui cells and tested for Unicode width, keyboard interaction, redraw stability, and ASCII fallback.
