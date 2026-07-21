# Third-party notices

Ocean Surface bundles the third-party assets listed below. We are grateful to
their authors and contributors. Human acknowledgements are in
[`CREDITS.md`](CREDITS.md); per-file hashes and classifications are in
[`docs/asset-provenance.json`](docs/asset-provenance.json).

## Lucide icons

The 18 SVG files under `crates/ocean-gui/assets/icons/ocean-gui/` are pinned
Lucide icons distributed under Lucide's ISC license, with MIT terms retained
for Feather-derived icons.

- Upstream: <https://github.com/lucide-icons/lucide>
- Audited revision: `658573b0171e693bc965c167592cc0b92d002a3e`
- Copyright (c) 2026 Lucide Icons and Contributors
- Feather-derived material: Copyright (c) 2013-present Cole Bemis
- Complete license and icon-family notice:
  [`crates/ocean-gui/assets/icons/ocean-gui/LICENSE-LUCIDE`](crates/ocean-gui/assets/icons/ocean-gui/LICENSE-LUCIDE)

The local directory README records the exact upstream icon mapped to every
retained Ocean filename. The files are redistributed without claiming Ocean
authorship.

## Poppins fonts

The four WOFF2 files under `public/fonts/` are Poppins Regular, Medium,
SemiBold, and Bold, version 4.004.

- Upstream: <https://github.com/itfoundry/Poppins>
- Audited Google Fonts revision: `389b770410cc0b7c21c85673bfa2077420fe7f65`
- Copyright 2020 The Poppins Project Authors
- Designers recorded by Google Fonts: Indian Type Foundry, Jonny Pinhorn,
  Ninad Kale
- License: SIL Open Font License 1.1
- Complete license: [`public/fonts/OFL.txt`](public/fonts/OFL.txt)

The font software remains under OFL-1.1. It is not covered by Ocean Surface's
`MIT OR Apache-2.0` project license.

## Dependency notices

Rust and JavaScript dependencies remain under the licenses declared by their
packages and lockfiles. Release artifacts should include generated dependency
license inventories. Third-party names and marks belong to their respective
owners; inclusion here does not imply endorsement of Ocean Surface or Rising
Tides.
