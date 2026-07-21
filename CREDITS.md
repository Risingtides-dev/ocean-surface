# Credits and acknowledgements

Ocean Surface is built with generous open-source work. This page recognizes
people and projects whose work is directly bundled or foundational to the
product. Legal terms and exact asset provenance are in [`NOTICE.md`](NOTICE.md)
and [`docs/ASSET_PROVENANCE.md`](docs/ASSET_PROVENANCE.md).

## Lucide and Feather

Thank you to the **Lucide contributors** for the clear, consistent icon system
used by the retained GPUI shell assets. Lucide's license also preserves credit
to **Cole Bemis**, creator of Feather, for the Feather-derived icon family.
Ocean's 18 vendored SVG paths are pinned, mapped to their exact upstream files,
and distributed with the complete ISC/MIT notice.

- Lucide: <https://github.com/lucide-icons/lucide>
- Audited revision: `658573b0171e693bc965c167592cc0b92d002a3e`
- Local mapping: [`crates/ocean-gui/assets/icons/ocean-gui/README.md`](crates/ocean-gui/assets/icons/ocean-gui/README.md)

## Poppins

Thank you to **Indian Type Foundry**, **Jonny Pinhorn**, **Ninad Kale**, and the
**Poppins Project Authors** for Poppins. Ocean Surface bundles Regular, Medium,
SemiBold, and Bold WOFF2 files under the SIL Open Font License 1.1. The font
software remains under OFL-1.1 and is not relicensed as Ocean code.

- Poppins: <https://github.com/itfoundry/Poppins>
- Audited Google Fonts revision: `389b770410cc0b7c21c85673bfa2077420fe7f65`
- Complete local license: [`public/fonts/OFL.txt`](public/fonts/OFL.txt)

## Product foundations

Thank you to **Greg Johnston** and the Leptos contributors for the reactive Rust
web framework at the center of the shared UI, and to the **Tauri contributors**
for the native application shell. Ocean Surface also depends on Rust, Tokio,
Axum, wasm-bindgen, web-sys, serde, Trunk, and many other open-source projects
represented in `Cargo.lock` and the JavaScript lockfiles.

Release distributions should carry generated dependency-license inventories;
this human acknowledgement does not replace package-level notices.

## No endorsement implied

Acknowledgement describes provenance and gratitude. It does not imply that any
person or upstream project endorses Ocean Surface or Rising Tides.
