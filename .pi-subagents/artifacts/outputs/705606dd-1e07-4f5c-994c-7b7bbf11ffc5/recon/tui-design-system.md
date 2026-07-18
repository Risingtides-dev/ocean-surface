---
name: Ocean TUI
document: DESIGN.md
format: google-labs-design
status: evidence-brief
source_of_truth: ../ocean-os/crates/ocean-tui/src
evidence_date: 2026-07-15
platform: terminal
renderer: ratatui
color_mode: rgb-and-xterm-indexed
tokens:
  color:
    bg-dark: "#060606"
    bg: "#0A0A0A"
    slate: "#141414"
    curline: "#121317"
    hover: "#1B1C21"
    bg-highlight: "#23252B"
    edge: "#2E323C"
    shadow: "#000000"
    foreground: "#FAFCFF"
    muted: "#909098"
    info: "#6AA6FF"
    accent: "#00D7D7"
    accent-deep: "#005FAF"
    success: "#1ED760"
    warning: "#FFB224"
    error: "#FF4D67"
    thinking: "#B794F6"
    orange: "#FF9E64"
  splash-xterm: [17, 19, 25, 31, 38, 44, 50, 87]
  spacing-cells:
    panel-inset: 1
    splitter: 1
    status-separator: 2
  breakpoint-cells:
    minimum-width: 40
    minimum-height: 8
---

# Ocean TUI Design System

## Overview

Ocean TUI is the sole active terminal workbench, rendered with Ratatui from `../ocean-os/crates/ocean-tui/src/shell`; the removed legacy/mesh surfaces are not design sources. Its visual register is restrained, near-black, session-first, and information-dense without ornamental chrome. The eight-row ASCII OCEAN splash and its deep-indigo-to-aqua xterm ramp are the terminal identity. This document records the inspected working-tree implementation; proposal material is identified explicitly in the evidence appendix.

For a Google Labs `DESIGN.md` handoff, retain this YAML token front matter and canonical section order. Terminal-only behavior belongs in the custom sections after **Do's and Don'ts**, not mixed into web CSS guidance.

## Colors

Use only the front-matter palette for TUI chrome. Surfaces progress `bg-dark` → `bg` → `slate` → `hover`/`bg-highlight`; `edge` is the sole divider. Primary text is `foreground`, metadata is `muted`, focused informational titles use `info`, and Ocean interaction/active emphasis uses `accent`. Semantic states are success/warning/error. `thinking` is an implemented violet exception and also colors graph navigation; it conflicts with current web no-purple guidance and requires an explicit product decision before normalization.

The splash paints one solid xterm color per row: `17,19,25,31,38,44,50,87`; never gradient-clip text. Its fade uses `17,18,19,20,DarkGray,238,236`. Program badge beds are Claude `#241830`, Codex `#14210F`, Pi `#0A1A2A`, Ocean `#04252B`.

## Typography

The UI inherits the terminal monospace font; do not specify Poppins or a pixel scale for the TUI. Establish hierarchy through bold, color, terminal modifiers, spacing, and one-cell geometry. Primary content is near-white; metadata is muted. Focused panel titles and Markdown headings are blue bold. Code uses Syntect foregrounds on `#060606`; inline code is cyan on the same bed. Links are cyan-underlined with the URL repeated in muted plain text. Italic, bold, crossed-out, quotes, lists, tasks, and flat-size ATX headings are supported.

Fancy glyphs must be paired with ASCII equivalents through `theme::g`: e.g. `›/>`, `❯/>`, `▾/v`, `☑/[x]`, `☐/[ ]`, `•/-`. Current selection is compile-time (`NERD=true`), not capability detection.

## Layout

Use a one-row title, flexible body, and one-row status frame. Body columns are Sessions rail / one-cell splitter / center workspace / one-cell splitter / Files rail. Center reserves a breadcrumb above Chat, Editor, or Graph and can dock a resizable PTY below a horizontal splitter. The Files rail may divide into tree and component tray. Side rails collapse to zero; the center retains its minimum workspace. Below 40×8 cells, replace the workbench with `window too small`.

Panels use a one-cell inset, two-row header zone (title then hairline), body, and reserved footer. Fit by Unicode display cells and grapheme clusters, never scalar character counts. Drop optional status segments whole under pressure rather than wrapping.

## Elevation & Depth

Terminal depth is color fill and draw order, not shadow blur. Root/title/status use `#060606`/`#0A0A0A`; panels and overlays use `#141414`; selection uses `#23252B`; single `#2E323C` rules separate regions. Overlays call `Clear`, draw an all-side edge border, and render after the workbench. Full-screen image view renders last. Mouse selection is a final reverse-video overlay. Do not emulate web shadows, glow, radii, or translucent elevation in terminal cells.

## Shapes

Use plain rectangular terminal regions. Splitters are one-cell `▏` or `─`; panel hairlines are `─`. Avoid decorative title glyphs and pill chrome. Two-character provider badges are the only compact filled badge beds. Selection is a full-row highlight, not a rounded control. Tables use `│` and `─┼─` with ASCII fallbacks. The splash is ASCII lettering, not the web circular wave mark; the web logo proposal does not supersede terminal identity.

## Components

- **Title bar:** bold workspace basename, dim `›`, muted active surface.
- **Breadcrumb:** eight-character session prefix for chat, full editor path for editor, blank for graph.
- **Panels:** shared slate bed, focused blue-bold or idle muted title, optional right-aligned exceptional state, hairline, muted footer.
- **Bottom control/status bar:** six mouse targets (Sessions, Chat, Editor, Graph, Terminal, Files); active icon receives its surface color, inactive is muted. Status follows model, branch, health, error, activity, tok/s.
- **Status:** model never drops; overflow removes tok/s, activity, branch, then health/error. Healthy state disappears. Warnings are yellow; ordinary context is muted.
- **Command/file/history overlays:** edge border, slate bed, blue-bold title, highlighted selected row, cyan selector, scrolling window that keeps selection visible.
- **Transcript:** structured prose, thinking, tools, permissions, errors, edit diffs, Markdown, image cards.
- **Tool/diff drawers:** collapsed/expanded states, keyboard traversal, running tool name as live activity, sanitized terminal-safe output.
- **Markdown:** flat headings, fenced/inline code, tables capped at 48 cells per column, quotes, rules, lists/tasks, links with copyable targets, image reference cards.
- **Terminal dock:** optional, resizable, separated by one horizontal rule.
- **Image viewer:** full-screen takeover with Kitty graphics where supported and visible textual image cards in transcript.

## Do's and Don'ts

**Do** use the exact palette; preserve solid per-element color; keep state text terse; sanitize daemon/path/tool text; measure display width; preserve grapheme clusters; provide ASCII symbol alternatives; keep healthy/idle chrome quiet; restore terminal state on every exit; keep URLs copyable; treat code syntax colors as separate from UI chrome.

**Don't** infer the active UI from archived room/mockup specs; add decorative icons to panel titles; stack multiple border/shadow columns; wrap the one-row status; show permanent success messages; translate web radii/shadows/fonts literally; assume Kitty or keyboard enhancement exists; treat discovered PNGs as canonical without provenance; silently resolve the violet/no-purple conflict.

## TUI-Specific Runtime Guidance

Splash: center for 700 ms, then quadratic upward slide/fade for 700 ms at ~16 ms frames; any key skips it; undersized terminals omit it. Enable bracketed paste and mouse capture; opportunistically enable Kitty keyboard disambiguation. RAII and panic hooks must restore raw mode, alternate screen, paste, mouse, and keyboard flags. Terminal-fed strings must remove control characters and normalize tabs/newlines. A future improvement should choose fancy versus ASCII glyphs at runtime rather than compile time.

## TUI Interaction States

- **Focused panel:** blue bold title; **unfocused:** muted title.
- **Selected row:** `#23252B` bed, cyan selector/name; **idle live row:** white bold; **roadmap/disabled:** muted or yellow, though current design proposals recommend hiding unfinished commands.
- **Active nav:** semantic surface color; **inactive nav:** muted.
- **Working:** current tool name or `working`; warning/running semantics use yellow.
- **Healthy/recovered:** no segment; **degraded/error/dirty Git:** yellow status segment.
- **Text selection:** reverse video over exact bounded cells.
- **Too small:** yellow `window too small` on deepest background.

## Evidence Appendix

## Scope and evidence posture

This is a read-only handoff brief. **Implemented truth** below means code present in the inspected `../ocean-os` working tree on 2026-07-15, not necessarily committed truth: that repository already had unstaged edits in `crates/ocean-tui/src/shell/{app.rs,components/chat.rs,markdown.rs}` before this scout. Archived specs and web-design targets are explicitly labeled as proposals/cross-surface guidance.

## Files Retrieved

1. `../ocean-os/crates/ocean-tui/src/main.rs` (lines 1-55) — active binary entry point; launches only the native workbench shell.
2. `../ocean-os/crates/ocean-tui/src/splash.rs` (lines 16-124) — canonical ASCII banner, xterm ramp, timing, small-terminal behavior.
3. `../ocean-os/crates/ocean-tui/src/shell/theme.rs` (lines 1-47) — complete active TUI palette and glyph fallback seam.
4. `../ocean-os/crates/ocean-tui/src/shell/app.rs` (lines 3666-3997, 4013-4068) — actual shell geometry, title/status bars, responsive floor, selection state, overlays, splitters, truncation.
5. `../ocean-os/crates/ocean-tui/src/shell/panel.rs` (lines 1-123) — shared panel chrome and Unicode/grapheme-safe fitting.
6. `../ocean-os/crates/ocean-tui/src/shell/status.rs` (lines 1-190) — health semantics, status segment priority, sanitization and width degradation.
7. `../ocean-os/crates/ocean-tui/src/shell/components/chat.rs` (lines 2025-2135) — representative command-palette component and selected/disabled/“soon” styling.
8. `../ocean-os/crates/ocean-tui/src/shell/markdown.rs` (lines 200-655) — transcript typography, code, tables, images, lists, links, and fallbacks.
9. `../ocean-os/crates/ocean-tui/src/shell/tui.rs` (lines 1-105) — terminal lifecycle, paste/mouse/keyboard capability handling and panic restoration.
10. `../ocean-os/docs/.agentarchive/specs/2026-07-09-ocean-tui-current-shell-completion-design.md` (lines 1-80) — archived, under-review proposal with a useful baseline/proposal ledger; not implementation authority.
11. `../ocean-os/docs/AGENT_RENDER_PROTOCOL.md` (lines 335-350) — current component-rendering boundary: bounded TUI projections exist; general component interaction is future.
12. `../ocean-os/docs/ocean-os-site/pages/tui.html` (lines 22-35) — product-site summary of the sole terminal surface and bottom navigation.
13. `docs/OCEAN_WEB_SURFACE_DESIGN.md` (lines 1-115) — cross-surface identity/token authority and explicit TUI-ramp provenance.
14. `docs/OCEAN_PLATFORM_CONTRACT.md` (lines 101-105) — one-design-system rule for shells.
15. `docs/OCEAN_PROJECT_MAP.md` (lines 1-75) — ownership: `ocean-os` owns TUI/runtime; surfaces remain thin clients.
16. `../ocean-os/docs/ocean-os-site/assets/surfaces/{longhouse-deck-live.png,map-render-test.png,model-dropdown-halt.png,tool-group-collapsed.png` — only discovered raster surface evidence; filenames suggest UI captures, but the TUI page does not embed them, so do not treat them as a canonical visual baseline without manual inspection/provenance.

## Key Code

### Implemented truth: identity and exact palette

The startup identity is an eight-row `OCEAN` ASCII wordmark. Each row receives one indexed terminal color: **17, 19, 25, 31, 38, 44, 50, 87** (`splash.rs:16-36`). It holds centered for **700 ms**, then slides upward with quadratic easing while fading through indexed **17, 18, 19, 20, DarkGray, 238, 236** for **700 ms** at ~16 ms frames; any key skips it, and terminals smaller than the banner silently render no banner (`splash.rs:38-50,73-124`).

Active RGB constants (`theme.rs:9-35`):

| Role | Value | Use |
|---|---:|---|
| `BG_DARK` | `#060606` | deepest void/gutters/title+status bars |
| `BG` | `#0A0A0A` | root/editor void |
| `SLATE` | `#141414` | panel bed/overlays |
| `HOVER` | `#1B1C21` | hover row |
| `BG_HL` | `#23252B` | selection/segment |
| `EDGE` | `#2E323C` | dividers/card lines |
| `SHADOW` | `#000000` | faux shadow (declared) |
| `CURLINE` | `#121317` | current line |
| `FG` | `#FAFCFF` | primary text |
| `COMMENT` | `#909098` | metadata/muted |
| `BLUE` | `#6AA6FF` | info, dirs, focused titles |
| `CYAN` | `#00D7D7` | primary Ocean accent |
| `DEEPBLUE` | `#005FAF` | depth shade |
| `GREEN` | `#1ED760` | success |
| `YELLOW` | `#FFB224` | warning/running |
| `RED` | `#FF4D67` | error |
| `MAGENTA` | `#B794F6` | thinking / graph nav; explicit token exception |
| `ORANGE` | `#FF9E64` | secondary semantic accent |
| Claude/Codex/Pi/Ocean badge beds | `#241830`, `#14210F`, `#0A1A2A`, `#04252B` | two-character program badges |

Important mismatch: the active TUI still defines/uses soft violet `MAGENTA` for thinking/graph, while web guidance says no magenta/purple (`docs/OCEAN_WEB_SURFACE_DESIGN.md:52-54`). Preserve as implemented truth in a faithful handoff, but flag for product reconciliation.

### Implemented truth: typography and symbols

Ratatui inherits the operator’s terminal monospace font; there is no selectable TUI font family or pixel type scale. Hierarchy is achieved with `BOLD`, color, underline, italics, crossed-out text, spacing, and one-cell geometry. Panel titles are bold blue when focused and muted gray otherwise; exceptional state is plain right-aligned gray (`panel.rs:49-81`).

`theme::g(nerd, ascii)` is the centralized fancy-glyph/ASCII chooser, but `NERD` is a **compile-time constant currently `true`**, not runtime terminal detection (`theme.rs:37-47`). Examples: `›/>`, `≡/[S]`, `◒/[C]`, `✎/[F]`, `⟠/[G]`, `⊟/[T]`, `◨/[E]`, `❯/>`, `▾/v`, `—/-`; markdown supplies `☑/[x]`, `☐/[ ]`, `•/-`, `▎/|`, box rules and table junction fallbacks (`app.rs:3893-3949`; `chat.rs:2065-2128`; `markdown.rs:239-655`).

Markdown implementation: ATX headings are flat-size blue bold; fenced code uses a dark bed plus Syntect colors; inline code is cyan on dark; bold/italic/strike map to terminal modifiers; links are cyan-underlined followed by a muted copyable URL; quotes use a dim rail; lists use cyan markers; checked tasks green; tables use padded cells, `│` and `─┼─`, with 48-column cell cap; image references become cyan `[img]` cards and `/image` opens Kitty rendering (`markdown.rs:200-295,383-470,473-655`).

### Implemented truth: layout and component vocabulary

The frame is **one-row title / flexible body / one-row status**. Body is `[sessions rail][1-cell splitter][center][1-cell splitter][file rail]`; rails may collapse to zero. Center is breadcrumb + chat/editor/graph, optionally a horizontal splitter and bottom PTY dock. The file rail may split into tree + component tray (`app.rs:3678-3793`). A terminal under **40×8** replaces everything with yellow `window too small` (`app.rs:3666-3675`).

Shared panel skin: `#141414` bed, one-cell horizontal inset, plain bold title, optional exceptional state, `#23252B` hairline, reserved footer. A single `#2E323C` `▏`/`─` splitter replaces stacked panel borders (`panel.rs:22-104`; `app.rs:4052-4068`).

Title bar is `{workspace basename} › {chat|editor|graph}`; breadcrumb is an 8-character session-id prefix or editor path (`app.rs:3753-3767,3868-3903`). Floating overlays use `Clear`, all-side `EDGE` border, `SLATE` bed, blue bold title. Palette rows use `BG_HL` when selected, cyan arrow/name for selected live entries, white bold for idle live entries, and yellow/muted treatment for roadmap entries (`chat.rs:2025-2135`).

Bottom bar intentionally places six mouse navigation affordances near the composer. Active icons get per-surface colors; inactive icons are `COMMENT`; hit targets include two trailing columns (`app.rs:3905-3970`). Mouse text selection uses reverse video and copies exact bounded on-screen cells (`app.rs:3795-3829`).

### Implemented truth: status, tools, interaction states

Status layout is `model  branch  health  error  activity  tok/s`, plain two-space separators. Model is primary/near-white; ordinary branch/activity/rate are muted; degraded health, unresolved error, or exceptional Git are yellow. Under width pressure it drops whole segments in order: tok/s, activity, branch, health/error; model survives and is hard-clipped (`status.rs:93-183`; `app.rs:3972-3997`). Healthy/recovered state disappears rather than showing success chrome. Daemon health outranks SSE health, and each source recovers independently (`status.rs:1-52`). Git format is `branch ~dirty +ahead -behind` (`status.rs:185-190` onward).

Chat data model supports structured text, thinking, streaming tool calls, permission cards, errors, edit diffs, collapse/expand, drawer focus, and a live activity derived as running tool name or `working` (see `components/chat.rs:52-107,260-272,1457-1682`). Tool/diff output is sanitized because terminal tabs/control characters can corrupt ratatui cell painting (`components/chat.rs:428-472`). Representative palette states are documented above; for a pixel-faithful tool-card handoff, next inspect the tool render block around `components/chat.rs:2030+` and diff helpers around `:428-821` against a live capture.

### Implemented truth: accessibility and fallbacks

- Unicode display width and grapheme segmentation prevent truncating wide characters or contextual emoji (`panel.rs:107-123`); status sanitizes daemon-fed newlines/tabs/control characters (`status.rs:83-90`).
- ASCII alternatives exist for most decorative symbols, but are not automatically selected because `NERD=true` is compiled in (`theme.rs:37-47`). This is the main fallback risk.
- Bracketed paste prevents pasted newline submission; mouse capture is enabled; Kitty keyboard enhancement is opportunistic and ignored when unsupported (`tui.rs:44-78`).
- RAII and panic hooks restore raw mode, alternate screen, mouse, paste, and keyboard protocol on errors/panics (`tui.rs:24-105`).
- Tiny screens receive text fallback; splash absence is silent on undersized screens (`app.rs:3666-3675`; `splash.rs:95-99`).
- No evidence of screen-reader semantics exists in the terminal renderer; accessibility is chiefly keyboard operation, plain-text/ASCII alternatives, safe resizing, contrast, and copyable URLs.

## Implemented truth vs proposals

**Trust code first.** `main.rs:29-35` launches `shell::run`; tests explicitly reject removed `--legacy` and `mesh` surfaces (`main.rs:75-96`). The active spatial shell is therefore `crates/ocean-tui/src/shell`, not archived room/mockup documents.

**Archived under-review proposal:** `docs/.agentarchive/specs/2026-07-09-ocean-tui-current-shell-completion-design.md:1-19` labels itself “Approved direction; written design under review.” Its three-column baseline and already-working list (`:21-49`) broadly match code, but its command ledger (`:51-80`) is proposal/history, not proof of current completion. In particular, the inspected chat palette still has a `soon` rendering path (`chat.rs:2087-2128`), despite the spec proposing removal.

**Cross-repo design guidance:** `docs/OCEAN_WEB_SURFACE_DESIGN.md:18-42` canonizes the TUI xterm ramp as Ocean identity but makes the circular neumorphic wave the active **web** logo direction. Do not replace the TUI ASCII splash based on that alone. The same document’s exact web ramp RGBs are `#00005F #0000AF #005FAF #0087AF #00AFD7 #00D7D7 #00FFD7 #5FFFFF` (`:71-81`) and its semantic colors align closely with TUI (`:83-88`). Web typography/radii/shadows (`:95-115`) are not implementable terminal properties; translate hierarchy/state, not CSS geometry.

## Architecture

`main.rs` resolves daemon URL/project/session and enters `shell::run`. The shell client consumes daemon-owned sessions/events/tools/permissions; `App` owns local presentation and geometry; leaf components (`chat`, rail, tree, editor, graph, PTY, tray) draw through Ratatui. `theme.rs` is the concrete color/symbol vocabulary, `panel.rs` shared chrome, `status.rs` pure width-aware status projection, and `markdown.rs` transcript typography. This respects the cross-repo ownership map: `ocean-os` owns runtime and TUI, while `ocean-surface` documents shared visual identity and thin-surface constraints (`docs/OCEAN_PROJECT_MAP.md:12-24,60-75`).

General agent-render components are intentionally bounded: the TUI projects supported render/unmount events and maintains shell-owned tray state, but posting general component interactions is future work (`../ocean-os/docs/AGENT_RENDER_PROTOCOL.md:335-350`).

## Start Here

Open `../ocean-os/crates/ocean-tui/src/shell/theme.rs` first for the exact vocabulary, then `shell/app.rs:3666-3997` for composition and state hierarchy. For transcript/tool fidelity, continue with `shell/components/chat.rs` and `shell/markdown.rs`.

## Residual risks / open questions

- The inspected `ocean-os` TUI files were already unstaged-modified, so this captures the active working tree, not a clean commit. A handoff intended for release must compare these ranges with `HEAD` and the deployed binary.
- No canonical current TUI screenshot is linked from `docs/ocean-os-site/pages/tui.html`; discovered PNGs need manual provenance review.
- Runtime glyph capability detection is absent; ASCII fallback requires rebuilding with `NERD=false`.
- TUI violet usage conflicts with current web “no magenta/purple” guidance; product owner should decide whether thinking/graph are grandfathered terminal semantics.
- This evidence review did not launch an interactive terminal, inspect the four PNG pixels, or validate real terminal color remapping.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Produced only the requested read-only evidence brief; no project/source file was edited."
    },
    {
      "id": "criterion-2",
      "status": "satisfied",
      "evidence": "Brief cites active entry points, exact colors, line-ranged implementation evidence, proposal boundaries, architecture, fallbacks, screenshots, and residual risks."
    }
  ],
  "changedFiles": [
    ".pi-subagents/artifacts/outputs/705606dd-1e07-4f5c-994c-7b7bbf11ffc5/recon/tui-design-system.md"
  ],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    {
      "command": "find/grep/read targeted ocean-tui source and Ocean design/project docs",
      "result": "passed",
      "summary": "Mapped active TUI entry point, palette, splash, layout, status, markdown, terminal behavior, docs, and image assets."
    },
    {
      "command": "git status --short && git -C ../ocean-os status --short",
      "result": "passed",
      "summary": "Confirmed both worktrees already contain unrelated unstaged changes; ocean-os includes unstaged TUI edits."
    },
    {
      "command": "git diff --cached --name-only; git -C ../ocean-os diff --cached --name-only",
      "result": "passed",
      "summary": "Both staged-file lists were empty."
    },
    {
      "command": "nl -ba ... | sed -n ...",
      "result": "passed",
      "summary": "Verified exact line ranges for cited palette, splash, app layout, status, panel, chat, and design documents."
    }
  ],
  "validationOutput": [
    "Artifact written at the authoritative requested path.",
    "No source/project file modifications were made by this scout.",
    "No staged files exist in either inspected repository."
  ],
  "residualRisks": [
    "Evidence reflects a dirty ocean-os working tree with pre-existing unstaged TUI edits, not necessarily HEAD or deployed behavior.",
    "No interactive TUI smoke test or screenshot provenance inspection was performed.",
    "NERD glyph fallback is compile-time rather than runtime-detected.",
    "Active TUI violet conflicts with current web no-purple guidance."
  ],
  "noStagedFiles": true,
  "diffSummary": "Added one reconnaissance markdown artifact only; project/source trees left untouched.",
  "reviewFindings": [
    "no blockers for the evidence brief",
    "warning: release handoff should reconcile dirty working-tree evidence with HEAD/deployed binary",
    "warning: reconcile TUI MAGENTA usage with cross-surface no-purple guidance"
  ],
  "manualNotes": "Review gate remains required. The four discovered PNG assets were catalogued but not treated as canonical screenshots without provenance."
}
```
