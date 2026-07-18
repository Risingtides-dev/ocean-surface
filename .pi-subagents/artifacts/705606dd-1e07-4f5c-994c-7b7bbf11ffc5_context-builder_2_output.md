# Ocean brand and asset-generation context brief

**Purpose.** Read-only recon for planning two future design-system documents: (1) a Claude Design–oriented product/UI design brief and (2) a ChatGPT image-generation asset/prompt brief. This brief does not create those documents or alter project/source files.

**Evidence posture.** Findings describe the current checkout on 2026-07-15. The checkout was already dirty before this recon. In particular, `docs/OCEAN_DESKTOP_NORTH_STAR.md` has a large unstaged rewrite and `docs/OCEAN_WEB_SURFACE_DESIGN.md` has an unstaged composer-only edit. Identity sections inspected here are stable relative to HEAD, but a reviewer should treat the desktop document’s current wording as workspace state until it is committed.

## 1. Brand authority and hierarchy

1. **Binding product visual authority:** `docs/OCEAN_WEB_SURFACE_DESIGN.md:1-14`. It applies to the Leptos app, styles, and HTML and defines the register as Ocean’s own identity, not an agency rebrand. Brand energy is rationed rather than decorative; familiar product affordances and restrained color take precedence.
2. **Repository-level non-negotiables:** `AGENTS.md:67-76`. The identity is the TUI-derived depth ramp plus the accepted circular neumorphic wave mark. It explicitly rejects Rising Tides magenta/purple, legacy teal-mint, gradient-clipped text, and prompt/cursor/code-glyph logo directions.
3. **Cross-shell authority:** `docs/OCEAN_PLATFORM_CONTRACT.md:13-42,85-89`. One Leptos/WASM product core renders web/PWA, extension, Tauri, and future mobile. `docs/OCEAN_PLATFORM_CONTRACT.md:104-108` binds every shell to the same design system, token colors, and `icons.rs` stroke family; no emoji glyphs in UI.
4. **Identity origin:** `../ocean-os/crates/ocean-tui/src/splash.rs:13-36`. The eight-row ASCII OCEAN banner uses xterm colors `17,19,25,31,38,44,50,87`. `docs/OCEAN_WEB_SURFACE_DESIGN.md:18-23` translates this as deep indigo rising to bright aqua while making clear that the ASCII banner is heritage, not the active logo.
5. **Accepted mark:** `public/brand/master-1024.png` is the canonical visual reference; `public/brand/ocean-mark.svg` is its portable vector counterpart (`docs/OCEAN_WEB_SURFACE_DESIGN.md:20-23`). Git history shows the tracked brand assets landed in commit `2181dcf` (`chore(brand,ledger): track referenced brand assets...`).

## 2. Non-negotiable visual DNA

### Canonical identity

- Use the **circular neumorphic wave mark**: circular/coin badge, stacked wave cutouts, turquoise-to-blue depth movement, matte near-black face, rim relief, shallow inner/outer shadow (`docs/OCEAN_WEB_SURFACE_DESIGN.md:25-29`; inspect `public/brand/master-1024.png`).
- The checked-in SVG has a `48×48` viewBox and a roughly 42-unit badge diameter (`public/brand/ocean-mark.svg:2-53`). Its geometry includes the face/rim, upper arc, two wave strokes, deep-water fill, bottom depth bar, vignette, and edge occlusion. Preserve silhouette and wave spacing rather than asking a model to reinterpret them.
- The page stage is near-black. Working UI cyan is scarce: primary actions, live/active states, and focus rings only (`docs/OCEAN_WEB_SURFACE_DESIGN.md:43-53`). Everything else is neutral.
- The product tone is serious, native, task-first, and high-trust—not a marketing landing page, agency concept, or “AI magic” aesthetic (`docs/OCEAN_WEB_SURFACE_DESIGN.md:4-14`; `docs/OCEAN_DESKTOP_NORTH_STAR.md:19-41`).

### Color direction

Authoritative UI ramp (`docs/OCEAN_WEB_SURFACE_DESIGN.md:71-82`; `styles/tokens.css:57-75`):

| Token | Hex | Role |
|---|---:|---|
| `--ocean-1` | `#00005F` | abyss indigo |
| `--ocean-2` | `#0000AF` | deep indigo-blue |
| `--ocean-3` | `#005FAF` | deep ocean blue |
| `--ocean-4` | `#0087AF` | depth accent / data-gradient end |
| `--ocean-5` | `#00AFD7` | cyan-blue |
| `--ocean-6` | `#00D7D7` | working interactive accent |
| `--ocean-7` | `#00FFD7` | live highlight |
| `--ocean-8` | `#5FFFFF` | brightest surface aqua |

Core stage and text: `#060606` background, `#0A0A0A` raised, `#141414` elevated, `#23252B` data well; `#FAFCFF` primary text, `#B8B9BB` secondary, `#909098` metadata (`styles/tokens.css:42-55`). Accent-filled controls use dark ink `#03181A`, never white (`styles/tokens.css:69-75`).

The badge owns a **separate, muted material palette**, not permission to spread teal through the UI: matte face `#020203`, aqua-to-indigo bands `#4fd8c0 #2bb4a6 #1f95b8 #2170be #2453bc #1a2f8e`, foam `#eafcff`, deep split `#05101f` (`styles/tokens.css:77-103`). These badge-only tokens reconcile the logo’s turquoise/blue material with the ban on legacy teal-mint UI chrome.

### Material and motion

- One overhead light source. Raised objects catch a top-edge specular and cast a dark drop; carved wells recede. Elevation replaces borders—do not combine a border with a broad blur shadow (`docs/OCEAN_WEB_SURFACE_DESIGN.md:193-216,280-289`).
- “Agent cognition is bioluminescence,” but glow is state-driven and restrained, not decorative (`docs/OCEAN_WEB_SURFACE_DESIGN.md:193-207,218-226`).
- Motion communicates state. No bounce, elastic movement, page-load choreography, or ornamental entrance. Every animation needs reduced-motion fallback (`docs/OCEAN_WEB_SURFACE_DESIGN.md:139-145`).
- Loading may animate current *inside* the circular mark while the rim remains calm; it must not become a spinner or bouncing-dot loader. Reduced motion shows the static mark plus a small live glow (`docs/OCEAN_WEB_SURFACE_DESIGN.md:232-246`).

## 3. Rejected directions and prompt negative constraints

Treat these as hard negatives in both future documents:

- **No Rising Tides brand transfer:** no magenta/purple; rejected 2026-07-04 (`AGENTS.md:70-75`; `docs/OCEAN_WEB_SURFACE_DESIGN.md:52-53`).
- No legacy teal-mint `#7fe7c8` family and no navy-chrome `#06111d` family (`docs/OCEAN_WEB_SURFACE_DESIGN.md:52-53,277-280`). Muted turquoise is confined to the accepted badge geometry.
- No terminal/prompt identity: `>`, underscore cursor, brackets, terminal glyphs, code-logo symbolism, Codex-adjacent shapes (`docs/OCEAN_WEB_SURFACE_DESIGN.md:31-35`). A terminal icon can represent an actual terminal feature, but must never become Ocean’s brand mark.
- No sparkle/star logo motifs, mascots, fake AI ornament, or generic “magic AI” visual language (`docs/OCEAN_WEB_SURFACE_DESIGN.md:31-35`; extension restraint in `AGENTS.md:157-168`).
- Never gradient-clip text. Ramp colors appear as solid fills on discrete elements/letters or vector fills inside the circular mark (`docs/OCEAN_WEB_SURFACE_DESIGN.md:41,79-82`).
- No gradient buttons; primary actions are a solid `--ocean-6` fill with dark ink (`docs/OCEAN_WEB_SURFACE_DESIGN.md:45-49,152-157`). Data gradients are allowed only for chart/data fills.
- No glassmorphism by default, excessive glow baths, giant soft-shadow borders, decorative waves sprayed across chrome, pill-shaped text controls, nested card grids, icon-heading-description marketing tiles, or full-width accent slabs (`docs/OCEAN_WEB_SURFACE_DESIGN.md:277-299`).
- No control-heavy dashboards. Conditional rendering, one primary affordance, one header overflow, quiet metadata, and reveal-on-intent are part of the brand behavior—not merely layout preferences (`docs/OCEAN_WEB_SURFACE_DESIGN.md:7-14,263-270`).

## 4. Canonical logo usage and lockup status

### Accepted now

- **Raster master:** `public/brand/master-1024.png`, 1024×1024 RGBA with transparency. SHA-256 observed: `f9587a7f02e531fc9653f968fb6617219e5ac02ae5929c46a3b487fbdf894bf9`.
- **Portable vector:** `public/brand/ocean-mark.svg`, width/height 1024, `viewBox="0 0 48 48"`, literal/pre-resolved paints. SHA-256 observed: `8a922b9fef04ad089de7cb4e5d1062075bf9ea8c5fb170073a0094f257c066e5`.
- **Web/PWA icon references:** SVG favicon first, then 192 and 512 PNG fallbacks, plus 180 Apple touch icon (`index.html:14-20`).
- **Manifest:** 192 and 512 PNGs are both `any maskable`, with `#060606` background/theme (`public/manifest.webmanifest:1-16`).
- **Tauri:** generated platform icon family exists under `crates/ocean-tauri/icons/`; `crates/ocean-tauri/tauri.conf.json:32-36` points at `icons/icon.png` and the tray uses the app default icon (`crates/ocean-tauri/src/lib.rs:1097-1132`).

### Exploratory, not yet accepted as a replacement

`docs/OCEAN_WEB_SURFACE_DESIGN.md:25-39` asks to test three lockups—mark-only, lowercase `ocean`, and tighter uppercase `OCEAN`—before choosing. This means:

- The **mark is accepted**; a final wordmark lockup is not established by that passage.
- Do not present model-generated typography or a new lockup as canonical.
- Poppins is the product sans and current wordmark token (`styles/tokens.css:9-39,106-109,124-125`), but any lockup choice still requires visual acceptance.
- Generate Tauri icons from the final accepted mark, not from an exploratory lockup (`docs/OCEAN_WEB_SURFACE_DESIGN.md:36-39`).

### Clear-space/minimum-size gap

No inspected source defines formal clear space, minimum reproduction size, monochrome variants, light-background treatment, or trademark rules. The two future documents should mark these as **TBD/review-required**, not fabricate standards. A practical draft may propose test matrices, but must not call them canonical without approval.

## 5. Iconography opportunities

The existing product pattern is the safest basis for an asset system:

- Inline SVGs inherit `currentColor`; the dominant family is 24×24 outline geometry, `stroke-width="2"`, round caps/joins, `1em` sizing (`crates/ocean-surface-ui/src/icons.rs:323-379,406-486`). `styles/chrome.css:102-122` preserves outline icons against the global fill rule.
- Real third-party service logos may retain official geometry while rendering in `currentColor` (Slack precedent at `crates/ocean-surface-ui/src/icons.rs:503-513`). Do not invent pseudo-logos for integrations.
- Product-relevant concepts already represented include menu/overflow, groups/council, capture, audio, phone/mic, folder/files, git branch, terminal, globe/browser, desktop/mobile, waves/live voice, send/stop, person/agent, settings/tools (`crates/ocean-surface-ui/src/icons.rs:323-726`). Extend this vocabulary rather than generate a second illustrative family.
- High-value future icon opportunities, subject to product need: approval/permission, session focus/background work, needs-reply, browser-driving state, repo/check/run state, artifact/diff, recall/search, notification/badge, and host capability. Keep them literal, compact, single-purpose, and state-colored through tokens.
- Do **not** turn icons into icon+heading+subtitle cards. Icons should support labeled controls or legible state, not become decorative category art (`docs/OCEAN_WEB_SURFACE_DESIGN.md:180-185,293-295`).
- No emoji glyphs in UI (`docs/OCEAN_PLATFORM_CONTRACT.md:104-108`).

## 6. Prompt-writing guardrails for ChatGPT image generation

### Prompt construction

Every generation prompt should specify, in this order:

1. **Asset role and surface:** app icon, auxiliary state icon, background texture, launch/marketing crop, etc.; name web/PWA, extension, Tauri, or mobile context.
2. **Attach the canonical reference:** use `public/brand/master-1024.png` as image reference. State “preserve this exact circular silhouette and wave geometry; do not redesign the logo.”
3. **Composition and dimensions:** exact canvas, transparent/opaque requirement, safe zone, crop behavior, and whether the mark is composited unchanged.
4. **Material:** near-black matte coin, shallow rim/specular relief, turquoise-to-blue depth inside the badge only, restrained soft shadow.
5. **Palette:** include exact token hex values relevant to the asset; explicitly reserve cyan for identity/live/primary-state uses.
6. **Negative prompt:** no purple/magenta, mint UI chrome, text, letters, terminal/code symbols, cursor, brackets, sparkle/star, mascot, glassmorphism, neon cyberpunk, lens flare, extra waves, extra rings, pseudo-3D bevel excess, or white text on cyan.
7. **Output constraints:** no watermark, no mock device frame unless requested, no generated typography, centered/aligned geometry, clean alpha edge.

### Model-use boundaries

- **Never ask the image model to redraw the canonical logo from prose.** Composite the checked-in PNG/SVG deterministically. Image models are suitable for supporting scenes/textures or exploratory concepts, not production logo geometry.
- **Never trust generated text or wordmarks.** Typeset approved lockups in a design/vector tool using the approved font and spacing after generation.
- Ask for one controlled variable per iteration (material, depth, background, or crop), not simultaneous logo, palette, and composition redesign.
- Request flat/orthographic output for icons. Avoid perspective, mockup lighting, or photoreal ocean imagery unless the deliverable is explicitly campaign art; those effects undermine small-size recognition.
- Require visual review at 16/20/32/48 px for icon-like outputs and on both transparent and `#060606` stages. Check alpha halos and rim noise.

### Reusable prompt skeleton

> Create a **[asset role]** for Ocean **[surface]**, **[dimensions]**. Use the attached `master-1024.png` as an immutable brand mark: preserve its exact circular silhouette, rim, stacked wave spacing, and turquoise-to-blue internal depth; do not redraw or reinterpret it. Stage on **[transparent / #060606]** with **[safe-zone/crop]**. Material is matte near-black with shallow overhead specular relief, restrained shadow, and no decorative glow except **[state reason]**. Use only **[token hexes]**. No text, letters, watermark, purple, magenta, mint chrome, terminal/code/cursor/bracket symbols, sparkle/star motifs, mascots, glassmorphism, cyberpunk neon, extra wave bands, or extra rings. Deliver **[format]**, centered, clean alpha, crisp at **[target sizes]**.

## 7. Asset deliverable specifications

### Existing deterministic deliverables (evidence-backed)

`scripts/build-brand-assets.mjs:1-31,286-319` documents and emits:

| Deliverable | Specification |
|---|---|
| `public/brand/master-1024.png` | 1024×1024, transparent background, badge master |
| `public/brand/ocean-mark.svg` | portable 1024-sized vector, literal/pre-resolved colors, no external CSS dependency |
| `public/icon-512.png` | 512×512, badge at ~80% on opaque `#060606`, maskable |
| `public/icon-192.png` | 192×192, same maskable treatment |
| `public/apple-touch-icon.png` | 180×180, opaque `#060606` because Apple drops alpha |
| `crates/ocean-tauri/icons/*` | full Tauri desktop/mobile icon set generated from the master via `cargo tauri icon`, iOS color `#060606` |

The script explains the geometry calculation: the badge is 42/48 of its SVG canvas and is sized to approximately 80% of the maskable canvas (`scripts/build-brand-assets.mjs:34-47`). Existing files were confirmed as PNG 1024 RGBA, 192 RGB, 512 RGB, and 180 RGB.

### Requirements for any new asset package

- Supply a source/master plus exported sizes; do not deliver only a generated bitmap.
- Preserve a transparent master and explicit opaque-background platform exports.
- Include a small-size contact sheet (16, 20, 24, 32, 48, 128 px) and dark-stage preview.
- Record prompt/model/version/seed or generation ID for reproducibility, but keep generated provenance out of the image pixels.
- Include exact color/alpha values, dimensions, intended host, safe-zone/crop notes, and accessibility purpose/alt-text guidance.
- For UI icons, deliver optimized SVG with `viewBox`, `currentColor`, no embedded raster, no text, and geometry compatible with the existing 24×24/2px round-stroke family.
- For logo-bearing production exports, composite from the checked-in master/vector rather than tracing a generated approximation.
- Review in compact extension width and touch/mobile contexts, not desktop alone (`docs/OCEAN_PLATFORM_CONTRACT.md:63-83`).

## 8. Cross-surface consistency rules

- One core, many shells: the same visual grammar and product identity must survive browser/PWA, Chrome side panel, Tauri desktop, and future mobile; platform shells add capability, not a new brand (`docs/OCEAN_PLATFORM_CONTRACT.md:13-42`).
- Tauri consumes the same Trunk bundle, so a shared logo/component fix covers browser and native content (`docs/OCEAN_WEB_SURFACE_DESIGN.md:36-38`).
- Extension uses the same system at tighter padding and must never fork colors (`docs/OCEAN_WEB_SURFACE_DESIGN.md:334-335`).
- `styles/tokens.css` is the only color-definition authority. Generated-document swatches should map back to token names; they must not encourage local literals in product CSS (`docs/OCEAN_WEB_SURFACE_DESIGN.md:55-57`; `AGENTS.md:76-80`).
- Desktop posture may be richer, but the shared center remains transcript-first and the web is not a degraded desktop (`docs/OCEAN_DESKTOP_NORTH_STAR.md:149-166,305-308`). Assets should scale from compact/touch to desktop rather than create desktop-only visual semantics.
- Touch cannot rely on hover-only affordances; compact/mobile uses `styles/compact.css`, safe areas, and visible/tappable floors (`docs/OCEAN_PLATFORM_CONTRACT.md:63-83`).
- Status semantics are stable across surfaces: live = accent, connected = green, reconnecting/running = amber, failure/barge = red (`docs/OCEAN_WEB_SURFACE_DESIGN.md:83-89,325-333`). Do not recolor status merely for aesthetic harmony.
- Typography: Poppins for UI; mono only for data such as paths, IDs, tokens, numbers, and code—not UI labels (`docs/OCEAN_WEB_SURFACE_DESIGN.md:96-103`).

## 9. Important implementation risks and unresolved conflicts

1. **Generator/source drift (material risk).** `scripts/build-brand-assets.mjs:4-12` says the `icons.rs` WaveBadge is a final SVG and copies SVG markup into the generator. Current `crates/ocean-surface-ui/src/icons.rs:1-181` instead implements a Canvas 2D “Tide Coin” with an animated waterline, and commit history shows this arrived later (`ca076e0`). Running the generator may therefore overwrite canonical exports from stale SVG/CSS assumptions. Reconcile generator, live component, and accepted master before regenerating production assets.
2. **Badge token comment has a stale source path.** `styles/tokens.css:77-82` cites `docs/assets/ocean-logo-circular-neumorphic-reference.png`, which does not exist in this checkout. The actual accepted reference path is `public/brand/master-1024.png` per the binding design doc and AGENTS guide.
3. **Static accepted mark vs live Tide Coin are related but not identical implementations.** The static SVG uses fixed stacked strata; the live component draws a dynamic water body and foam crest. The future docs should describe one identity family while clearly distinguishing immutable production logo exports from state animation in UI.
4. **Lockup standards remain undecided.** Mark-only/lowercase/uppercase are tests, not three approved logos. Formal clear space, min size, monochrome, light-background, and misuse examples need explicit design approval.
5. **Current workspace is dirty and no visual acceptance screenshots were generated.** This recon intentionally did not build or mutate assets. Any next agent should avoid attributing unrelated working-tree edits to this task.

## 10. Google Labs DESIGN.md packaging requirement (mid-run update)

Both final deliverables must be **standalone Google Labs DESIGN.md-format files**, not supplements that depend on this brief or on one another. The current public specification is the Google Labs `design.md` alpha format ([repository](https://github.com/google-labs-code/design.md), [specification home](https://stitch.withgoogle.com/docs/design-md/specification)); it combines normative YAML front matter with ordered Markdown rationale.

### Required standalone structure

Each file should begin directly with YAML front matter and independently define every token it references. Do not put a title, preamble, or cross-document include before the opening `---`. Use the canonical prose sections, when present, in this exact order:

1. `## Overview` (alias accepted by the spec: `Brand & Style`)
2. `## Colors`
3. `## Typography`
4. `## Layout` (alias: `Layout & Spacing`)
5. `## Elevation & Depth` (alias: `Elevation`)
6. `## Shapes`
7. `## Components`
8. `## Do's and Don'ts`

Google Labs DESIGN.md allows unknown/custom `##` sections and preserves them, but duplicate headings are invalid. Therefore put specialized prose **only after** the canonical eight sections. Recommended custom tails:

- **Claude Design file:** `## Product Register`, `## Responsive and Cross-Surface Behavior`, `## Motion and State`, `## Logo and Iconography`, `## Review Checklist`.
- **ChatGPT image-generation file:** `## Canonical Asset Inputs`, `## Asset Generation Boundaries`, `## Prompt Construction`, `## Negative Prompt Constraints`, `## Deliverable Matrix`, `## Generation QA and Escalation`.

The image-generation document must still contain the canonical sections first; it is not merely a prompt appendix. Its custom sections should carry the prose in §§4–7 of this brief, particularly immutable-logo handling, the prompt skeleton, output dimensions, and generator drift.

### Front-matter schema and exact Ocean mapping

The alpha schema recognizes top-level `version`, `name`, `description`, `colors`, `typography`, `rounded`, `spacing`, and `components`. Token references use `{path.to.token}`. Component properties such as `backgroundColor`, `textColor`, `typography`, `rounded`, `padding`, `size`, `height`, and `width` are recognized. Unknown component properties produce warnings, so keep generation-specific metadata in Markdown custom sections rather than inventing YAML component fields.

Use semantic DESIGN.md names while preserving exact source values and documenting their source-token mapping in `## Colors`. At minimum, each standalone file should carry this normative core (expand with status/badge tokens when referenced):

```yaml
---
version: alpha
name: Ocean
description: Task-first Ocean product identity for one shared core across web, PWA, extension, Tauri, and future mobile.
colors:
  primary: "#00D7D7"          # source: --ocean-6 / --accent
  on-primary: "#03181A"       # source: --fg-on-accent
  primary-bright: "#00FFD7"   # source: --ocean-7 / --accent-bright
  primary-deep: "#0087AF"     # source: --ocean-4 / --accent-deep
  ocean-1: "#00005F"
  ocean-2: "#0000AF"
  ocean-3: "#005FAF"
  ocean-4: "#0087AF"
  ocean-5: "#00AFD7"
  ocean-6: "#00D7D7"
  ocean-7: "#00FFD7"
  ocean-8: "#5FFFFF"
  background: "#060606"       # source: --bg
  surface-raised: "#0A0A0A"  # source: --bg-raised
  surface-elevated: "#141414" # source: --bg-elevated
  surface-hover: "#1B1C21"   # source: --bg-hover
  surface-well: "#23252B"    # source: --bg-well
  text-primary: "#FAFCFF"     # source: --fg
  text-secondary: "#B8B9BB"   # source: --fg-2
  text-metadata: "#909098"    # source: --fg-3
  text-disabled: "#5A5C63"    # source: --fg-4
  status-success: "#1ED760"   # source: --ok
  status-error: "#FF4D67"     # source: --err
  status-warning: "#FFB224"   # source: --warn
  status-info: "#6AA6FF"      # source: --info
typography:
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
  label-caps:
    fontFamily: Poppins
    fontSize: 11px
    fontWeight: 600
    letterSpacing: 0.08em
  data:
    fontFamily: "SF Mono, SFMono-Regular, Menlo, monospace"
    fontSize: 12px
    fontWeight: 400
rounded:
  sm: 4px
  md: 6px
  lg: 10px
  pill: 999px
spacing:
  xs: 4px
  sm: 8px
  md: 12px
  lg: 16px
  xl: 24px
  2xl: 32px
  3xl: 48px
components:
  button-primary:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.on-primary}"
    typography: "{typography.body}"
    rounded: "{rounded.md}"
    height: 34px
    padding: 0 14px
  input:
    backgroundColor: "{colors.surface-raised}"
    textColor: "{colors.text-primary}"
    typography: "{typography.body}"
    rounded: "{rounded.md}"
    height: 34px
---
```

**Drafting caution:** YAML comments above are useful source annotations but the prose `## Colors` should also state the CSS mapping explicitly so downstream tools do not lose rationale. If the linter rejects a multi-font `fontFamily` string or shorthand padding, simplify those fields rather than changing the Ocean source values. Include every front-matter color in at least one component or rationale; the current linter warns on orphaned color tokens.

### Badge-only token extension

If either file discusses rendering or compositing the circular mark, include the badge colors in that file’s `colors` map with exact values: `badge-face #020203`, `badge-rim rgba(148,156,160,0.22)`, `badge-band-1 #4fd8c0`, `badge-band-2 #2bb4a6`, `badge-band-3 #1f95b8`, `badge-band-4 #2170be`, `badge-band-5 #2453bc`, `badge-band-6 #1a2f8e`, `badge-foam #eafcff`, `badge-split #05101f`, and `badge-gloss #cff4ff` (`styles/tokens.css:77-103`). In prose, repeat that these are **badge-confined colors**, not a second UI palette and not an exception allowing legacy teal-mint chrome.

### Custom asset-generation prose contract

After `## Do's and Don'ts`, the ChatGPT-oriented standalone file should contain copy-ready prose along these lines:

- Under `## Canonical Asset Inputs`: identify `public/brand/master-1024.png` as immutable raster reference and `public/brand/ocean-mark.svg` as portable vector; provide existing hashes and explain that TUI ASCII is heritage, not a logo input.
- Under `## Asset Generation Boundaries`: state that the image model may create supporting scenes, textures, or exploratory non-logo imagery, but production logo pixels must be composited from checked-in sources. Generated text and traced/redrawn marks are prohibited.
- Under `## Prompt Construction`: include the seven-part ordering in §6 and the reusable prompt skeleton verbatim or nearly verbatim. Every prompt must name exact DESIGN.md color tokens (for example `{colors.background}`, `{colors.primary}`, `{colors.badge-band-1}`) and also include hex values because image generators do not reliably resolve token references.
- Under `## Negative Prompt Constraints`: repeat the complete negatives as affirmative “must not” statements; do not rely on a pointer to the UI document. At minimum: no magenta/purple, legacy mint chrome, terminal/prompt/code/cursor/brackets, sparkles/stars, mascots, generated text, gradient-clipped text, white-on-cyan controls, glassmorphism, cyberpunk neon, lens flare, extra rings/waves, pseudo-3D bevel excess, decorative glow, watermark, or device frame unless requested.
- Under `## Deliverable Matrix`: reproduce the exact PNG/SVG/PWA/Apple/Tauri specifications in §7 and separate **existing checked-in outputs** from **requirements for new generated supporting assets**.
- Under `## Generation QA and Escalation`: require small-size/contact-sheet review, alpha-edge checks on transparent and `#060606`, cross-surface checks, prompt/provenance recording, and escalation for geometry, lockup, clear-space, minimum-size, monochrome/light variants, or generator execution.

### DESIGN.md validation expectations

For each final standalone file, run:

```sh
npx @google/design.md lint path/to/DESIGN.md
```

The linter checks broken references, primary color presence, contrast, orphan tokens, typography, section order, and unknown keys. A valid handoff should have no errors; warnings must be reviewed and either fixed or explicitly justified. Also inspect that custom sections appear after `## Do's and Don'ts`, no canonical heading is duplicated, all `{...}` references resolve within the same file, and neither document depends on a link to the other for hard constraints.

## 11. Compact meta-prompt contract for the next agent

### Goal

Produce two concise, production-usable, **standalone Google Labs DESIGN.md-format** Ocean documents: one optimized for Claude Design software to generate/refine UI concepts, and one optimized for ChatGPT image generation to create controlled asset outputs without redesigning the accepted identity. Each must contain complete YAML tokens, canonical sections in specification order, and specialized custom sections only after `## Do's and Don'ts`.

### Context/evidence

Use the authority and file evidence in this brief, especially `docs/OCEAN_WEB_SURFACE_DESIGN.md:1-145,193-335`, `docs/OCEAN_PLATFORM_CONTRACT.md:13-42,63-108`, `public/brand/master-1024.png`, `public/brand/ocean-mark.svg`, `styles/tokens.css:42-145`, `scripts/build-brand-assets.mjs:1-47,286-319`, and `../ocean-os/crates/ocean-tui/src/splash.rs:13-36`.

### Success criteria

- Both files lint as standalone Google Labs DESIGN.md documents with no broken references or section-order errors.
- Both documents identify the accepted mark and TUI-derived depth ramp.
- Hard bans and negative prompts are explicit and impossible to confuse with inspiration.
- Canonical assets are separated from exploratory lockups and generated supporting art.
- Exact palette, material, typography, icon, motion, export, and cross-surface rules are included.
- Claude Design guidance preserves task-first product register and conditional density.
- ChatGPT prompts require reference-image attachment, immutable logo geometry, exact outputs, and production compositing rather than model-redrawn logos.
- Gaps (clear space, minimum size, monochrome/light variants, final wordmark lockup) remain labeled TBD/review-required.

### Hard constraints

- Do not replace or reinterpret the circular wave mark.
- Do not import Rising Tides magenta/purple or legacy teal-mint UI language.
- Do not create terminal/code/cursor/sparkle/mascot identity.
- Do not introduce shell-specific colors or a second visual system.
- Do not claim exploratory lockups or undocumented sizing rules are approved.
- Do not run the brand generator until its drift from the Canvas Tide Coin is resolved.
- Do not put specialized custom sections before or between the canonical DESIGN.md sections.
- Do not make either file depend on the other for tokens, negative constraints, or asset rules.

### Suggested approach

Duplicate one shared brand kernel into both standalone docs (do not cross-reference it), then specialize: Claude Design gets product register, component/material recipes, density and responsive behavior; ChatGPT gets prompt templates, negative prompts, generation boundaries, exact export matrices, and QA checklists. Prefer tables and copy-ready prompt blocks over narrative.

### Validation

Run `npx @google/design.md lint` on each final file, then cross-check every color against `styles/tokens.css`; every logo claim against the PNG/SVG and identity section; every export against the manifest/generator; and every shell claim against the platform contract. Review generated examples at compact and desktop sizes and with reduced-motion/touch constraints. Request human approval for lockups or new canonical variants.

### Stop/escalation rules

Stop when the two documents can guide work without inventing brand standards. Escalate any request to alter logo geometry, approve a wordmark lockup, define legal/trademark usage, choose clear space/minimum size, add a light/monochrome logo, or regenerate checked-in production assets. Do not silently resolve generator/live-component drift.

### Resolved assumptions

- “Ocean” refers to the product identity, not Rising Tides corporate branding.
- The current accepted static reference is `public/brand/master-1024.png`; the SVG is its portable counterpart.
- The ASCII banner is palette heritage and a landing/TUI expression, not the logo.
- Supporting image generation may explore scenes/textures, but production logo pixels come from checked-in sources.

## Acceptance review

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Created only the requested read-only recon artifact; no project/source file was modified by this task."
    },
    {
      "id": "criterion-2",
      "status": "satisfied",
      "evidence": "Brief cites binding design/platform docs, accepted PNG/SVG assets, exact token and TUI sources, generator/export behavior, live icon implementation, manifest/index/Tauri usage, hashes, and explicit residual risks."
    }
  ],
  "changedFiles": [
    ".pi-subagents/artifacts/outputs/705606dd-1e07-4f5c-994c-7b7bbf11ffc5/recon/brand-asset-brief.md"
  ],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    {
      "command": "Repository find/grep/read inspection across docs, public/brand, styles, UI icons, PWA/Tauri configuration, and ../ocean-os TUI splash",
      "result": "passed",
      "summary": "Located and inspected brand authority, accepted assets, palette, icon system, platform contract, exports, and TUI identity source."
    },
    {
      "command": "file public/brand/master-1024.png public/brand/ocean-mark.svg public/icon-192.png public/icon-512.png public/apple-touch-icon.png",
      "result": "passed",
      "summary": "Confirmed master/vector formats and PNG dimensions/color modes."
    },
    {
      "command": "shasum -a 256 public/brand/master-1024.png public/brand/ocean-mark.svg",
      "result": "passed",
      "summary": "Recorded canonical asset fingerprints used in this brief."
    },
    {
      "command": "git status --short; git diff --cached --name-only; equivalent checks in ../ocean-os",
      "result": "passed",
      "summary": "Confirmed pre-existing dirty worktrees and no staged files in either inspected repository."
    },
    {
      "command": "web_search: Google Labs DESIGN.md format canonical sections and alpha schema",
      "result": "passed",
      "summary": "Confirmed YAML front matter, canonical section order, custom-section behavior, token-reference syntax, and lint command from the current Google Labs specification."
    },
    {
      "command": "git log -- public/brand scripts/build-brand-assets.mjs public icon outputs",
      "result": "passed",
      "summary": "Confirmed tracked brand-asset history and later Tide Coin implementation commit."
    }
  ],
  "validationOutput": [
    "Requested artifact written at the authoritative output path.",
    "No build or asset generation was run because the task is read-only and generator drift is a documented risk.",
    "Evidence distinguishes accepted mark, exploratory lockups, and current animated Tide Coin implementation."
  ],
  "residualRisks": [
    "The current checkout contains unrelated unstaged edits; desktop-document evidence includes workspace-state wording.",
    "Brand generator comments/markup are out of sync with the current Canvas Tide Coin implementation and should not be run until reconciled.",
    "Clear space, minimum size, monochrome/light variants, and final wordmark lockup are not defined in inspected authority and require human approval."
  ],
  "noStagedFiles": true,
  "diffSummary": "Added one read-only context brief under the required .pi-subagents artifact path; no project/source edits.",
  "reviewFindings": [
    "no blockers for recon handoff",
    "warning: scripts/build-brand-assets.mjs describes a final SVG source while current icons.rs uses a Canvas Tide Coin",
    "warning: styles/tokens.css cites a nonexistent docs/assets logo reference; binding docs point to public/brand/master-1024.png"
  ],
  "manualNotes": "Review gate remains required. Mid-run steering incorporated: both final outputs must be standalone Google Labs DESIGN.md files with complete YAML tokens, canonical ordered sections, and specialized custom sections after Do's and Don'ts. The next agent must not regenerate assets or approve unresolved logo variants."
}
```
