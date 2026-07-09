#!/usr/bin/env node
/**
 * Ocean Surface — deterministic brand-asset renderer.
 *
 * The WaveBadge SVG mark (crates/ocean-surface-ui/src/icons.rs `WaveBadge`)
 * is FINAL but carries ZERO inline color: every fill/stroke/stop/filter
 * resolves through `.wave-badge__*` CSS using color-mix() over --badge-* /
 * --ocean-* tokens. resvg/librsvg/sharp cannot render that, so this script
 * rasterizes through a headless browser (pixel parity with the shipped mark,
 * drop-shadow relief included) and ALSO emits a portable vector master
 * (public/brand/ocean-mark.svg) with every color PRE-RESOLVED to literal hex
 * via getComputedStyle — no CSS classes, no external deps.
 *
 * Outputs (regenerated every run, from repo root):
 *   public/brand/master-1024.png   1024×1024 badge on TRANSPARENT bg (master)
 *   public/brand/ocean-mark.svg    portable vector master, colors pre-resolved
 *   public/icon-512.png            512×512  maskable (badge 80% on --bg)
 *   public/icon-192.png            192×192  maskable
 *   public/apple-touch-icon.png    180×180  opaque --bg (Apple drops alpha)
 *   crates/ocean-tauri/icons/*     full Tauri set via `cargo tauri icon`
 *
 * Run:  node scripts/build-brand-assets.mjs   (or: bun scripts/build-brand-assets.mjs)
 *
 * Determinism note: launches system Chrome via channel:'chrome' so the render
 * is stable, offline-safe, and recent enough to resolve color-mix in computed
 * stop-color/filter — never a version-pinned browser download.
 */

import { chromium } from 'playwright';
import { readFileSync, writeFileSync, mkdirSync, readdirSync } from 'node:fs';
import { execSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(__dirname, '..');
const PUBLIC = path.join(REPO, 'public');
const BRAND = path.join(PUBLIC, 'brand');
const TAURI_ICONS = path.join(REPO, 'crates', 'ocean-tauri', 'icons');

// --bg page token (styles/tokens.css) — opaque background for maskable/apple.
const BG = '#060606';
// Badge circle is r21 in a 48-unit viewBox → diameter 42/48 = 0.875 of the svg.
// Maskable safe zone wants the mark at ~80% of the canvas.
const MASKABLE_MARK_FRAC = 0.80;
const BADGE_DIAMETER_FRAC = 42 / 48; // 0.875
const maskableSvgSize = (canvas) => Math.round((MASKABLE_MARK_FRAC / BADGE_DIAMETER_FRAC) * canvas);

// ── Badge SVG markup ───────────────────────────────────────────────────────
// Copied from crates/ocean-surface-ui/src/icons.rs `WaveBadge` (non-spinning).
// Geometry + gradient defs are pixel-measured from the reference; ALL paint
// (fill/stroke/stroke-width/stroke-linecap/stop-color/filter) is authored in
// styles/transcript.css `.wave-badge__*` and resolves here at render time.
// Keep this in sync with icons.rs if the mark changes.
function badgeSvg(width) {
  return `<svg class="wave-badge" viewBox="0 0 48 48" width="${width}" height="${width}" fill="none" aria-hidden="true">
    <defs>
      <clipPath id="wave-badge-clip"><circle cx="24" cy="24" r="20.4" /></clipPath>
      <linearGradient id="wave-badge-face-g" gradientUnits="userSpaceOnUse" x1="0" y1="3" x2="0" y2="45">
        <stop offset="0" class="wave-badge__stop-face-hi" />
        <stop offset="0.55" class="wave-badge__stop-face" />
      </linearGradient>
      <linearGradient id="wave-badge-arc-g" gradientUnits="userSpaceOnUse" x1="0" y1="4.35" x2="0" y2="17.95">
        <stop offset="0" class="wave-badge__stop-arc-hi" />
        <stop offset="0.12" class="wave-badge__stop-arc" />
        <stop offset="1" class="wave-badge__stop-arc-lo" />
      </linearGradient>
      <linearGradient id="wave-badge-w1-g" gradientUnits="userSpaceOnUse" x1="0" y1="16" x2="0" y2="20.8">
        <stop offset="0" class="wave-badge__stop-w1-hi" />
        <stop offset="0.14" class="wave-badge__stop-w1" />
        <stop offset="1" class="wave-badge__stop-w1-lo" />
      </linearGradient>
      <linearGradient id="wave-badge-w2-g" gradientUnits="userSpaceOnUse" x1="0" y1="24.7" x2="0" y2="29.5">
        <stop offset="0" class="wave-badge__stop-w2-hi" />
        <stop offset="0.14" class="wave-badge__stop-w2" />
        <stop offset="1" class="wave-badge__stop-w2-lo" />
      </linearGradient>
      <linearGradient id="wave-badge-fill-g" gradientUnits="userSpaceOnUse" x1="0" y1="33" x2="0" y2="39.6">
        <stop offset="0" class="wave-badge__stop-fill-hi" />
        <stop offset="0.22" class="wave-badge__stop-fill" />
        <stop offset="1" class="wave-badge__stop-fill-lo" />
      </linearGradient>
      <linearGradient id="wave-badge-bar-g" gradientUnits="userSpaceOnUse" x1="0" y1="42.65" x2="0" y2="44.85">
        <stop offset="0" class="wave-badge__stop-bar-hi" />
        <stop offset="0.18" class="wave-badge__stop-bar" />
        <stop offset="1" class="wave-badge__stop-bar-lo" />
      </linearGradient>
      <linearGradient id="wave-badge-rim-g" gradientUnits="userSpaceOnUse" x1="0" y1="3" x2="0" y2="45">
        <stop offset="0" class="wave-badge__stop-rim-hi" />
        <stop offset="1" class="wave-badge__stop-rim-lo" />
      </linearGradient>
      <radialGradient id="wave-badge-vig-g">
        <stop offset="0.62" class="wave-badge__stop-vig-in" />
        <stop offset="1" class="wave-badge__stop-vig-out" />
      </radialGradient>
      <radialGradient id="wave-badge-occl-g">
        <stop offset="0.78" class="wave-badge__stop-occl-in" />
        <stop offset="0.94" class="wave-badge__stop-occl-mid" />
        <stop offset="1" class="wave-badge__stop-occl-out" />
      </radialGradient>
    </defs>
    <circle class="wave-badge__face" cx="24" cy="24" r="21" />
    <circle class="wave-badge__vig" cx="24" cy="24" r="20.6" />
    <circle class="wave-badge__ring" cx="24" cy="24" r="20.7" />
    <g clip-path="url(#wave-badge-clip)" stroke-linecap="round">
      <path class="wave-badge__arc" stroke-linecap="butt" d="M 8.2 16 A 17.7 17.7 0 0 1 39.8 16" />
      <path class="wave-badge__w1" d="M -22 18.4 q 5.5 -1.9 11 0 t 11 0 t 11 0 t 11 0 t 11 0 t 11 0 t 11 0 t 11 0" />
      <path class="wave-badge__w2" d="M -22 27.1 q 5.5 -1.9 11 0 t 11 0 t 11 0 t 11 0 t 11 0 t 11 0 t 11 0 t 11 0" />
      <path class="wave-badge__fill" d="M -22 33.6 q 5.5 1.7 11 0 t 11 0 t 11 0 t 11 0 t 11 0 t 11 0 t 11 0 t 11 0 L 66 39.6 L -22 39.6 Z" />
      <path class="wave-badge__bar" d="M 15.5 43.75 L 32.5 43.75" />
    </g>
    <circle class="wave-badge__occl" cx="24" cy="24" r="20.4" />
  </svg>`;
}

// ── CSS: tokens (:root) + the .wave-badge__* rules from transcript.css ──────
// @font-face url()s are stripped (the mark has no text; avoids file:// 404s).
function loadCss() {
  const tokens = readFileSync(path.join(REPO, 'styles', 'tokens.css'), 'utf8')
    .replace(/@font-face\s*\{[^}]*\}/g, '');
  const transcript = readFileSync(path.join(REPO, 'styles', 'transcript.css'), 'utf8');
  return `${tokens}\n${transcript}`;
}

function pageHtml({ canvas, background, maskable }) {
  const badgeWidth = maskable ? maskableSvgSize(canvas) : canvas;
  const bodyBg = background ?? 'transparent';
  return `<!doctype html>
<html><head><meta charset="utf-8"><style>
${CSS}
html, body { margin: 0; padding: 0; }
body {
  width: ${canvas}px; height: ${canvas}px;
  background: ${bodyBg};
  display: flex; align-items: center; justify-content: center;
  overflow: hidden;
}
.wave-badge { width: ${badgeWidth}px; height: ${badgeWidth}px; }
</style></head>
<body>${badgeSvg(badgeWidth)}</body></html>`;
}

let CSS;

// ── Portable vector master: resolve every class to literal presentation attrs ─
// Runs inside the page (badge mounted with the live cascade). Reads computed
// fill/stroke/stroke-width/stroke-linecap, stop-color (+ stop-opacity for the
// alpha stops), and the resolved filter (drop-shadows incl. the root aura),
// inlining them so the output has NO classes and NO external CSS dependency.
async function emitOceanMarkSvg(page) {
  const svg = await page.evaluate(() => {
    const src = document.querySelector('svg.wave-badge');
    const clone = src.cloneNode(true);
    clone.removeAttribute('class');
    clone.setAttribute('width', '1024');
    clone.setAttribute('height', '1024');

    const parseColor = (v) => {
      if (!v) return null;
      const h = (n) => Math.round(n).toString(16).padStart(2, '0');
      // Chrome resolves color-mix() to modern color(srgb R G B [/ A]); handle it
      // first so the whitened/darkened stops don't fall through to the default.
      let m = v.match(/color\(srgb\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)(?:\s*\/\s*([\d.]+))?\s*\)/i);
      if (m) return { hex: '#' + h(+m[1] * 255) + h(+m[2] * 255) + h(+m[3] * 255), a: m[4] ? +m[4] : 1 };
      // legacy rgb()/rgba()
      m = v.match(/rgba?\(([^)]+)\)/i);
      if (m) {
        const p = m[1].split(',').map((s) => parseFloat(s));
        return { hex: '#' + h(p[0]) + h(p[1]) + h(p[2]), a: p.length === 4 ? p[3] : 1 };
      }
      return null;
    };
    const normUrl = (v) =>
      v ? v.replace(/url\(\s*['"]?#?([^'")]+)#?['"]?\s*\)/i, 'url(#$1)') : v;
    // Convert any color(srgb r g b [/ a]) inside a resolved value (filter
    // glow colors come back this way from color-mix) to literal rgba() so the
    // portable SVG carries no color-mix / modern-notation color, only literals.
    const colorToRgba = (v) =>
      v.replace(
        /color\(srgb\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)(?:\s*\/\s*([\d.]+))?\s*\)/gi,
        (_m, r, g, b, a) =>
          `rgba(${Math.round(+r * 255)}, ${Math.round(+g * 255)}, ${Math.round(+b * 255)}, ${a != null ? +a : 1})`
      );

    // Read computed styles from the ATTACHED source (the clone is detached,
    // so getComputedStyle on it returns initial values). srcs[i] and cls[i]
    // align 1:1 because clone is a deep copy — read cs from srcs[i], write to cls[i].
    const srcs = [src, ...src.querySelectorAll('*')];
    const cls = [clone, ...clone.querySelectorAll('*')];
    for (let i = 0; i < cls.length; i++) {
      const sEl = srcs[i];
      const cEl = cls[i];
      cEl.removeAttribute('class');
      const cs = getComputedStyle(sEl);
      const tag = cEl.localName;

      if (tag === 'stop') {
        const c = parseColor(cs.getPropertyValue('stop-color')) || { hex: '#000000', a: 1 };
        const so = parseFloat(cs.getPropertyValue('stop-opacity') || '1');
        cEl.setAttribute('stop-color', c.hex);
        const a = +(c.a * so).toFixed(4);
        if (a < 1) cEl.setAttribute('stop-opacity', a);
        continue;
      }
      if (tag === 'defs' || tag === 'lineargradient' || tag === 'radialgradient' || tag === 'clippath')
        continue;

      const fill = cs.getPropertyValue('fill');
      if (fill && fill !== 'none') {
        const nf = normUrl(fill);
        cEl.setAttribute('fill', nf.startsWith('url') ? nf : (parseColor(nf)?.hex || nf));
      }
      const stroke = cs.getPropertyValue('stroke');
      if (stroke && stroke !== 'none') {
        const ns = normUrl(stroke);
        cEl.setAttribute('stroke', ns.startsWith('url') ? ns : (parseColor(ns)?.hex || ns));
        const sw = cs.getPropertyValue('stroke-width');
        if (sw && sw !== 'auto') cEl.setAttribute('stroke-width', sw.replace(/px$/i, ''));
        const lc = cs.getPropertyValue('stroke-linecap');
        if (lc && lc !== 'butt') cEl.setAttribute('stroke-linecap', lc);
      }
      const filt = cs.getPropertyValue('filter');
      if (filt && filt !== 'none') cEl.setAttribute('style', `filter:${colorToRgba(filt)}`);
    }
    return new XMLSerializer().serializeToString(clone);
  });
  return `<?xml version="1.0" encoding="UTF-8"?>\n${svg}\n`;
}

// ── Render one canvas to a PNG ──────────────────────────────────────────────
async function renderPng(page, opts) {
  const { canvas, file, background, maskable, transparent } = opts;
  await page.setViewportSize({ width: canvas, height: canvas });
  await page.setContent(pageHtml({ canvas, background, maskable }), { waitUntil: 'networkidle' });
  await page.waitForTimeout(120); // let filters/relief settle
  await page.screenshot({
    path: file,
    clip: { x: 0, y: 0, width: canvas, height: canvas },
    omitBackground: !!transparent,
  });
}

function dims(file) {
  const out = execSync(`sips -g pixelWidth -g pixelHeight "${file}"`, { encoding: 'utf8' });
  const w = +out.match(/pixelWidth: (\d+)/)?.[1];
  const h = +out.match(/pixelHeight: (\d+)/)?.[1];
  return `${w}×${h}`;
}

function runTauriIcon(masterPng) {
  // Generates the full Tauri icon set from the 1024 master into crates/ocean-tauri/icons.
  // --ios-color matches the page bg so iOS icons sit on the same dark stage.
  execSync(
    `cargo tauri icon "${masterPng}" -o "${TAURI_ICONS}" --ios-color "${BG}"`,
    { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }
  );
}

async function main() {
  CSS = loadCss();
  mkdirSync(BRAND, { recursive: true });

  console.log('› launching system Chrome (channel:chrome)…');
  const browser = await chromium.launch({ channel: 'chrome' });
  const page = await browser.newPage({ deviceScaleFactor: 1 });

  const masterPng = path.join(BRAND, 'master-1024.png');
  try {
    // 1) Master render — transparent 1024×1024 (badge fills the frame; aura fits).
    console.log('› rendering transparent master 1024…');
    await renderPng(page, { canvas: 1024, file: masterPng, transparent: true });
    // ocean-mark.svg is read from this same mount (badge at 1024).
    console.log('› emitting pre-resolved ocean-mark.svg…');
    writeFileSync(path.join(BRAND, 'ocean-mark.svg'), await emitOceanMarkSvg(page));

    // 2) Maskable icons — badge at 80% on opaque --bg (manifest: any maskable).
    console.log('› rendering maskable icons (512 / 192)…');
    await renderPng(page, { canvas: 512, file: path.join(PUBLIC, 'icon-512.png'), background: BG, maskable: true });
    await renderPng(page, { canvas: 192, file: path.join(PUBLIC, 'icon-192.png'), background: BG, maskable: true });

    console.log('› rendering apple-touch-icon 180…');
    await renderPng(page, { canvas: 180, file: path.join(PUBLIC, 'apple-touch-icon.png'), background: BG, maskable: true });
  } finally {
    await browser.close();
  }

  // 4) Tauri icon set from the master.
  console.log('› generating Tauri icon set via `cargo tauri icon`…');
  runTauriIcon(masterPng);

  // 5) Report — verify every output exists with expected dimensions.
  const outputs = [
    ['public/brand/master-1024.png', masterPng, '1024×1024'],
    ['public/brand/ocean-mark.svg', path.join(BRAND, 'ocean-mark.svg'), 'svg'],
    ['public/icon-512.png', path.join(PUBLIC, 'icon-512.png'), '512×512'],
    ['public/icon-192.png', path.join(PUBLIC, 'icon-192.png'), '192×192'],
    ['public/apple-touch-icon.png', path.join(PUBLIC, 'apple-touch-icon.png'), '180×180'],
  ];
  console.log('\n──────── brand assets ────────');
  for (const [rel, file, expect] of outputs) {
    console.log(`  ${rel.padEnd(32)} ${expect === 'svg' ? 'ok' : dims(file)}  (expect ${expect})`);
  }
  console.log('\n──────── tauri icons ────────');
  for (const f of readdirSync(TAURI_ICONS).sort()) {
    if (f === '.gitkeep') continue;
    const full = path.join(TAURI_ICONS, f);
    const isPng = f.endsWith('.png');
    console.log(`  crates/ocean-tauri/icons/${f.padEnd(22)} ${isPng ? dims(full) : '(vector/archive)'}`);
  }
  console.log('\n✓ done');
}

main().catch((e) => {
  console.error('✗ brand-asset build failed:', e);
  process.exit(1);
});
