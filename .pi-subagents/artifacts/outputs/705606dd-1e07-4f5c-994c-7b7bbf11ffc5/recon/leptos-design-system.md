---
name: Ocean Surface
description: Evidence-backed design system for the canonical Leptos/WASM product surface shared by web, extension, and Tauri.
source_of_truth: styles/tokens.css
status: implemented-with-documented-aspirations
platforms: [web, pwa, chrome-extension, tauri]
color_mode: dark
font_family: Poppins
mono_family: ui-monospace
shell_max_width: 1120px
breakpoints:
  compact: 720px
  narrow_components: 480px
radii:
  sm: 4px
  control: 6px
  large: 10px
  pill_geometry_only: 999px
control_heights:
  standard: 34px
  compact: 28px
motion:
  fast: 160ms
  standard: 240ms
  slow: 420ms
  ease: cubic-bezier(.22,.61,.36,1)
  tide: cubic-bezier(.16,.84,.28,1)
---

# Overview

Ocean Surface is a near-black, product-register interface whose identity is the OCEAN deep-indigo-to-aqua depth ramp and circular relief wave mark. Brand energy is rationed to identity, primary actions, focus, and live state. The UI favors conditional rendering, one header overflow, quiet metadata, and reveal-on-intent controls over permanent chrome.

**Authority and evidence**

- Normative contract: `docs/OCEAN_WEB_SURFACE_DESIGN.md:1-349` (inspected completely).
- Implemented foundation: `styles/tokens.css:1-184`, `styles/base.css:1-108`.
- Cascade, in shipping order: `styles/tokens.css`, `base.css`, `chrome.css`, `island.css`, `transcript.css`, `components.css`, `composer.css`, `panels.css`, `deck.css`, `workspace.css`, `council.css`, `call.css`, `canvas.css`, `compact.css`, `float.css`. See `index.html:36-52` and `extension/sidepanel.html:8-22`.
- Extension assembly copies all emitted CSS and fonts/assets: `scripts/build-extension.sh:24-32`. The wildcard copy does not itself link future CSS, so both HTML enumerations must still be updated together.
- Current markup/state hooks: `crates/ocean-surface-ui/src/app.rs:1200-1700`, `src/transcript.rs:130-290`, `src/icons.rs:1-726`; component families live in `components.rs`, `sessions.rs`, `rooms.rs`, `call.rs`, and `canvas.rs`.
- Assets: `public/brand/master-1024.png`, `public/brand/ocean-mark.svg:1-47`, `public/icon-{192,512}.png`, `public/apple-touch-icon.png`, `public/manifest.webmanifest:1-15`, and Poppins WOFF2 files under `public/fonts/`.

**Implemented truth vs aspiration**

- Shipping landing is a full-pane WebGL `SoundingsLanding`, not the document’s eight `<pre>` banner rows (`transcript.rs:142-163`; `transcript.css:872-951`). Pending response is `SoundingsThinking`; an active streamed turn shows a Canvas tide coin plus “ocean is working…” (`transcript.rs:165-282`).
- The design document requests an inline SVG mark with named layers. Shipping `WaveBadge` is Canvas 2D, DPR-capped at 2, animated with rAF and static under reduced motion (`icons.rs:1-292`). The external SVG is a static favicon/asset, not the interactive component.
- Sessions now ship as a centered modal; rooms remains a right slide-over (`panels.css:1-87`).
- The implemented compact/full-width breakpoint is 720px, not the document’s stated 960px (`compact.css:143-298`).
- The implemented badge/water palette adds scoped muted teal/blue tokens beyond the document’s main neon ramp (`tokens.css:71-108`).

# Colors

All new UI colors belong in `styles/tokens.css`; domain styles should reference tokens.

## Surfaces and text

| Token | Exact value | Use |
|---|---:|---|
| `--bg` | `#060606` | page |
| `--bg-raised` | `#0A0A0A` | bars, panels, drawers |
| `--bg-elevated` | `#141414` | cards, controls, chips |
| `--bg-hover` | `#1B1C21` | elevated hover |
| `--bg-well` | `#23252B` | code, diff, transcript/data wells |
| `--fg` | `#FAFCFF` | headings and primary content |
| `--fg-2` | `#B8B9BB` | secondary/body text |
| `--fg-3` | `#909098` | labels and metadata |
| `--fg-4` | `#5A5C63` | disabled only |

Evidence: `styles/tokens.css:34-46`.

## Identity and interactive accent

The eight OCEAN solids are `#00005F`, `#0000AF`, `#005FAF`, `#0087AF`, `#00AFD7`, `#00D7D7`, `#00FFD7`, `#5FFFFF`. `--accent` is ocean-6; `--accent-bright` ocean-7; `--accent-deep` ocean-4. Accent fill takes dark `--fg-on-accent: #03181A`, never white. `--accent-soft` is `rgba(0,215,215,.12)` and the focus ring color is `rgba(0,215,215,.45)`. The vertical `--gradient` is data-fill only, never button or text (`tokens.css:48-69`).

The implemented mark has an intentionally scoped muted badge ramp: `#4fd8c0`, `#2bb4a6`, `#1f95b8`, `#2170be`, `#2453bc`, `#1a2f8e`; face `#020203`, foam `#eafcff`, split `#05101f`, gloss `#cff4ff`. Water uses `#2ea6c6/#1c63b0/#0a1642` (`tokens.css:71-108`). These are identity/scene-only and must not leak into controls.

## Semantic state

- Success/connected: `--ok #1ED760`, soft `rgba(30,215,96,.12)`.
- Error/failed/barge: `--err #FF4D67`, soft `rgba(255,77,103,.12)`.
- Warning/reconnecting/running: `--warn #FFB224`, soft `rgba(255,178,36,.12)`.
- Information: `--info #6AA6FF`, soft `rgba(106,166,255,.12)`.
- Live/recording is brand accent, not conventional red.

Evidence: `tokens.css:110-116`, `transcript.css:628-673`.

## Borders and current debt

White-alpha borders are 6%, 10%, and 20% (`tokens.css:118-122`). Inspection found 39 raw color-function/hex occurrences in domain CSS outside `tokens.css`. Notable debt: the select data-SVG in `chrome.css:284`, transcript animation alpha literals at `transcript.css:750-767`, overlay rgba at `panels.css:11`, slash-menu legacy fallbacks at `composer.css:616-688`, map/service-worker inline values in `index.html`, and Canvas literals in `icons.rs:20-170`. Asset-embedded SVG colors are expected but should remain asset-local.

# Typography

- UI family: vendored Poppins 400, 500, 600, 700 (`tokens.css:12-31,124-128`). Stack: `'Poppins', system-ui, -apple-system, 'Segoe UI', sans-serif`.
- Mono: `ui-monospace, 'SF Mono', SFMono-Regular, Menlo, monospace`; use only for data such as paths, IDs, token counts, numbers, and code.
- Body: 14px, line-height 1.55 (`base.css:33-44`). Normative product scale is 11 label, 12 small, 13 secondary, 14 body, 15 emphasis, 18 title, 22 display.
- Uppercase data headings: 11px, weight 600, letter-spacing `.08em`, `--fg-3` (`components.css:45-67`).
- Transcript prose: 14px/1.6; h1/h2/h3 22/18/15; inline and block code 12px mono (`transcript.css:416-515`).
- Header wordmark uses five individually solid-colored Poppins letters at 14px/700/.22em, never gradient-clipped (`chrome.css:39-102`).
- Avoid mono UI labels and avoid unreadably dim `--fg-4` for content. Actual domain CSS includes a few 9/10/24px values; treat these as specialized existing implementation, not additions to the core scale.

# Layout

- Desktop shell: centered `max-width: 1120px`; full viewport width at 720px and in extension (`base.css:56-68`; `compact.css:1-5,143-150`). Preserve safe-area inset padding and the load-bearing height chain: `html, body {height:100%}` and root `height:100%;height:100dvh`.
- Header: 56px minimum, 16px horizontal padding, 16px major gap, 8px right-control gap (`chrome.css:1-18`). Visible controls are context, Sessions, quiet runtime metadata, and one `⋯` overflow; phone/council/rooms/capture are conditional menu actions (`app.rs:1200-1320`).
- Spacing scale: 4/8/12/16/24/32/48 (`docs/OCEAN_WEB_SURFACE_DESIGN.md:248-252`). Shipping transcript uses 18px gap and 18×22 padding; composer uses 8×12 padding with 8px columns/4px rows.
- Transcript text measure is about 72ch. User card max is `min(72ch,86%)`; assistant is `min(74ch,100%)` (`transcript.css:330-386`).
- Composer is a two-row grid: chromeless input across row one; orb, flexible turn controls, and one Send/Stop slot across row two. Textarea minimum 32px and maximum 240px; action/orb are 34px (`composer.css:9-27,463-576`).
- Compact at ≤720px: single-row header; tokens and browser label hide; status caps at 96px; controls cap at 46vw; transcript padding becomes 12×14; orb becomes 32px; live chip collapses to its dot (`compact.css:143-298`). Extension applies the same rules unconditionally (`compact.css:1-141`).
- Narrow at ≤480px and always in extension: kanban stacks; tables become `data-label` row cards; charts wrap; stats/dashboard become one column; galleries use 100px minimum cells; confirms wrap (`compact.css:300-599`). At ≤720px pinned cards become a 78vw swipe strip with unpin always visible (`601-617`).
- Canvas geometry is load-bearing: keep absolute positioning and border-box semantics. Rich content must reflow or scroll locally rather than widen the viewport.

# Elevation & Depth

- `--shadow-sm`: `0 2px 8px rgba(0,0,0,.40)`.
- `--shadow-md`: `0 8px 24px rgba(0,0,0,.55)`.
- `--shadow-lg`: `0 24px 64px rgba(0,0,0,.65)`.
- `--glow-brand`: `0 0 32px rgba(0,215,215,.28)`; reserve for voice/live moments.
- Raised material uses top-edge specular and `--elev-1` controls/rows, `--elev-2` cards/docks, `--elev-3` popovers/modals. Carved data/input surfaces use `--well` or `--well-deep`. Active raised controls use `--pressed` and a 1px downward movement (`tokens.css:137-165`).
- Implemented `--sheen` and `--sheen-soft` provide corner/top-to-bottom shading; pointer light is opt-in via `.ocean-lit`, hover-capable only, disabled under reduced motion (`tokens.css:148-165`; `base.css:75-108`).
- Thinking/streaming/live use restrained bioluminescence. Streaming assistant cards breathe their aqua specular over 6s and settle over 1.2s; newly appended turns rise 2px in 160ms, but restored history does not animate (`transcript.css:750-789`).
- Motion tokens are 160/240/420ms with standard and tidal curves. Every animation requires a reduced-motion kill. Overlays/popovers may use an 8px tidal rise; avoid page-load choreography.
- Normative rule: use border or elevation, not both, especially never a 1px border plus a ≥16px blur shadow. Existing assistant/permission cards mix hairlines with tight elevation; treat this as implementation debt, not precedent.

# Shapes

- `--radius-sm: 4px`: chips, badges, menu rows, tabs, mono data chips.
- `--radius: 6px`: buttons, inputs, selects, list rows, menus.
- `--radius-lg: 10px`: true cards, panels, modal, composer dock.
- `--radius-pill: 999px`: geometry only—dots, circles, orb, progress, scrollbar thumbs. Text-bearing pills are banned.
- Standard controls are 34px high; compact controls 28px (`tokens.css:130-184`).
- Cards/inputs must not exceed 16px radius. Do not nest cards. A primary action is content-width, not a full-width accent slab.
- Current exceptions/debt: `.voice-live-chip` is text-bearing with pill radius (`composer.css:410-456`); the circular Send/Stop and voice orb are valid geometric exceptions. `composer.css:616-688` has literal 8px radius and an undefined `--radius-md` fallback.

# Components

## Buttons, fields, and focus

Primary controls use accent fill, dark accent ink, weight 600, 6px radius, brightness 1.12 hover, compact press, and `.4-.45` disabled opacity with no pointer events. Permission Approve is canonical (`transcript.css:96-159`). Secondary controls use elevated neutral surfaces; danger uses error text and error-soft hover; ghost controls reveal elevated fill on hover.

Inputs use raised/carved dark surfaces, `--fg`, placeholder `--fg-3`, and accent border plus the 2px focus ring. Selects remove native appearance and use a custom grey chevron (`chrome.css:274-343`). The composer intentionally has no textarea frame; its single dock owns `:focus-within` (`composer.css:9-27,463-503`). Global `:focus-visible` is the safety floor (`base.css:48-54`).

## Header and overflow

The reference menu is `.ocean-more__menu`: 176px minimum, 4px padding, 6px radius, elev-3, plain 13px rows, no icon/subtitle/accent chrome (`chrome.css:405-466`). Keep one overflow rather than equal-weight action rows.

## Transcript and status

Tool drawers are mono disclosure rows with 7px semantic state dots; thinking is quiet/italic; assistant prose is contained in one reply card, and rich component outer wrappers flatten inside it to prevent nested cards (`transcript.css:526-749,791-821`). Status chips must contain one concise normalized line; full payloads belong in expanded error tool blocks/logs. Failed turns must preserve input and support resend.

## Rich agent components

True component cards use sheen/elev-2/radius-10/16px padding with 11px uppercase headings (`components.css:4-67`). Tables, kanban, charts, forms, progress, markdown, dashboards, timeline, stats, file tree, diff/code, gallery, confirm, map, and video retain tight data-forward inner structures. Interactive rows hover with `--bg-hover`; diff/code are 12px mono wells with semantic add/delete tints.

## Composer and voice

Composer has one dock and one mutually exclusive Send/Stop action slot. Voice idle is neutral; recording/capturing uses accent fill/dark ink/glow; transcribing uses warn; live uses accent-soft; off is neutral/static. Trigger opacity has a `.35` touch/findability floor. Realtime voice hides text/action/turn controls and promotes the orb to 108px (`composer.css:42-180,367-456,590-736`).

## Panels and accessibility

Sessions is a centered modal up to 760px and 80vh/720px; rooms is a 380px/92vw slide-over. Overlay is `rgba(6,6,6,.72)` with 4px blur; closed sessions use `[hidden]` and leave layout/accessibility tree (`panels.css:1-87`). Markup uses semantic buttons/forms/selects, labels/titles on icon controls, menuitem roles, dialog/complementary labels, a visually hidden landing h1, live-region status, and `aria-hidden` ornament. Composer keyboard behavior covers IME, slash-menu arrows/Enter/Tab/Escape, Enter submit, and Shift+Enter newline (`app.rs` composer block).

# Do's and Don'ts

## Do

- Route new UI colors through `styles/tokens.css` and keep extension/web cascade enumeration synchronized.
- Use solid OCEAN ramp colors on discrete identity elements; use accent only for primary action, focus, and live/active state.
- Use dark `--fg-on-accent` on accent fills.
- Prefer conditional rendering, one overflow, quiet metadata, and reveal-on-intent controls.
- Preserve every existing class, scoped `is-*` state, overlay click contract, `lk-tile-*` IDs, map/social JS globals, float corridor mutation, canvas geometry, and full-height chain (`docs/OCEAN_WEB_SURFACE_DESIGN.md:327-349`).
- Provide visible keyboard focus and reduced-motion alternatives. Make touch actions visible without hover.
- Keep failure summaries concise and recoverable; carry detail in transcript tool blocks.
- Verify both ≤720px and ≤480px behavior plus extension’s always-compact root.

## Don't

- No gradient-clipped text, white text on accent, magenta/purple, legacy mint `#7fe7c8`, navy chrome, or legacy yellow user color.
- No pill-shaped text controls, nested cards, full-width accent slabs, glassmorphism defaults, accent scrollbars, or accent side rails.
- No icon+heading+subtitle launcher grids, hero metrics, permanent same-weight action rows, or decorative sparkle/prompt/cursor identity.
- No raw JSON/multiline errors in status and no silent dangling failed turns.
- No hover-only affordance on compact/touch.
- Do not copy current debt: raw domain colors, slash-menu fallback tokens, text-bearing live pill, landing-control backdrop blur, or border-plus-elevation combinations.

# AI Asset Generation Guidance

This custom section follows the canonical sections and is suitable for a Google Labs DESIGN.md consumer that supports unknown sections.

- Generate only the **circular Ocean wave mark**, never UI mockups or decorative hero art. Reference `public/brand/master-1024.png` for silhouette and `public/brand/ocean-mark.svg:1-47` for geometry.
- Required visual language: circular badge, stacked wave cutouts/strata, matte near-black coin face, shallow inner/outer relief, muted aqua-to-indigo depth, calm rim, no glossy SaaS orb.
- Forbidden prompt concepts: terminal prompt, `>`, `_`, cursor, code brackets, sparkle/star, mascot, magenta/purple, text gradients, legacy mint, or a separate “AI” glyph.
- Request transparent 1024×1024 master plus mask-safe crops. Keep critical geometry within the central safe area for 192/512 and 180px app icons. Validate at 16, 24, 32, 48, 192, and 512px.
- For an animatable deliverable, request clean vector paths and named layers: `rim`, `wave-1`, `wave-2`, `wave-3`, `deep-water`, `wordmark`. Motion should shear/drift wave bands under a calm circular clip; never spin or bounce. Provide a static reduced-motion frame.
- Test mark-only, lowercase `ocean`, and tighter uppercase `OCEAN` lockups. Text must use separately filled solid letters, never gradient clipping.
- Generated output is a proposal, not token authority. Sampled colors must be reconciled into `styles/tokens.css`; do not paste generated literals into domain CSS.

# Acceptance Evidence

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Produced a read-only, standalone DESIGN.md-format evidence handoff at the authoritative artifact path without changing project/source files."
    },
    {
      "id": "criterion-2",
      "status": "satisfied",
      "evidence": "Document has YAML token front matter, canonical section order, exact tokens and file ranges, implemented-vs-aspirational distinctions, responsive/accessibility rules, asset guidance, and review risks."
    }
  ],
  "changedFiles": [
    ".pi-subagents/artifacts/outputs/705606dd-1e07-4f5c-994c-7b7bbf11ffc5/recon/leptos-design-system.md"
  ],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    {
      "command": "wc -l docs/OCEAN_WEB_SURFACE_DESIGN.md styles/*.css selected Rust/HTML files; git status --short; git diff --cached --name-only",
      "result": "passed",
      "summary": "Confirmed scope, pre-existing dirty tree, and no staged files."
    },
    {
      "command": "file public/brand/master-1024.png public/brand/ocean-mark.svg public/icon-192.png public/icon-512.png public/apple-touch-icon.png public/fonts/*.woff2",
      "result": "passed",
      "summary": "Verified brand/icon dimensions, formats, alpha, and vendored font files."
    },
    {
      "command": "grep domain CSS for interaction/media/type/radius/shadow rules and raw color literals outside tokens.css",
      "result": "passed",
      "summary": "Mapped states/responsive behavior and found 39 non-token CSS color occurrences for review."
    }
  ],
  "validationOutput": [
    "All 15 stylesheet links match and have identical order in index.html and extension/sidepanel.html.",
    "Design document inspected completely (349 lines); complete cascade mapped in shipping order.",
    "Output follows required canonical order: Overview, Colors, Typography, Layout, Elevation & Depth, Shapes, Components, Do's and Don'ts; custom AI guidance follows canonical sections.",
    "git diff --cached --name-only returned empty."
  ],
  "residualRisks": [
    "Working tree already contained unrelated modified/untracked files; this task did not alter them.",
    "No browser rendering, numeric contrast audit, or WASM build was run because this was read-only reconnaissance.",
    "Specialized call/canvas/deck selectors should be spot-checked by the implementing reviewer when those domains change."
  ],
  "noStagedFiles": true,
  "diffSummary": "Added/updated one evidence artifact only; project/source tree unchanged by this task.",
  "reviewFindings": [
    "no blocker to design handoff",
    "warning: shipping landing/pending mark, sessions geometry, and compact breakpoint differ from prose design document",
    "warning: token-only color and several anti-pattern rules have current exceptions/debt"
  ],
  "manualNotes": "Review gate remains required. Acceptance Evidence is appended after the canonical design sections to satisfy the run acceptance contract."
}
```
