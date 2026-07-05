# Ocean Web Surface — Design System

Authority for all visual work in the Leptos web surface (`crates/ocean-surface-ui`,
`styles/*.css`, `index.html`). Ocean's OWN identity — not an agency re-brand —
applied at **product register**: the UI serves the task; brand energy is
rationed, never decorative.

Register rule: a user fluent in Linear/Figma/Raycast should trust every control
on sight. Familiar affordances, one component vocabulary, restrained color.

**Control density is a design defect.** The surface holds exactly the controls
the current moment needs; everything else is conditional, collapsed behind one
overflow, or revealed on intent (hover/focus). A row of same-weight buttons is
failure — one primary affordance, quiet metadata, ghost triggers.

## 1. Identity

The OCEAN depth ramp — the ASCII splash banner from the TUI
(`ocean-os/crates/ocean-tui/src/splash.rs`), deep indigo rising to bright aqua
(xterm 17→19→25→31→38→44→50→87). The banner IS the logo: the landing hero
renders it verbatim with one solid ramp color per row; the header wordmark
"OCEAN" carries the ramp one solid color per letter. Never gradient-clip text —
the ramp is always painted as solid colors on discrete elements.

The page is near-black; the working accent (`--ocean-6` cyan) appears ONLY at:

- Primary actions (send, approve, join, submit, create) — solid cyan fill with
  dark ink (`--fg-on-accent`), never white text, never a gradient fill.
- Live/active states: voice orb recording, speaking ring, live dots, active
  selection fills.
- Focus rings (single cyan ring).

Everything else is neutral: near-black surfaces, low-opacity white borders,
the near-white/grey text ramp. No teal-mint (legacy #7fe7c8), no navy chrome,
no magenta/purple (rejected 2026-07-04).

## 2. Tokens (`styles/tokens.css` — the only place colors are defined)

Surfaces
- `--bg: #060606` page
- `--bg-raised: #0A0A0A` bars, panels, drawers
- `--bg-elevated: #141414` cards, inputs, chips, wells
- `--bg-hover: #1B1C21` hover fill on elevated surfaces
- `--bg-well: #23252B` special data wells (code, diff, transcript wells)

Text
- `--fg: #FAFCFF` headings, primary content
- `--fg-2: #B8B9BB` body secondary
- `--fg-3: #909098` labels, metadata
- `--fg-4: #5A5C63` disabled only — never for readable content

Brand — the OCEAN ramp
- `--ocean-1..8`: `#00005F #0000AF #005FAF #0087AF #00AFD7 #00D7D7 #00FFD7 #5FFFFF`
  (identity surfaces only: banner rows, wordmark letters, data-fill gradient)
- `--accent: var(--ocean-6)` interactive accent (links, active text, hot states)
- `--accent-bright: var(--ocean-7)` live highlights
- `--accent-deep: var(--ocean-4)` depth end of data gradients
- `--fg-on-accent: #03181A` ink on accent fills — accent fills NEVER take white text
- `--gradient: linear-gradient(180deg, var(--ocean-6), var(--ocean-4))` DATA
  fills (chart bars) only — never buttons, never text
- `--accent-soft: rgba(0,215,215,0.12)` tinted fills
- `--accent-ring: rgba(0,215,215,0.45)` focus ring color

Status (semantic; used for tool state, call state, deltas, diffs)
- `--ok: #1ED760`, `--ok-soft: rgba(30, 215, 96, 0.12)`
- `--err: #FF4D67`, `--err-soft: rgba(255, 77, 103, 0.12)`
- `--warn: #FFB224`, `--warn-soft: rgba(255, 178, 36, 0.12)`
- `--info: #6AA6FF`, `--info-soft: rgba(106, 166, 255, 0.12)`
- `--running: var(--warn)` (in-flight tool state)

Borders
- `--border-subtle: rgba(255,255,255,0.06)`
- `--border: rgba(255,255,255,0.10)`
- `--border-strong: rgba(255,255,255,0.20)`

Type
- `--font: 'Poppins', system-ui, -apple-system, 'Segoe UI', sans-serif`
- `--mono: ui-monospace, 'SF Mono', SFMono-Regular, Menlo, monospace`
- Scale (fixed px, product register): 11 label / 12 small / 13 secondary /
  14 body / 15 emphasized / 18 title / 22 display. Body line-height 1.55.
- Uppercase labels: 11px, weight 600, `letter-spacing: 0.08em`, `--fg-3`.
- Mono is for DATA (paths, ids, tokens, numbers, code), never for UI labels.

Radii
- `--radius-sm: 8px` small chips, inline pills' inner elements
- `--radius: 12px` inputs, buttons, list items
- `--radius-lg: 16px` cards, panels, composer
- `--radius-pill: 999px` pills, chips, badges, orb

Shadows / glow
- `--shadow-sm: 0 2px 8px rgba(0,0,0,0.40)`
- `--shadow-md: 0 8px 24px rgba(0,0,0,0.55)`
- `--shadow-lg: 0 24px 64px rgba(0,0,0,0.65)` overlays/slide-overs only
- `--glow-brand: 0 0 32px rgba(0,215,215,0.28)` voice orb / live moments only

Motion
- `--ease: cubic-bezier(.22,.61,.36,1)`; `--dur-fast: 160ms`; `--dur: 240ms`
- Motion conveys state only. No bounce, no elastic, no entrance choreography.
- EVERY animation gets a `@media (prefers-reduced-motion: reduce)` kill.

Controls & layout
- `--ctl-h: 34px` standard control height; `--ctl-h-sm: 28px` compact chips
- `--focus-ring: 0 0 0 2px var(--accent-ring)`
- `--shell-max: 1120px` desktop shell width

## 3. Control recipes (the ONE vocabulary — every domain uses these verbatim)

Primary button (send, approve, submit, join, create):
- `background: var(--accent)`, `color: var(--fg-on-accent)` (dark ink — never
  white), weight 600, `--radius`, compact height (`--ctl-h-sm` in docks),
  padding 0 14px, border: none.
- Hover: `filter: brightness(1.12)`. Press: `transform: scale(0.985)`.
- Disabled: `opacity: 0.45`, no pointer events.

Secondary button (cancel, leave, toggles, header actions):
- `background: var(--bg-elevated)`, `1px solid var(--border)`, `--fg` text,
  `--radius`, height `--ctl-h`.
- Hover: `background: var(--bg-hover); border-color: var(--border-strong)`.
- NEVER pair a border with a wide soft shadow.

Ghost button (icon buttons, close ×, drawer heads):
- Transparent bg, `--fg-3` content; hover: `--bg-elevated` fill + `--fg`.

Danger: secondary shape with `--err` text; fill `--err-soft` on hover.
Deny/halt follows danger; approve follows primary.

Inputs & textareas:
- `background: var(--bg-raised)`, `1px solid var(--border)`, `--radius`,
  `--fg` text, placeholder `--fg-3` (not dimmer).
- Focus: `border-color: var(--accent); box-shadow: var(--focus-ring)`; no outline.

Selects (project picker, thinking, model, form selects):
- `appearance: none` + the recipe above + custom chevron as inline
  `background-image` SVG (stroke `#909098`), `background-position: right 10px center`,
  padding-right 30px. Same height as buttons. Sans font, NOT mono.

Chips/badges: pill radius, `--bg-elevated` + `--border-subtle`, 12px text.
State chips tint with the matching `*-soft` bg + solid status text.

Focus: `:focus-visible { box-shadow: var(--focus-ring); outline: none; }` on all
interactive elements. Never remove focus affordance without replacing it.

Scrollbars: keep the existing thin overlay treatment, thumb
`rgba(255,255,255,0.12)`, hover `rgba(255,255,255,0.22)`. No accent scrollbars.

## 4. Layout

- Shell: `max-width: var(--shell-max)` desktop, full-bleed below 960px.
- Header: 56px raised bar (`--bg-raised`, bottom `--border-subtle`), OCEAN
  ramp wordmark left, controls right, 8px gaps, all controls `--ctl-h-sm`.
  Visible header controls are capped: context (project/session), ONE nav icon
  (sessions), text metadata (tokens/status), and ONE `⋯` overflow holding
  everything else (council, rooms, mute, capture).
- Transcript prose measure: `max-width: 72ch` for text blocks.
- Spacing scale: 4/8/12/16/24/32/48. Vary rhythm; don't pad everything 12px.
- Cards are earned: components that ARE cards (kanban card, stat, session item)
  get `--bg-elevated` + `--border-subtle` + `--radius-lg`. NEVER nest cards.

## 5. Bans (refuse-and-rewrite)

- No teal/cyan (#7fe7c8 family), no navy (#06111d family), no legacy yellow
  `--user`. All literals route through tokens; the ONLY raw values allowed in
  domain files are white/black alphas for one-off masks — and prefer tokens.
- No gradient text (`background-clip: text`).
- No `border-left`/`border-right` accent stripes > 1px. (Existing drawer
  left-rails: replace with tinted fills or dot indicators.)
- No 1px border + ≥16px blur shadow on the same element.
- No border-radius > 16px on cards/inputs (pills exempt).
- No glassmorphism as default; `backdrop-filter` only on overlay backdrops.
- No decorative motion; no page-load choreography.
- Uniform icon+heading+text card grids and hero-metric lockups: no.

## 6. Per-surface notes

- Landing (`transcript__landing`): the OCEAN banner hero — 8 `pre` rows, one
  ramp color per row, fluid mono sizing; lead in `--fg-2`, hint in `--fg-3`.
  Title is visually-hidden (screen readers only). No glow bath.
- Tool drawers: quiet single-line disclosures, mono label, status dot
  (ok/err/running) instead of colored rails.
- Thinking: italic 13px `--fg-3` with pill toggle; never louder than answers.
- Agent components (kanban/table/chart/…): data-forward, tight 12/13px type,
  mono numerals, consistent headers (11px uppercase label style), hairline
  `--border-subtle` separators; interactive rows hover `--bg-hover`.
- Diff/code: `--bg-well` body, mono 12px, add/del tints via `*-soft` + solid
  gutter text; filename heads 11px uppercase labels.
- Voice orb: neutral elevated circle idle; recording = solid accent fill +
  `--glow-brand` + pulse; transcribing = warn tint pulse. Kill pulses under
  reduced-motion.
- Composer: ONE dock card (the form is the frame; focus ring via
  `:focus-within`); chromeless textarea; turn-control selects are borderless
  minis on the dock's bottom row; voice mode switch reveals on orb
  hover/focus; no permanent captions under controls.
- Slide-overs (sessions/rooms): `--bg-raised` panel, `--shadow-lg`, backdrop
  `rgba(6,6,6,0.72)` + blur(4px); 280ms slide with reduced-motion fallback.
- Live states (call/livekit): live dot = `--err`-family red pulse? No — live
  recording convention is red, but our live accent is the brand: use
  `--accent` pulsing dot for "live", `--ok` for connected, `--warn` for
  reconnecting, `--err` for failed/barge.
- Extension side panel (`compact.css`): same system, tighter paddings; never
  fork colors.

## 7. Behavioral constraints (do not break)

- Every existing class name stays. This is a re-skin + markup-quality pass,
  not a rename. `sessions-overlay`/`rooms-overlay` close-on-click checks,
  `lk-tile-*` ids, `ocean-map__*`, `oceanRenderMap`/`oceanRenderSocialVideo`
  contracts, `#ocean-float-corridor` inline-style mutation, `is-*` modifiers —
  all load-bearing. Keep `is-*` selectors scoped to their family.
- Canvas card geometry (left/top/width/height inline styles from Rust) must
  keep `position: absolute` + `box-sizing: border-box` semantics.
- Height chains: `html, body { height: 100% }` +
  `.ocean-surface { height: 100%; height: 100dvh }` stay (extension side-panel
  dvh collapse, OCEAN-70).
- Google Places theming flows through the documented custom properties;
  update values to this palette, keep the property names.
