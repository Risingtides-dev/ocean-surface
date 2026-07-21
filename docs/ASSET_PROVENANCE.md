# Ocean Surface Asset Provenance

Status: current-tree inventory complete; project-license decision pending.

This document summarizes the machine-verifiable inventory in
[`asset-provenance.json`](asset-provenance.json). The manifest covers every
tracked image, icon, font, audio/video file, PDF, and HTML application/design
artifact in the current public tree. Run:

```sh
node scripts/check-asset-provenance.mjs
```

The check fails when a tracked asset is absent from the manifest, a removed
asset remains listed, a byte count or SHA-256 changes, a required license file
is absent, or a record lacks provenance/license classification.

## Current inventory

- **113 tracked assets**
- **22 third-party assets cleared for redistribution**
  - 18 Lucide SVG icons under ISC/MIT
  - 4 Poppins WOFF2 fonts under SIL OFL 1.1
- **91 project-authored/generated artifacts pending the root project-license
  decision**
  - 52 generated application icons
  - 4 canonical/mirrored Ocean brand masters
  - 4 legacy Ocean application icons
  - 1 repository-authored VS Code wave icon
  - 30 application, design-study, mockup, or generated-documentation HTML files
- **0 tracked audio or video assets**
- **0 unresolved third-party asset families in the current tree**

`project_license_pending` does not mean unknown third-party provenance. It means
repository history and source/generation paths identify the asset as
Ocean/Risingtides-dev work, but the operator still must confirm the copyright
holder and choose the license governing project-authored material.

## Third-party assets

### Poppins

The four vendored files identify themselves as Poppins version 4.004, copyright
2020 The Poppins Project Authors, and link to the SIL Open Font License. The
required license now ships at [`../public/fonts/OFL.txt`](../public/fonts/OFL.txt),
from the pinned Google Fonts Poppins tree:

- <https://github.com/google/fonts/tree/389b770410cc0b7c21c85673bfa2077420fe7f65/ofl/poppins>

The fonts may be bundled and redistributed under OFL-1.1; they must remain under
that font license rather than a future Ocean source-code license.

### Lucide shell icons

The 18 soft-deprecated GPUI shell icon paths now contain pinned Lucide icons:

- upstream revision: `658573b0171e693bc965c167592cc0b92d002a3e`
- mapping and source notes:
  [`../crates/ocean-gui/assets/icons/ocean-gui/README.md`](../crates/ocean-gui/assets/icons/ocean-gui/README.md)
- required license:
  [`../crates/ocean-gui/assets/icons/ocean-gui/LICENSE-LUCIDE`](../crates/ocean-gui/assets/icons/ocean-gui/LICENSE-LUCIDE)

The prior SVG copies were attributed to Zed's primarily GPL-licensed icon tree.
They were replaced instead of carrying an uncertain copyleft boundary into a
future Ocean release.

## Ocean-owned and generated families

- `public/brand/*` is the canonical static Ocean mark family.
- `design-systems/ocean-leptos/assets/*` is an exact mirror of the canonical
  static marks.
- `public/icon-*.png`, `public/apple-touch-icon.png`, and
  `crates/ocean-tauri/icons/**` are generated application-icon derivatives.
- `scripts/build-brand-assets.mjs` and
  `crates/ocean-surface-ui/src/icons.rs` are the current reconstruction path for
  the canonical mark and application icons.
- `legacy-voice/public/icon*` is retained project-authored legacy application
  artwork.
- `vscode-extension/media/wave.svg` is a project-authored UI icon.
- Tracked HTML consists of application entry points, repository-authored design
  studies/mockups, or generated documentation artifacts.

Before applying a public project license, the operator must confirm whether the
copyright holder for original Ocean artwork and project-authored assets is an
individual, Rising Tides, or another entity.

## Removed from the current public tree

Two unreferenced implementation screenshots were removed during this audit:

- `map-render-test.png`
- `model-dropdown-halt.png`

They remain part of already-public Git history; removal reduces the current
release surface but is not represented as historical recall.

## Adding or changing an asset

1. Establish its source, author/owner, license, and redistribution terms before
   committing it.
2. Preserve the required third-party notice or license in the nearest asset
   directory.
3. Add or update its record in `asset-provenance.json`, including exact byte
   count and SHA-256.
4. Run `node scripts/check-asset-provenance.mjs`.
5. For visual assets, verify intended dimensions, transparency/background,
   compact rendering, and canonical-brand consistency.
6. Record meaningful asset-family or provenance changes in `events.md`.

Do not classify an unknown asset as project-authored merely because it appears
in repository history. Unknown provenance is a release blocker until evidence
or replacement resolves it.
