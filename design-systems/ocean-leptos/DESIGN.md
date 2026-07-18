---
version: alpha
name: Ocean Leptos/WASM Surface
description: A task-first, near-black product surface molded from the digital ocean, shared by web, PWA, Chrome extension, Tauri desktop, and future mobile shells.
colors:
  primary: "#00D7D7"
  on-primary: "#03181A"
  primary-bright: "#00FFD7"
  primary-deep: "#0087AF"
  primary-soft: "rgba(0, 215, 215, 0.12)"
  primary-ring: "rgba(0, 215, 215, 0.45)"
  ocean-1: "#00005F"
  ocean-2: "#0000AF"
  ocean-3: "#005FAF"
  ocean-4: "#0087AF"
  ocean-5: "#00AFD7"
  ocean-6: "#00D7D7"
  ocean-7: "#00FFD7"
  ocean-8: "#5FFFFF"
  background: "#060606"
  surface-raised: "#0A0A0A"
  surface-elevated: "#141414"
  surface-hover: "#1B1C21"
  surface-well: "#23252B"
  text-primary: "#FAFCFF"
  text-secondary: "#B8B9BB"
  text-metadata: "#909098"
  text-disabled: "#5A5C63"
  border-subtle: "rgba(255, 255, 255, 0.06)"
  border-default: "rgba(255, 255, 255, 0.10)"
  border-strong: "rgba(255, 255, 255, 0.20)"
  status-success: "#1ED760"
  status-success-soft: "rgba(30, 215, 96, 0.12)"
  status-warning: "#FFB224"
  status-warning-soft: "rgba(255, 178, 36, 0.12)"
  status-error: "#FF4D67"
  status-error-soft: "rgba(255, 77, 103, 0.12)"
  status-info: "#6AA6FF"
  status-info-soft: "rgba(106, 166, 255, 0.12)"
  badge-face: "#020203"
  badge-rim: "rgba(148, 156, 160, 0.22)"
  badge-band-1: "#4FD8C0"
  badge-band-2: "#2BB4A6"
  badge-band-3: "#1F95B8"
  badge-band-4: "#2170BE"
  badge-band-5: "#2453BC"
  badge-band-6: "#1A2F8E"
  badge-foam: "#EAFCFF"
  badge-split: "#05101F"
  badge-gloss: "#CFF4FF"
  water-high: "#2EA6C6"
  water-mid: "#1C63B0"
  water-low: "#0A1642"
typography:
  display:
    fontFamily: Poppins
    fontSize: 22px
    fontWeight: 600
    lineHeight: 1.25
  title:
    fontFamily: Poppins
    fontSize: 18px
    fontWeight: 600
    lineHeight: 1.3
  emphasis:
    fontFamily: Poppins
    fontSize: 15px
    fontWeight: 600
    lineHeight: 1.4
  body:
    fontFamily: Poppins
    fontSize: 14px
    fontWeight: 400
    lineHeight: 1.55
  secondary:
    fontFamily: Poppins
    fontSize: 13px
    fontWeight: 400
    lineHeight: 1.55
  control-label:
    fontFamily: Poppins
    fontSize: 13px
    fontWeight: 600
    lineHeight: 1
  small:
    fontFamily: Poppins
    fontSize: 12px
    fontWeight: 400
    lineHeight: 1.4
  label-caps:
    fontFamily: Poppins
    fontSize: 11px
    fontWeight: 600
    lineHeight: 1.2
    letterSpacing: 0.08em
  metadata:
    fontFamily: Poppins
    fontSize: 11px
    fontWeight: 500
    lineHeight: 1.35
  data:
    fontFamily: "ui-monospace, 'SF Mono', SFMono-Regular, Menlo, monospace"
    fontSize: 12px
    fontWeight: 400
    lineHeight: 1.55
  wordmark:
    fontFamily: Poppins
    fontSize: 14px
    fontWeight: 700
    lineHeight: 1
    letterSpacing: 0.22em
rounded:
  small: 4px
  control: 6px
  large: 10px
  geometry: 999px
spacing:
  micro: 4px
  compact: 8px
  small: 12px
  standard: 16px
  transcript-gap: 18px
  large: 24px
  xlarge: 32px
  display: 48px
components:
  button-primary:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.on-primary}"
    typography: "{typography.control-label}"
    rounded: "{rounded.control}"
    height: 34px
  button-secondary:
    backgroundColor: "{colors.surface-elevated}"
    textColor: "{colors.text-primary}"
    typography: "{typography.control-label}"
    rounded: "{rounded.control}"
    height: 34px
  button-danger:
    backgroundColor: "{colors.surface-elevated}"
    textColor: "{colors.status-error}"
    typography: "{typography.control-label}"
    rounded: "{rounded.control}"
    height: 34px
  input:
    backgroundColor: "{colors.surface-raised}"
    textColor: "{colors.text-primary}"
    typography: "{typography.body}"
    rounded: "{rounded.control}"
    height: 34px
  data-well:
    backgroundColor: "{colors.surface-well}"
    textColor: "{colors.text-primary}"
    typography: "{typography.data}"
    rounded: "{rounded.control}"
    padding: 12px
  chip:
    backgroundColor: "{colors.surface-elevated}"
    textColor: "{colors.text-secondary}"
    typography: "{typography.small}"
    rounded: "{rounded.small}"
    height: 28px
  card:
    backgroundColor: "{colors.surface-elevated}"
    textColor: "{colors.text-secondary}"
    typography: "{typography.body}"
    rounded: "{rounded.large}"
    padding: 16px
  composer:
    backgroundColor: "{colors.surface-elevated}"
    textColor: "{colors.text-primary}"
    typography: "{typography.body}"
    rounded: "{rounded.large}"
    padding: 8px
  menu:
    backgroundColor: "{colors.surface-elevated}"
    textColor: "{colors.text-secondary}"
    typography: "{typography.secondary}"
    rounded: "{rounded.control}"
    padding: 4px
  modal:
    backgroundColor: "{colors.surface-elevated}"
    textColor: "{colors.text-primary}"
    typography: "{typography.body}"
    rounded: "{rounded.large}"
    padding: 16px
  status-success:
    backgroundColor: "{colors.surface-elevated}"
    textColor: "{colors.status-success}"
    typography: "{typography.small}"
    rounded: "{rounded.small}"
    padding: 8px
  status-warning:
    backgroundColor: "{colors.surface-elevated}"
    textColor: "{colors.status-warning}"
    typography: "{typography.small}"
    rounded: "{rounded.small}"
    padding: 8px
  status-error:
    backgroundColor: "{colors.surface-elevated}"
    textColor: "{colors.status-error}"
    typography: "{typography.small}"
    rounded: "{rounded.small}"
    padding: 8px
  ocean-badge:
    backgroundColor: "{colors.badge-face}"
    textColor: "{colors.badge-foam}"
    rounded: "{rounded.geometry}"
    size: 48px
---

# Ocean Leptos/WASM Surface

## Overview

Ocean is a **native instrument panel molded from dark water**, not an AI landing page and not a control-heavy developer dashboard. The stage is almost black. Familiar product controls feel machined into it: raised controls catch one restrained overhead highlight; inputs and code wells recede into depth; active agent work appears as faint bioluminescence.

The product register is trusted desktop software in the lineage of Linear, Figma, and Raycast: compact, direct, readable, and unsurprising. Ocean retains its own identity through the deep-indigo-to-bright-aqua depth ramp and the circular wave coin. It does not copy those products' chrome.

The same Leptos/WASM product core renders browser, PWA, Chrome side panel, Tauri desktop, and future mobile shells. A shell may add platform capability, but it must not invent a second visual language.

**Control density is a design defect.** Show the control needed now. Put secondary actions behind one overflow, reveal power controls on intent, and remove unavailable capabilities instead of rendering dead chrome. One primary action should dominate any local decision.

## Colors

### The dark stage

- **Background `{colors.background}`** is the page and deepest transcript void.
- **Raised `{colors.surface-raised}`** is for bars, assistant surfaces, and drawers.
- **Elevated `{colors.surface-elevated}`** is for cards, controls, chips, and docks.
- **Hover `{colors.surface-hover}`** is a temporary interaction state, never a permanent panel color.
- **Well `{colors.surface-well}`** is reserved for code, diffs, terminal-like data, and inset transcript bodies.

Text steps down deliberately: `{colors.text-primary}` for headings and decisive content, `{colors.text-secondary}` for readable body copy, `{colors.text-metadata}` for labels and context, and `{colors.text-disabled}` only for disabled content. Never use disabled grey for ordinary readable copy.

### The OCEAN depth ramp

The identity ramp rises from abyss to surface:

`{colors.ocean-1}` → `{colors.ocean-2}` → `{colors.ocean-3}` → `{colors.ocean-4}` → `{colors.ocean-5}` → `{colors.ocean-6}` → `{colors.ocean-7}` → `{colors.ocean-8}`.

Use these as **solid colors on discrete identity elements**: individual wordmark letters, banner rows, chart segments, or vector layers inside the mark. Never gradient-clip text. A continuous OCEAN-ramp color gradient is permitted only inside data visualization or the circular mark, never on a button. Neutral low-alpha material sheens are allowed on raised surfaces because they describe overhead light rather than brand color.

`{colors.primary}` is the working interactive cyan. It appears only on:

1. the single primary action in a decision,
2. focus indication,
3. live/recording or an explicit live-attention state,
4. concise active selection cues.

Ordinary in-flight tool work is amber `{colors.status-warning}`. The Dynamic Island may use primary cyan for running because it is explicitly a live attention surface; this is a named exception, not the generic running color.

Accent-filled controls always use dark ink `{colors.on-primary}`, never white.

### Badge-confined material palette

`badge-face`, `badge-rim`, `badge-band-1` through `badge-band-6`, `badge-foam`, `badge-split`, and `badge-gloss` are scoped material-reference tokens from the live badge system. They describe the intended muted turquoise-to-indigo family but are **not sufficient to reconstruct the production logo**: the packaged PNG/SVG contain the authoritative static geometry and embedded paints, while the live Canvas tide coin uses its own rendered water material. These tokens are not a second UI palette and are not permission to spread teal through navigation or controls.

### Semantic state

- Connected, completed, and valid: `{colors.status-success}`.
- Running, reconnecting, pending permission, and caution: `{colors.status-warning}` unless an explicitly live action uses primary cyan.
- Failed, destructive, denied, and barge/error state: `{colors.status-error}`.
- Informational headings and neutral system context: `{colors.status-info}`.
- Live participation is primary cyan, not conventional recording red.

Use soft semantic backgrounds only as quiet tints. Do not turn status into colored slabs.

## Typography

Poppins is the product voice. It should feel compact and engineered, not promotional.

- **Display and title** are modest: 22px and 18px, semibold. There are no giant hero headlines in the product.
- **Body** is 14px/1.55. Assistant prose may open to 1.6 line-height for sustained reading.
- **Secondary and small** copy are 13px and 12px.
- **Labels** are 11px semibold uppercase with `0.08em` tracking. Use this register for table headers, filenames, and small section labels—not whole paragraphs.
- **Metadata** is quiet 11px text. Paths, IDs, token counts, numbers, code, and technical values use the data face.
- **Mono is for data, never for ordinary UI labels.** Use the platform stack `ui-monospace, "SF Mono", SFMono-Regular, Menlo, monospace` in implementation.
- **Wordmark** letters are separately filled Poppins Bold characters. The current uppercase lockup uses 14px with `0.22em` tracking; mark-only, lowercase, and tighter uppercase lockups remain review variants rather than three canonical logos.

Use weight and contrast before introducing another size. A view should rarely need more than three type levels at once.

## Layout

The desktop product shell is centered at a maximum width of **1120px**. It becomes full width in narrow and extension contexts. Preserve viewport safe areas and a true full-height chain.

### Core geometry

- Header: 56px minimum height, 16px horizontal padding, 8px control gaps.
- Standard control: 34px high. Compact control: 28px high.
- Transcript: roughly 72ch prose measure; 18px vertical rhythm; readable edge gutters.
- Composer: one two-row dock. The chromeless textarea owns row one. Voice, quiet turn controls, and one Send/Stop slot share row two.
- Composer textarea: 32px minimum writing row, growing upward to 240px.
- Workspace: transcript remains the center of record; desktop workbench modules reveal only when useful.

Use the 4/8/12/16/24/32/48 spacing scale. Vary rhythm by relationship; do not put 12px around everything.

### Responsive posture

At 720px and below, preserve one header row. Remove token metadata, collapse labels to state dots when necessary, shorten controls, and keep the transcript readable. At 480px and below, components genuinely reflow: kanban columns stack, tables become labeled row cards, charts wrap, dashboards and stats become one column, and confirm actions wrap.

The Chrome side panel always uses the compact posture. Mobile is not a later redesign; compact.css is the mobile grammar. No essential action may depend on hover.

## Elevation & Depth

Ocean material uses one overhead light.

- **Raised level 1:** controls, menu rows, chips, and compact session rows. A faint top-edge specular plus a tight dark drop.
- **Raised level 2:** cards, composer dock, list panels, and substantial components.
- **Raised level 3:** popovers, modals, tooltips, and the expanded Dynamic Island.
- **Carved well:** inputs, code, diffs, and data bodies use an inset shadow on `{colors.surface-well}`.
- **Pressed:** a raised control moves down 1px and replaces its lift with an inset shadow.

An element takes a border recipe **or** an elevation recipe. Never combine a one-pixel border with a large soft shadow. Hairlines may define a flat transcript boundary, but they must not become glowing card outlines.

Glow is state, not decoration. Thinking receives a faint cognition bloom; streaming may breathe a subtle aqua specular; live voice may use a stronger brand glow. Idle surfaces do not glow.

Motion is quick and tidal: 160ms feedback, 240ms standard transitions, and 420ms deliberate overlay movement. Overlays may rise 8px from depth. Nothing bounces, overshoots, or performs a decorative page-load entrance. Every animation has a static reduced-motion treatment.

## Shapes

The shape language is compact and machined:

- 4px for chips, badges, tabs, and menu rows.
- 6px for controls, inputs, selects, list rows, and menus.
- 10px for true cards, panels, modals, and the composer dock.
- 999px only for physical geometry: circles, status dots, the voice orb, progress tracks, and scrollbar thumbs.

Text-bearing pill controls are prohibited. Cards and inputs never exceed 16px radius. Do not nest cards. A component that already sits inside an assistant reply card should flatten its outer frame and preserve only meaningful inner wells.

The circular Ocean mark is an intentional identity object, not a precedent for circular navigation chrome.

## Components

### Primary, secondary, ghost, and danger controls

A primary button is compact, content-width, solid `{colors.primary}`, and set in dark `{colors.on-primary}` ink at semibold weight. It is 34px high with `0 14px` implementation padding. Hover brightens slightly; press is a restrained 1px depression. Never create a full-width cyan slab.

Secondary controls use neutral elevated material. Ghost controls are transparent and metadata-grey until hover/focus. Danger controls retain the secondary shape and use error text with an error-soft hover. Approve follows primary; deny and stop follow danger.

### Inputs and selects

Inputs are quiet carved or raised dark fields with primary text and metadata-grey placeholders. Focus uses one cyan ring. Selects use the same shape and a custom neutral chevron; they are sans-serif controls, not mono data chips.

The main composer is the exception: the textarea is chromeless because the composer dock is already the frame. The dock owns `focus-within`.

### Header and overflow

The header shows identity, current project/session context, one Sessions affordance, terse runtime state, and one overflow. Secondary actions—Council, Rooms, mute, capture, and comparable host features—belong in the overflow or disappear when unavailable.

The overflow reference is a plain elevated menu: tight 6px radius, 4px inset, 13px rows, no icon-title-subtitle lockups, and no accent chrome.

### Transcript

User input is a compact raised card aligned to the right. Assistant output is a single readable response surface aligned left. Do not create a bubble around every paragraph or component.

Thinking is quiet and italic. Tool activity is a mono disclosure row with a tiny status dot and a locally scrolling well when expanded. Running is amber; success is green; failure is red. Full error payloads belong in expanded transcript detail, while the global status remains one concise recoverable line.

### Rich agent components

Tables, kanban, charts, forms, progress, dashboards, timelines, stats, file trees, code, diffs, galleries, confirmations, maps, and video are data-first. Use 11px label headings, 12/13px content, mono numerals, hairline separators, and hover only on interactive rows. Code and diff bodies are wells; additions and deletions use semantic tints rather than neon rails.

Do not generate uniform icon-heading-description component grids. A component exists to carry data or action, not to advertise a capability.

### Permissions

A permission request is a compact elevated decision surface with a short label, tool name, plain-language reason, lossless args in a data well, and exactly two actions: deny and approve. The pending state may use an amber ring or tint. Preserve keyboard focus and the ability to recover from failure.

### Composer and voice

The composer is one dock with one action slot. Send and Stop are mutually exclusive. Voice is neutral when idle, cyan when live/capturing, amber when transcribing, and quiet/static when off. The settings trigger keeps a visible floor on touch. A full realtime voice state may promote the orb, but it must yield when the agent renders useful content.

### Dynamic Island

The Tauri Island is a living attention object, not a dashboard. Compact form shows the focused session and a tiny state cue. Expanded form replaces itself with exactly one mode at a time: Agent, Sessions, or Recall. It does not append heterogeneous feeds. Running uses cyan, needs-human uses amber, failure uses red, and healthy idle uses a quiet green cue.

### Panels and overlays

Sessions is a centered modal; Rooms is a right slide-over. Both use the same row vocabulary and conditional action disclosure. Overlay blur belongs only to the backdrop. Destructive row actions reveal on hover, focus, or a clear tap path.

## Do's and Don'ts

### Do

- Use near-black neutral surfaces and ration the OCEAN ramp to identity and state.
- Use `{colors.on-primary}` on cyan fills.
- Show one primary action and move secondary actions behind one overflow.
- Prefer conditional rendering and reveal-on-intent to permanent controls.
- Keep error summaries short, truthful, and recoverable.
- Use solid ramp colors on discrete letters, rows, layers, or data marks.
- Design desktop and compact behavior in the same component pass.
- Preserve visible focus, touch paths, safe areas, and reduced-motion alternatives.
- Use the existing 24×24 round-stroke icon family and real service marks when necessary.
- Validate components in the 380px side panel, at ≤480px, and on the 1120px desktop shell.

### Don't

- Don't use magenta or purple, legacy mint `#7FE7C8`, or navy `#06111D` chrome.
- Don't use gradient-clipped text, gradient buttons, or white text on cyan.
- Don't use terminal prompts, cursors, underscores, brackets, sparkles, stars, mascots, or generic AI magic as Ocean identity.
- Don't use pill-shaped text controls, nested cards, full-width accent slabs, or accent scrollbars.
- Don't use glassmorphism as the default material.
- Don't combine a one-pixel border with a broad blurred shadow.
- Don't build icon-title-subtitle launcher grids or permanent same-weight action rows.
- Don't hide an essential action behind hover on touch devices.
- Don't redraw the canonical logo from prose or accept generated typography as a production wordmark.

## Identity and Logo

The production identity input is `assets/master-1024.png`; `assets/ocean-mark.svg` is the portable vector. Treat their silhouette, circular rim, wave spacing, depth order, internal cutouts, and embedded static colors as immutable. The static mark and the live Canvas tide coin belong to one identity family, but only the checked-in static files are production logo sources. Do not rebuild the static logo from the front-matter badge or water tokens.

The badge is a matte near-black coin with shallow rim relief and stacked water depth. It is not a glossy SaaS orb, a photoreal ocean, or a cyberpunk neon emblem. The rim remains calm while loading motion moves water inside the clip.

Mark-only is production-safe. Lowercase `ocean` and uppercase `OCEAN` lockups remain visual review variants. Clear space, legal usage, monochrome/light variants, and minimum reproduction size are not yet canonical; do not invent them.

## Iconography

Product icons use a 24×24 viewBox, 2px optical stroke, round caps, round joins, `currentColor`, and a restrained outline vocabulary. They should remain legible at 16, 20, and 24px. Filled geometry is reserved for inherently filled concepts such as Stop or an official service mark.

Prioritize literal product concepts: Sessions, Recall, permission, needs reply, browser driving, file, editor, repo branch, diff, checks, artifact, call, voice state, Council, Canvas, and host capability. Do not create a separate illustrative icon family and do not use emoji in product UI.

Every custom icon delivery should include:

- optimized SVG with no embedded text or raster,
- 24×24 master geometry,
- 16/20/24/32px contact sheet,
- dark-stage and one-color previews,
- `currentColor` version,
- short semantic name and accessibility label,
- no baked shadow or glow.

## Motion and State

Use motion only to clarify state change:

- Hover/focus/press: 160ms.
- Standard component transition: 240ms.
- Overlay or tidal rise: 420ms maximum.
- New streamed content: opacity plus 2px rise.
- Loading mark: internal wave shear/drift; calm rim; never spin.
- Permission wait: amber ring pulse.
- Live: cyan dot pulse.
- Connected: green.
- Reconnecting/running: amber unless the action is explicitly live.
- Failure/barge: red.

Reduced motion removes looping movement, transforms, and animated entrances while preserving state through color, copy, and static geometry.

## Responsive and Accessibility

A missing host capability renders as absence, not an error. Every hover reveal has a keyboard and touch path. All icon-only controls require an accessible name. Status updates use a live region without flooding it with tool output. Decorative logo layers are hidden from assistive technology while the product name remains available as text.

Maintain WCAG AA for ordinary text. Metadata may be quieter but must remain readable. Never rely on hue alone: state combines color with a dot, label, icon, or action. Preserve IME composition, keyboard navigation, Escape dismissal, and sensible focus return.

## AI Asset Generation Brief

Use image generation for **exploratory supporting assets, icon concept sheets, component mood studies, and non-logo backgrounds**. Do not use it to redraw the production logo or typeset a final wordmark.

Every prompt must state:

1. the asset role and target shell,
2. exact canvas and alpha requirement,
3. the attached canonical mark as immutable when present,
4. exact token names and hex values,
5. the one-overhead-light material model,
6. hard negative constraints,
7. source and export deliverables.

For production logo-bearing outputs, composite `assets/master-1024.png` or `assets/ocean-mark.svg` deterministically after generation. Do not trust generated edges, text, or wave geometry.

### Copy-ready custom icon prompt

> Design a coherent set of **[N] Ocean product icons** for **[features]** on the **[web / PWA / extension / Tauri / mobile]** shell. Use a 24×24 master grid on a **[pixel dimensions]** transparent RGBA canvas, 2px monoline strokes, round caps and joins, flat orthographic geometry, and one-color `currentColor` output. The set must read clearly at 16px and match serious native productivity software. Use no text, letters, emoji, purple, magenta, mint chrome, gradient-clipped text, sparkles, stars, mascots, terminal prompts, cursors, brackets, baked shadows, or decorative waves. Ocean identity comes from the product, not from putting a wave in every icon. Deliver individual SVG reconstruction paths, transparent PNG previews, and a 16/20/24/32px contact sheet on `{colors.background}` `#060606`.

### Copy-ready component exploration prompt

> Create a high-fidelity **[pixel dimensions]** opaque component study for the **Ocean Leptos/WASM [web / extension / Tauri / mobile] surface**: **[component]** in idle, hover, focus, active, disabled, running, success, warning, and error states. Use `{colors.background}` `#060606`, `{colors.surface-raised}` `#0A0A0A`, `{colors.surface-elevated}` `#141414`, `{colors.text-primary}` `#FAFCFF`, `{colors.text-secondary}` `#B8B9BB`, scarce `{colors.primary}` `#00D7D7` with dark `{colors.on-primary}` `#03181A`, success `{colors.status-success}` `#1ED760`, warning `{colors.status-warning}` `#FFB224`, and error `{colors.status-error}` `#FF4D67`. Poppins, compact 34px controls, 6px control radius, one overhead specular, carved wells, WCAG AA text contrast, and a non-color cue for every state. No purple, magenta, mint chrome, gradient-clipped text, terminal symbols, glassmorphism, pill text controls, nested cards, gradient buttons, giant blur shadows, sparkles, mascots, or generic AI ornament. Show desktop and 380px compact layouts with equivalent hierarchy. Deliver an opaque PNG study plus a reconstruction spec listing tokens, dimensions, spacing, and state behavior.

### Copy-ready supporting asset prompt

> Create a **[asset role]** for Ocean **[surface]** at **[dimensions]**. Use the attached `master-1024.png` as an immutable brand mark: preserve its exact circular silhouette, rim, stacked wave spacing, internal depth, and embedded colors; do not redraw or reinterpret it. Stage on **[transparent RGBA / opaque #060606]** with **[safe zone]**. Material is matte near-black with shallow overhead relief and restrained shadow. Use only **[token names + hex values]**. No generated text, watermark, purple, magenta, mint chrome, gradient-clipped text, white-on-cyan controls, terminal/code/cursor/bracket symbols, sparkle/star motifs, mascots, glassmorphism, cyberpunk neon, lens flare, extra rings, or extra wave bands. Deliver a clean PNG/SVG-ready concept, alpha preview, contact sheet at target sizes, and a reconstruction/layer map; request a native layered source only when the selected design tool supports one.

## Asset Delivery and QA

The package should preserve a transparent 1024×1024 source, optimized SVG where applicable, and explicit opaque platform exports. Review every icon-like output at 16, 20, 24, 32, 48, 192, and 512px on transparent and `{colors.background}` stages. Check alpha halos, one-color legibility, compact-width behavior, touch affordances, and reduced-motion fallback.

Record model, version, prompt, generation ID or seed, and post-processing steps. Generated output remains a proposal until reviewed against this DESIGN.md and the canonical assets.
