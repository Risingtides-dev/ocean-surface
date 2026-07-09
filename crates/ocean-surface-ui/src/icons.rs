//! Inline SVG icons (game-icons.net, CC-BY 3.0). Each renders at 1em and
//! inherits `currentColor`, so CSS color controls them — they match the
//! site palette (cyan --accent, etc.) wherever they're placed.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use leptos::prelude::*;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

// ── Living-water math ─────────────────────────────────────────────────
// The badge is six depth strata whose boundaries are traveling waves. Each
// edge sums two low-frequency components (laminar surface flow) with slow
// amplitude breathing + a whisper ripple; amplitude tapers with depth so the
// surface rolls and the abyss stays calm. Ported 1:1 from the approved
// mockups/animated-badge-v15.html.
struct Edge {
    y0: f64,
    a: f64,
    k: f64,
    spd: f64,
    ph: f64,
}

const EDGES: [Edge; 5] = [
    Edge {
        y0: 14.0,
        a: 1.50,
        k: 0.33,
        spd: 0.85,
        ph: 0.2,
    },
    Edge {
        y0: 20.4,
        a: 1.15,
        k: 0.44,
        spd: 1.05,
        ph: 1.5,
    },
    Edge {
        y0: 25.2,
        a: 0.92,
        k: 0.52,
        spd: 0.75,
        ph: 2.8,
    },
    Edge {
        y0: 31.6,
        a: 0.82,
        k: 0.40,
        spd: 0.95,
        ph: 4.0,
    },
    Edge {
        y0: 36.6,
        a: 0.66,
        k: 0.58,
        spd: 0.68,
        ph: 5.2,
    },
];
const STEP: f64 = 0.4;
const X0: f64 = 1.5;
const X1: f64 = 46.5;

fn ey(e: &Edge, x: f64, t: f64) -> f64 {
    let br = 1.0 + 0.11 * (0.55 * t + e.ph).sin();
    let s = (e.k * x - e.spd * t + e.ph).sin()
        + 0.44 * (1.7 * e.k * x - 1.28 * e.spd * t + e.ph * 1.6 + 0.7).sin();
    e.y0 + e.a * br * s + e.a * 0.09 * (3.0 * e.k * x + 0.5 * e.spd * t + e.ph).sin()
}

fn samp(e: &Edge, t: f64) -> Vec<(f64, f64)> {
    let mut p = Vec::new();
    let mut x = X0;
    while x <= X1 + 0.01 {
        p.push((x, ey(e, x, t)));
        x += STEP;
    }
    p
}

fn edge_d(p: &[(f64, f64)]) -> String {
    let mut d = format!("M{:.2} {:.2}", p[0].0, p[0].1);
    for q in &p[1..] {
        d.push_str(&format!("L{:.2} {:.2}", q.0, q.1));
    }
    d
}

fn band_top(top: &[(f64, f64)], bot_y: f64) -> String {
    let mut d = edge_d(top);
    d.push_str(&format!("L{:.2} {:.2}L{:.2} {:.2}Z", X1, bot_y, X0, bot_y));
    d
}

fn band2(top: &[(f64, f64)], bot: &[(f64, f64)]) -> String {
    let mut d = edge_d(top);
    for q in bot.iter().rev() {
        d.push_str(&format!("L{:.2} {:.2}", q.0, q.1));
    }
    d.push('Z');
    d
}

struct Paths {
    bands: [String; 6],
    edges: [String; 5],
}

fn compute_paths(t: f64) -> Paths {
    let a = samp(&EDGES[0], t);
    let b = samp(&EDGES[1], t);
    let c = samp(&EDGES[2], t);
    let d = samp(&EDGES[3], t);
    let e = samp(&EDGES[4], t);
    Paths {
        bands: [
            band_top(&a, 2.0),
            band2(&a, &b),
            band2(&b, &c),
            band2(&c, &d),
            band2(&d, &e),
            band_top(&e, 46.0),
        ],
        edges: [edge_d(&a), edge_d(&b), edge_d(&c), edge_d(&d), edge_d(&e)],
    }
}

// ── Reveal scene math (ported 1:1 from mockups/wordmark-reveal-v22.html) ──

const SCENE_CX: f64 = 300.0;
const SCENE_CY: f64 = 148.0;
const SCENE_R: f64 = 76.0;
const LAND_Y: f64 = 552.0;
const L_FINAL: f64 = 157.0;

/// Full-bleed water extents (scene units). The scene svg overflows its
/// centered box (`.ocean-reveal { overflow: visible }`) so the water floods
/// the whole transcript pane — a glass filled up to the orb — and the pane
/// clips it. Wide/deep enough for any pane aspect around the 340px box.
const WATER_X0: f64 = -1800.0;
const WATER_X1: f64 = 2400.0;
const WATER_BOTTOM: f64 = 3000.0;
/// Waterline height at which the rising tide floods the Sessions launcher's
/// spot: the rAF stamps `data-flooded` on the landing container so CSS
/// surfaces the button in sync with the water — no hardcoded delay to
/// drift from these timeline constants.
const FLOOD_Y: f64 = 470.0;

/// Surface deviation from the mean waterline at x.
fn wave_dev(x: f64, amp: f64, ph: f64) -> f64 {
    (x * 0.035 + ph).sin() * amp + (x * 0.08 + ph * 1.6).sin() * amp * 0.4
}

/// Full waterline polyline (M…L…).  `jet_{a,w}` paint the Worthington gaussian.
fn wave_top(L: f64, amp: f64, ph: f64, jet_a: f64, jet_w: f64) -> String {
    let mut s = String::new();
    let mut x: f64 = WATER_X0;
    while x <= WATER_X1 + 0.01 {
        let mut y = L + wave_dev(x, amp, ph);
        if jet_a > 0.05 {
            let d2 = (x - SCENE_CX) * (x - SCENE_CX);
            y -= jet_a * (-d2 / (2.0 * jet_w * jet_w)).exp();
        }
        if s.is_empty() {
            s.push_str(&format!("M{x} {y:.1}"));
        } else {
            s.push_str(&format!("L{x} {y:.1}"));
        }
        x += 12.0;
    }
    s
}

/// Water body path (waterline → pane-bottom fill, full bleed).
fn body_d(L: f64, amp: f64, ph: f64, jet_a: f64, jet_w: f64) -> String {
    let mut s = wave_top(L, amp, ph, jet_a, jet_w);
    s.push_str(&format!(
        "L{WATER_X1} {WATER_BOTTOM} L{WATER_X0} {WATER_BOTTOM} Z"
    ));
    s
}

/// Pendant drop: bead → sag → thread neck → hanging bulb.
fn pendant_d(g: f64) -> String {
    let top: f64 = 221.0;
    let bulb_r = 3.2 + 5.8 * g;
    let drop = 2.0 + 34.0 * g * g;
    let bulb_cy = top + bulb_r * 0.4 + drop;
    let neck_w = (1.0_f64).max(bulb_r * (1.0 - 1.05 * g));
    let att_w = 6.5 - 1.5 * g;
    let neck_y = top + (bulb_cy - bulb_r - top) * 0.55;
    let f = |n: f64| format!("{n:.1}");
    format!(
        "M{} {top} C{} {} {} {} {} {} C{} {} {} {} {} {} A{} {} 0 0 0 {} {} C{} {} {} {} {} {} C{} {} {} {} {} {} Z",
        f(SCENE_CX - att_w),
        f(SCENE_CX - att_w), f(top + 3.0),
        f(SCENE_CX - neck_w), f(neck_y - 4.0),
        f(SCENE_CX - neck_w), f(neck_y),
        f(SCENE_CX - neck_w), f(neck_y + 3.0),
        f(SCENE_CX - bulb_r), f(bulb_cy - bulb_r * 0.85),
        f(SCENE_CX - bulb_r), f(bulb_cy),
        f(bulb_r), f(bulb_r),
        f(SCENE_CX + bulb_r), f(bulb_cy),
        f(SCENE_CX + bulb_r), f(bulb_cy - bulb_r * 0.85),
        f(SCENE_CX + neck_w), f(neck_y + 3.0),
        f(SCENE_CX + neck_w), f(neck_y),
        f(SCENE_CX + neck_w), f(neck_y - 4.0),
        f(SCENE_CX + att_w), f(top + 3.0),
        f(SCENE_CX + att_w), top,
    )
}

/// Post-pinch stub: shallow cap that recoils into the sphere.
fn stub_d(h: f64) -> String {
    let top: f64 = 221.0;
    let w = 6.2 * (0.55 + 0.45 * h);
    let dep = 9.5 * h;
    let f = |n: f64| format!("{n:.1}");
    format!(
        "M{} {top} C{} {} {} {} {} {} C{} {} {} {} {} {} Z",
        f(SCENE_CX - w),
        f(SCENE_CX - w * 0.85),
        f(top + dep * 0.75),
        f(SCENE_CX - w * 0.35),
        f(top + dep),
        f(SCENE_CX),
        f(top + dep),
        f(SCENE_CX + w * 0.35),
        f(top + dep),
        f(SCENE_CX + w * 0.85),
        f(top + dep * 0.75),
        f(SCENE_CX + w),
        top,
    )
}

// ── Orb inner markup for the reveal scene ──────────────────────────
// Shares the living-water math with WaveBadge (compute_paths, etc.) but
// generates its own defs + paths with `orb-` prefixed IDs for the nested
// 48×48 SVG.  All colors live in CSS classes (wave-badge__stop-*, wave-
// badge__split, wave-badge__foam, etc.) — zero hex in Rust.

fn orb_inner_html(uid: usize) -> String {
    let p = compute_paths(0.0);
    let band_y: [(f64, f64); 6] = [
        (5.0, 16.0),
        (15.0, 22.0),
        (21.0, 27.0),
        (26.0, 33.0),
        (32.0, 39.0),
        (36.0, 46.0),
    ];
    let mut s = String::new();
    s.push_str("<defs>");
    s.push_str(&format!(
        "<clipPath id='orb-coin-{uid}'><circle cx='24' cy='24' r='20.4'/></clipPath>"
    ));
    s.push_str(&format!(
        "<linearGradient id='orb-face-{uid}' gradientUnits='userSpaceOnUse' x1='0' y1='3' x2='0' y2='45'><stop offset='0' class='wave-badge__stop-face-hi'/><stop offset='0.55' class='wave-badge__stop-face'/></linearGradient>"
    ));
    for i in 0..6 {
        let n = i + 1;
        let (y1, y2) = band_y[i];
        s.push_str(&format!(
            "<linearGradient id='orb-b{n}-{uid}' gradientUnits='userSpaceOnUse' x1='0' y1='{y1}' x2='0' y2='{y2}'><stop offset='0' class='wave-badge__stop-b{n}-hi'/><stop offset='1' class='wave-badge__stop-b{n}-lo'/></linearGradient>"
        ));
    }
    s.push_str(&format!(
        "<radialGradient id='orb-vig-{uid}'><stop offset='0.62' class='wave-badge__stop-vig-in'/><stop offset='1' class='wave-badge__stop-vig-out'/></radialGradient>"
    ));
    s.push_str(&format!(
        "<radialGradient id='orb-gloss-{uid}' cx='0.5' cy='0.18' r='0.6'><stop offset='0' class='wave-badge__stop-gloss'/><stop offset='0.55' class='wave-badge__stop-gloss-out'/></radialGradient>"
    ));
    s.push_str(&format!(
        "<radialGradient id='orb-occl-{uid}'><stop offset='0.78' class='wave-badge__stop-occl-in'/><stop offset='0.94' class='wave-badge__stop-occl-mid'/><stop offset='1' class='wave-badge__stop-occl-out'/></radialGradient>"
    ));
    s.push_str(&format!(
        "<linearGradient id='orb-rim-{uid}' gradientUnits='userSpaceOnUse' x1='0' y1='3' x2='0' y2='45'><stop offset='0' class='wave-badge__stop-rim-hi'/><stop offset='1' class='wave-badge__stop-rim-lo'/></linearGradient>"
    ));
    s.push_str(&format!(
        "<filter id='orb-soft-{uid}' x='-20%' y='-20%' width='140%' height='140%'><feGaussianBlur stdDeviation='0.22'/></filter>"
    ));
    s.push_str(&format!(
        "<filter id='orb-crisp-{uid}' x='-20%' y='-20%' width='140%' height='140%'><feGaussianBlur stdDeviation='0.05'/></filter>"
    ));
    s.push_str(&format!(
        "<filter id='orb-spl-{uid}' x='-20%' y='-20%' width='140%' height='140%'><feGaussianBlur stdDeviation='0.13'/></filter>"
    ));
    s.push_str("</defs>");
    s.push_str(&format!(
        "<circle class='wave-badge__face' cx='24' cy='24' r='21' fill='url(#orb-face-{uid})'/>"
    ));
    s.push_str(&format!(
        "<circle class='wave-badge__vig' cx='24' cy='24' r='20.6' fill='url(#orb-vig-{uid})'/>"
    ));
    s.push_str(&format!(
        "<circle class='wave-badge__ring' cx='24' cy='24' r='20.7' fill='none' stroke='url(#orb-rim-{uid})' stroke-width='0.55'/>"
    ));
    s.push_str(&format!("<g clip-path='url(#orb-coin-{uid})'>"));
    for i in 0..6 {
        let n = i + 1;
        s.push_str(&format!(
            "<path class='wave-badge__band' fill='url(#orb-b{n}-{uid})' d='{}'/>",
            p.bands[i]
        ));
    }
    for i in 0..5 {
        let o = if i == 4 { "0.47" } else { "0.55" };
        s.push_str(&format!(
            "<path class='wave-badge__split' fill='none' stroke-width='0.30' stroke-opacity='{o}' filter='url(#orb-spl-{uid})' d='{}'/>",
            p.edges[i]
        ));
    }
    s.push_str(&format!(
        "<path class='wave-badge__foam wave-badge__foam-halo' fill='none' stroke-linecap='round' stroke-width='0.95' stroke-opacity='0.22' filter='url(#orb-soft-{uid})' d='{}'/>",
        p.edges[0]
    ));
    s.push_str(&format!(
        "<path class='wave-badge__foam wave-badge__foam-core' fill='none' stroke-linecap='round' stroke-width='0.30' stroke-opacity='0.85' filter='url(#orb-crisp-{uid})' d='{}'/>",
        p.edges[0]
    ));
    s.push_str(&format!(
        "<path class='wave-badge__foam wave-badge__foam-halo' fill='none' stroke-linecap='round' stroke-width='0.60' stroke-opacity='0.14' filter='url(#orb-soft-{uid})' d='{}'/>",
        p.edges[1]
    ));
    s.push_str(&format!(
        "<path class='wave-badge__foam wave-badge__foam-core' fill='none' stroke-linecap='round' stroke-width='0.22' stroke-opacity='0.50' filter='url(#orb-crisp-{uid})' d='{}'/>",
        p.edges[1]
    ));
    s.push_str(&format!(
        "<rect x='0' y='0' width='48' height='48' fill='url(#orb-gloss-{uid})'/>"
    ));
    s.push_str(&format!(
        "<circle class='wave-badge__occl' cx='24' cy='24' r='20.4' fill='url(#orb-occl-{uid})'/>"
    ));
    s.push_str("</g>");
    s
}

static UID: AtomicUsize = AtomicUsize::new(0);

/// Build one badge's SVG markup. Returns `(root_id, html)`. Every def id
/// carries the instance uid so mounted badges never collide; colors come
/// from `.wave-badge__*` classes (tokens.css), while stroke widths / blur
/// scale with `compact` (small mounts need bolder marks to survive the
/// downscale).
fn badge_svg_html(compact: bool) -> (String, String) {
    let uid = UID.fetch_add(1, Ordering::Relaxed);
    let root = format!("wb-{uid}");
    let (spl_w, spl_o, spf_o, core_w, core_o, halo_w, fb_w, fb_o, fbh_w, core_b, spl_b, soft_b) =
        if compact {
            (
                0.85, 0.62, 0.53, 0.62, 0.92, 1.15, 0.42, 0.58, 0.72, 0.03, 0.05, 0.22,
            )
        } else {
            (
                0.30, 0.56, 0.48, 0.30, 0.85, 0.95, 0.22, 0.50, 0.60, 0.05, 0.13, 0.22,
            )
        };
    let p = compute_paths(0.0);
    let band_y = [
        (5.0, 16.0),
        (15.0, 22.0),
        (21.0, 27.0),
        (26.0, 33.0),
        (32.0, 39.0),
        (36.0, 46.0),
    ];
    let cls = if compact {
        "wave-badge wave-badge--compact"
    } else {
        "wave-badge"
    };

    let mut s = String::new();
    s.push_str(&format!(
        "<svg id='{root}' class='{cls}' viewBox='0 0 48 48' width='1em' height='1em' fill='none' aria-hidden='true'>"
    ));
    s.push_str("<defs>");
    s.push_str(&format!(
        "<clipPath id='coin-{uid}'><circle cx='24' cy='24' r='20.4'/></clipPath>"
    ));
    s.push_str(&format!(
        "<linearGradient id='face-{uid}' gradientUnits='userSpaceOnUse' x1='0' y1='3' x2='0' y2='45'><stop offset='0' class='wave-badge__stop-face-hi'/><stop offset='0.55' class='wave-badge__stop-face'/></linearGradient>"
    ));
    for i in 0..6 {
        let n = i + 1;
        let (y1, y2) = band_y[i];
        s.push_str(&format!(
            "<linearGradient id='b{n}-{uid}' gradientUnits='userSpaceOnUse' x1='0' y1='{y1}' x2='0' y2='{y2}'><stop offset='0' class='wave-badge__stop-b{n}-hi'/><stop offset='1' class='wave-badge__stop-b{n}-lo'/></linearGradient>"
        ));
    }
    s.push_str(&format!(
        "<radialGradient id='vig-{uid}'><stop offset='0.62' class='wave-badge__stop-vig-in'/><stop offset='1' class='wave-badge__stop-vig-out'/></radialGradient>"
    ));
    s.push_str(&format!(
        "<radialGradient id='gloss-{uid}' cx='0.5' cy='0.18' r='0.6'><stop offset='0' class='wave-badge__stop-gloss'/><stop offset='0.55' class='wave-badge__stop-gloss-out'/></radialGradient>"
    ));
    s.push_str(&format!(
        "<radialGradient id='occl-{uid}'><stop offset='0.78' class='wave-badge__stop-occl-in'/><stop offset='0.94' class='wave-badge__stop-occl-mid'/><stop offset='1' class='wave-badge__stop-occl-out'/></radialGradient>"
    ));
    s.push_str(&format!(
        "<linearGradient id='rim-{uid}' gradientUnits='userSpaceOnUse' x1='0' y1='3' x2='0' y2='45'><stop offset='0' class='wave-badge__stop-rim-hi'/><stop offset='1' class='wave-badge__stop-rim-lo'/></linearGradient>"
    ));
    s.push_str(&format!(
        "<filter id='soft-{uid}' x='-20%' y='-20%' width='140%' height='140%'><feGaussianBlur stdDeviation='{soft_b}'/></filter>"
    ));
    s.push_str(&format!(
        "<filter id='crisp-{uid}' x='-20%' y='-20%' width='140%' height='140%'><feGaussianBlur stdDeviation='{core_b}'/></filter>"
    ));
    s.push_str(&format!(
        "<filter id='spl-{uid}' x='-20%' y='-20%' width='140%' height='140%'><feGaussianBlur stdDeviation='{spl_b}'/></filter>"
    ));
    s.push_str("</defs>");
    s.push_str(&format!(
        "<circle class='wave-badge__face' cx='24' cy='24' r='21' fill='url(#face-{uid})'/>"
    ));
    s.push_str(&format!(
        "<circle class='wave-badge__vig' cx='24' cy='24' r='20.6' fill='url(#vig-{uid})'/>"
    ));
    s.push_str(&format!(
        "<circle class='wave-badge__ring' cx='24' cy='24' r='20.7' fill='none' stroke='url(#rim-{uid})' stroke-width='0.55'/>"
    ));
    s.push_str(&format!("<g clip-path='url(#coin-{uid})'>"));
    for i in 0..6 {
        let n = i + 1;
        s.push_str(&format!(
            "<path class='wave-badge__band' fill='url(#b{n}-{uid})' d='{d}'/>",
            d = p.bands[i]
        ));
    }
    for i in 0..5 {
        let o = if i == 4 { spf_o } else { spl_o };
        s.push_str(&format!(
            "<path class='wave-badge__split' fill='none' stroke-width='{spl_w}' stroke-opacity='{o}' filter='url(#spl-{uid})' d='{d}'/>",
            d = p.edges[i]
        ));
    }
    s.push_str(&format!(
        "<path class='wave-badge__foam wave-badge__foam-halo' fill='none' stroke-linecap='round' stroke-width='{halo_w}' stroke-opacity='0.22' filter='url(#soft-{uid})' d='{d}'/>",
        d = p.edges[0]
    ));
    s.push_str(&format!(
        "<path class='wave-badge__foam wave-badge__foam-core' fill='none' stroke-linecap='round' stroke-width='{core_w}' stroke-opacity='{core_o}' filter='url(#crisp-{uid})' d='{d}'/>",
        d = p.edges[0]
    ));
    s.push_str(&format!(
        "<path class='wave-badge__foam wave-badge__foam-halo' fill='none' stroke-linecap='round' stroke-width='{fbh_w}' stroke-opacity='0.14' filter='url(#soft-{uid})' d='{d}'/>",
        d = p.edges[1]
    ));
    s.push_str(&format!(
        "<path class='wave-badge__foam wave-badge__foam-core' fill='none' stroke-linecap='round' stroke-width='{fb_w}' stroke-opacity='{fb_o}' filter='url(#crisp-{uid})' d='{d}'/>",
        d = p.edges[1]
    ));
    s.push_str(&format!(
        "<rect x='0' y='0' width='48' height='48' fill='url(#gloss-{uid})'/>"
    ));
    s.push_str(&format!(
        "<circle class='wave-badge__occl' cx='24' cy='24' r='20.4' fill='url(#occl-{uid})'/>"
    ));
    s.push_str("</g></svg>");
    (root, s)
}

fn collect(root: &web_sys::Element, sel: &str) -> Vec<web_sys::Element> {
    let mut v = Vec::new();
    if let Ok(list) = root.query_selector_all(sel) {
        for i in 0..list.length() {
            if let Some(node) = list.item(i) {
                if let Ok(el) = node.dyn_into::<web_sys::Element>() {
                    v.push(el);
                }
            }
        }
    }
    v
}

/// The Ocean badge — a circular emblem of six living-water depth strata
/// (sunlit aqua surface → indigo abyss) with dark longitudinal splits and a
/// soft whitecap glint. `spinning=true` flows the strata as traveling waves
/// via a requestAnimationFrame loop (paused under `prefers-reduced-motion`;
/// the frame is cancelled on unmount). `compact` bolds strokes + tightens
/// blur for small mounts (header, pending) so the mark stays crisp when
/// scaled down. Colors live in tokens.css (`--badge-*`); geometry is here.
#[component]
pub fn WaveBadge(
    #[prop(optional)] spinning: bool,
    #[prop(optional)] compact: bool,
) -> impl IntoView {
    let (root_id, html) = badge_svg_html(compact);

    if spinning {
        // Paused under prefers-reduced-motion (the static t=0 frame already
        // renders). match_media needs no DOM node, so check it up front.
        let reduce = web_sys::window()
            .and_then(|w| {
                w.match_media("(prefers-reduced-motion: reduce)")
                    .ok()
                    .flatten()
            })
            .map(|m| m.matches())
            .unwrap_or(false);
        if !reduce {
            // A self-rescheduling rAF loop. The closure holds itself via
            // `holder` and reschedules each frame, lazily resolving the path
            // elements once the SVG is mounted. `on_cleanup` cancels the
            // pending frame and drops the closure at unmount — from OUTSIDE a
            // running frame, so we never free the executing closure. The Rc
            // state is `!Send`; SendWrapper satisfies on_cleanup's Send+Sync
            // bound (sound on wasm's single thread).
            let raf_id: Rc<RefCell<i32>> = Rc::new(RefCell::new(0));
            let holder: Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>> = Rc::new(RefCell::new(None));
            let start: Rc<RefCell<Option<f64>>> = Rc::new(RefCell::new(None));
            #[allow(clippy::type_complexity)]
            let cache: Rc<
                RefCell<
                    Option<(
                        Vec<web_sys::Element>,
                        Vec<web_sys::Element>,
                        Vec<web_sys::Element>,
                    )>,
                >,
            > = Rc::new(RefCell::new(None));
            let holder_inner = holder.clone();
            let raf_inner = raf_id.clone();
            let root_for_cb = root_id.clone();
            let cb = Closure::wrap(Box::new(move |ts: f64| {
                let Some(win) = web_sys::window() else { return };
                let Some(doc) = win.document() else { return };
                // Unmounted (or not yet mounted): skip this frame's work.
                let Some(root) = doc.get_element_by_id(&root_for_cb) else {
                    return;
                };
                let need = cache.borrow().is_none();
                if need {
                    let bands = collect(&root, ".wave-badge__band");
                    let splits = collect(&root, ".wave-badge__split");
                    let foam = collect(&root, ".wave-badge__foam");
                    if bands.len() >= 6 && splits.len() >= 5 && foam.len() >= 4 {
                        *cache.borrow_mut() = Some((bands, splits, foam));
                    }
                }
                if let Some((bands, splits, foam)) = cache.borrow().as_ref() {
                    let t0 = {
                        let mut b = start.borrow_mut();
                        if b.is_none() {
                            *b = Some(ts);
                        }
                        b.unwrap()
                    };
                    let t = (ts - t0) / 1000.0 * 0.6;
                    let p = compute_paths(t);
                    for (el, d) in bands.iter().zip(p.bands.iter()) {
                        let _ = el.set_attribute("d", d);
                    }
                    for (el, d) in splits.iter().zip(p.edges.iter()) {
                        let _ = el.set_attribute("d", d);
                    }
                    let fd = [&p.edges[0], &p.edges[0], &p.edges[1], &p.edges[1]];
                    for (el, d) in foam.iter().zip(fd.iter()) {
                        let _ = el.set_attribute("d", d.as_str());
                    }
                }
                if let Some(c) = holder_inner.borrow().as_ref() {
                    if let Ok(id) = win.request_animation_frame(c.as_ref().unchecked_ref()) {
                        *raf_inner.borrow_mut() = id;
                    }
                }
            }) as Box<dyn FnMut(f64)>);
            if let Some(win) = web_sys::window() {
                if let Ok(id) = win.request_animation_frame(cb.as_ref().unchecked_ref()) {
                    *raf_id.borrow_mut() = id;
                }
            }
            *holder.borrow_mut() = Some(cb);
            let cleanup = send_wrapper::SendWrapper::new((raf_id, holder));
            on_cleanup(move || {
                let (raf_id, holder) = &*cleanup;
                if let Some(w) = web_sys::window() {
                    let _ = w.cancel_animation_frame(*raf_id.borrow());
                }
                holder.borrow_mut().take();
            });
        }
    }

    view! { <span style="display:contents" inner_html=html></span> }
}

/// The OCEAN wordmark — five ramped letter spans (O C E A N) using the
/// depth ramp (ocean-4 through ocean-8). Bare spans, no classes — the
/// parent wrapper owns font / size / spacing via nth-child selectors
/// (chrome.css for the header, transcript.css for the landing).
/// Set `lower` for the lowercase "ocean" variant.
#[component]
pub fn OceanWordmark(#[prop(optional)] lower: bool) -> impl IntoView {
    let letters = if lower {
        ['o', 'c', 'e', 'a', 'n']
    } else {
        ['O', 'C', 'E', 'A', 'N']
    };
    view! {
        <span>{letters[0].to_string()}</span>
        <span>{letters[1].to_string()}</span>
        <span>{letters[2].to_string()}</span>
        <span>{letters[3].to_string()}</span>
        <span>{letters[4].to_string()}</span>
    }
}

/// Menu / sessions toggle — three stacked lines (replaces "☰"). Rounded 2px
/// stroke to match the wave-badge + header-icon family. (OCEAN-202)
#[component]
pub fn Menu() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <line x1="3" y1="6" x2="21" y2="6" />
            <line x1="3" y1="12" x2="21" y2="12" />
            <line x1="3" y1="18" x2="21" y2="18" />
        </svg>
    }
}

/// Council deck — a classical pillared building (replaces "🏛"). (OCEAN-202)
#[component]
pub fn Council() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M3 9l9-5 9 5" />
            <line x1="3" y1="9" x2="21" y2="9" />
            <line x1="5" y1="9" x2="5" y2="18" />
            <line x1="10" y1="9" x2="10" y2="18" />
            <line x1="14" y1="9" x2="14" y2="18" />
            <line x1="19" y1="9" x2="19" y2="18" />
            <line x1="3" y1="21" x2="21" y2="21" />
        </svg>
    }
}

/// Rooms / collaboration spaces — two people (replaces "👥"). (OCEAN-202)
#[component]
pub fn Groups() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M16 19v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" />
            <circle cx="9" cy="7" r="4" />
            <path d="M22 19v-2a4 4 0 0 0-3-3.87" />
            <path d="M16 3.13a4 4 0 0 1 0 7.75" />
        </svg>
    }
}

/// Capture visible tab — a camera (replaces "📷"). (OCEAN-202)
#[component]
pub fn Capture() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M23 19a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4l2-3h6l2 3h4a2 2 0 0 1 2 2z" />
            <circle cx="12" cy="13" r="4" />
        </svg>
    }
}

/// delapouite/sound-on — TTS unmuted.
#[component]
pub fn SoundOn() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 512 512" width="1em" height="1em"
             fill="currentColor" aria-hidden="true">
            <path d="M256 32C132.3 32 32 132.3 32 256s100.3 224 224 224 224-100.3 224-224S379.7 32 256 32zm-30 99v250l-95-75H66V206h65l95-75zm99.7 14.7c34.3 22.6 57 61.4 57 105.6s-22.7 83-57 105.6l-13.4-20.3c28-18.5 46.4-50.2 46.4-85.3s-18.4-66.8-46.4-85.3l13.4-20.3zM294 184.6c20.4 13 34 35.8 34 61.7s-13.6 48.7-34 61.7l-13.6-20.7c14-9 23.3-24.6 23.3-41.7s-9.3-32.7-23.3-41.7L294 184.6z"/>
        </svg>
    }
}

/// delapouite/sound-off — TTS muted.
#[component]
pub fn SoundOff() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 512 512" width="1em" height="1em"
             fill="currentColor" aria-hidden="true">
            <path d="M256 32C132.3 32 32 132.3 32 256s100.3 224 224 224 224-100.3 224-224S379.7 32 256 32zm-30 99v250l-95-75H66V206h65l95-75zm143.4 36.5l20.1 20.1L344.1 256l45.4 45.4-20.1 20.1L324 276.1l-45.4 45.4-20.1-20.1L303.9 256l-45.4-45.4 20.1-20.1L324 235.9l45.4-45.4z"/>
        </svg>
    }
}

/// Outbound call — a handset (replaces "📞"). Rounded 2px stroke to match the
/// wave-badge + header-icon family. Drives the place-call trigger (OCEAN-261).
#[component]
pub fn Phone() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M22 16.92v3a2 2 0 0 1-2.18 2 19.79 19.79 0 0 1-8.63-3.07 19.5 19.5 0 0 1-6-6 19.79 19.79 0 0 1-3.07-8.67A2 2 0 0 1 4.11 2h3a2 2 0 0 1 2 1.72c.13.96.36 1.9.7 2.81a2 2 0 0 1-.45 2.11L8.09 9.91a16 16 0 0 0 6 6l1.27-1.27a2 2 0 0 1 2.11-.45c.91.34 1.85.57 2.81.7A2 2 0 0 1 22 16.92z" />
        </svg>
    }
}

/// Microphone — voice input. Studio-capsule mark: grille lines inside the
/// capsule so it reads "mic" (not "pill") at orb size. Rounded 2px stroke
/// to match the header-icon family.
#[component]
pub fn Mic() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <rect x="9" y="2" width="6" height="12" rx="3" />
            <line x1="9" y1="6" x2="15" y2="6" />
            <line x1="9" y1="9" x2="15" y2="9" />
            <path d="M5 11a7 7 0 0 0 14 0" />
            <line x1="12" y1="18" x2="12" y2="22" />
        </svg>
    }
}

/// Project folder — the sessions panel's project mark.
#[component]
pub fn Folder() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
        </svg>
    }
}

/// Git branch — trunk, fork arc, and branch head. Rendered wherever a live
/// branch name appears (session rows, worktree groups).
#[component]
pub fn GitBranch() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <circle cx="6" cy="5" r="2.4" />
            <circle cx="6" cy="19" r="2.4" />
            <circle cx="18" cy="5" r="2.4" />
            <line x1="6" y1="7.4" x2="6" y2="16.6" />
            <path d="M18 7.4a9 9 0 0 1-9 9" />
        </svg>
    }
}

/// Terminal — the TUI surface origin.
#[component]
pub fn Terminal() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <polyline points="4 17 10 11 4 5" />
            <line x1="13" y1="19" x2="20" y2="19" />
        </svg>
    }
}

/// Globe — the web surface origin.
#[component]
pub fn Globe() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <circle cx="12" cy="12" r="9" />
            <line x1="3" y1="12" x2="21" y2="12" />
            <path d="M12 3a14.5 14.5 0 0 1 0 18a14.5 14.5 0 0 1 0-18" />
        </svg>
    }
}

/// Monitor — the native desktop surface origin.
#[component]
pub fn Desktop() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <rect x="2" y="4" width="20" height="13" rx="2" />
            <line x1="8" y1="21" x2="16" y2="21" />
            <line x1="12" y1="17" x2="12" y2="21" />
        </svg>
    }
}

/// Slack — the one brand mark in the set, so it keeps its official fill
/// geometry (Simple Icons path, CC0) painted in currentColor like every
/// other icon here. Design doc: "real logos" are part of the iconography.
#[component]
pub fn Slack() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 24 24" width="1em" height="1em"
             fill="currentColor" aria-hidden="true">
            <path d="M5.042 15.165a2.528 2.528 0 0 1-2.52 2.523A2.528 2.528 0 0 1 0 15.165a2.527 2.527 0 0 1 2.522-2.52h2.52v2.52zM6.313 15.165a2.527 2.527 0 0 1 2.521-2.52 2.527 2.527 0 0 1 2.521 2.52v6.313A2.528 2.528 0 0 1 8.834 24a2.528 2.528 0 0 1-2.521-2.522v-6.313zM8.834 5.042a2.528 2.528 0 0 1-2.521-2.52A2.528 2.528 0 0 1 8.834 0a2.528 2.528 0 0 1 2.521 2.522v2.52H8.834zM8.834 6.313a2.528 2.528 0 0 1 2.521 2.521 2.528 2.528 0 0 1-2.521 2.521H2.522A2.528 2.528 0 0 1 0 8.834a2.528 2.528 0 0 1 2.522-2.521h6.312zM18.956 8.834a2.528 2.528 0 0 1 2.522-2.521A2.528 2.528 0 0 1 24 8.834a2.528 2.528 0 0 1-2.522 2.521h-2.522V8.834zM17.688 8.834a2.528 2.528 0 0 1-2.523 2.521 2.527 2.527 0 0 1-2.52-2.521V2.522A2.527 2.527 0 0 1 15.165 0a2.528 2.528 0 0 1 2.523 2.522v6.312zM15.165 18.956a2.528 2.528 0 0 1 2.523 2.522A2.528 2.528 0 0 1 15.165 24a2.527 2.527 0 0 1-2.52-2.522v-2.522h2.52zM15.165 17.688a2.527 2.527 0 0 1-2.52-2.523 2.526 2.526 0 0 1 2.52-2.52h6.313A2.527 2.527 0 0 1 24 15.165a2.528 2.528 0 0 1-2.522 2.523h-6.313z" />
        </svg>
    }
}

/// Code chevrons — the ACP editor surface (Zed / Cursor).
#[component]
pub fn Code() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <polyline points="16 18 22 12 16 6" />
            <polyline points="8 6 2 12 8 18" />
        </svg>
    }
}

/// Puzzle piece — the browser-extension surface.
#[component]
pub fn Puzzle() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M9 4a2 2 0 1 1 4 0h4a1 1 0 0 1 1 1v4a2 2 0 1 1 0 4v4a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1v-4a2 2 0 1 1 0-4V5a1 1 0 0 1 1-1z" />
        </svg>
    }
}

/// Smartphone — the mobile surface.
#[component]
pub fn Smartphone() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <rect x="6" y="2" width="12" height="20" rx="2" />
            <line x1="12" y1="18" x2="12" y2="18" />
        </svg>
    }
}

/// Microphone disabled — the voice kill switch. Same studio capsule with a
/// hard diagonal slash; the orb's Off state and the mode menu's Off row.
#[component]
pub fn MicOff() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <rect x="9" y="2" width="6" height="12" rx="3" />
            <path d="M5 11a7 7 0 0 0 14 0" />
            <line x1="12" y1="18" x2="12" y2="22" />
            <line x1="4" y1="4" x2="20" y2="20" />
        </svg>
    }
}

/// Open-mic field — hands-free mode. A source dot between spreading arcs:
/// sound arriving from the whole room, not a held capsule.
#[component]
pub fn Waves() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <circle cx="12" cy="12" r="2" />
            <path d="M7.76 16.24a6 6 0 0 1 0-8.48" />
            <path d="M16.24 7.76a6 6 0 0 1 0 8.48" />
            <path d="M4.93 19.07a10 10 0 0 1 0-14.14" />
            <path d="M19.07 4.93a10 10 0 0 1 0 14.14" />
        </svg>
    }
}

/// Chevron — the voice-menu trigger.
#[component]
pub fn ChevronDown() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <polyline points="6 9 12 15 18 9" />
        </svg>
    }
}

/// The Ocean reveal — the landing scene. A 600×600 SVG of the OCEAN wordmark
/// being revealed by a rising waterline; a living-water orb rides the surface;
/// a pendant drop forms, pinches off, falls, and splashes. The full timeline
/// plays once then idles forever with the water rippling and the orb bobbing.
/// Pure Rust rAF loop (no GSAP); all colors live in CSS tokens.
/// Ported from the approved `mockups/wordmark-reveal-v22.html`.
#[component]
pub fn OceanReveal() -> impl IntoView {
    let uid = UID.fetch_add(1, Ordering::Relaxed);

    // Reduced-motion: render the settled frame and skip the rAF timeline.
    let reduce = web_sys::window()
        .and_then(|w| {
            w.match_media("(prefers-reduced-motion: reduce)")
                .ok()
                .flatten()
        })
        .map(|m| m.matches())
        .unwrap_or(false);

    // Per-letter solid depth ramp (ocean-4 → ocean-8). Never gradient-clip text.
    let ocean_fills = [
        "var(--ocean-4)",
        "var(--ocean-5)",
        "var(--ocean-6)",
        "var(--ocean-7)",
        "var(--ocean-8)",
    ];

    let orb_markup = orb_inner_html(uid);
    let (body_opacity, crest_opacity, pendant_opacity) = if reduce {
        ("1", "0.9", "0")
    } else {
        ("0", "0", "0")
    };
    let (l_init, amp_init) = if reduce { (L_FINAL, 2.0) } else { (620.0, 0.0) };
    let crest_d_init = wave_top(l_init, amp_init, 0.0, 0.0, 12.0);
    let body_d_init = body_d(l_init, amp_init, 0.0, 0.0, 12.0);
    let pendant_d_init = pendant_d(0.0);

    let html = format!(
        "<svg class='ocean-reveal' viewBox='0 0 600 600' fill='none' aria-hidden='true'>\
        <defs>\
        <clipPath id='rv-tide-{uid}'><path id='rv-tideP-{uid}' d='{body_d_init}'/></clipPath>\
        <clipPath id='rv-ballClip-{uid}'><circle id='rv-ballC-{uid}' cx='{SCENE_CX}' cy='{SCENE_CY}' r='{SCENE_R}'/></clipPath>\
        <linearGradient id='rv-waterBody-{uid}' x1='0' y1='150' x2='0' y2='600' gradientUnits='userSpaceOnUse'>\
        <stop offset='0' class='ocean-reveal__stop-water-hi' stop-opacity='0.40'/>\
        <stop offset='0.5' class='ocean-reveal__stop-water-mid' stop-opacity='0.50'/>\
        <stop offset='1' class='ocean-reveal__stop-water-lo' stop-opacity='0.64'/>\
        </linearGradient>\
        <linearGradient id='rv-dripRamp-{uid}' x1='0' y1='0' x2='0' y2='1'>\
        <stop offset='0' class='ocean-reveal__stop-drip-hi'/>\
        <stop offset='0.45' class='ocean-reveal__stop-drip-mid'/>\
        <stop offset='1' class='ocean-reveal__stop-drip-lo'/>\
        </linearGradient>\
        <filter id='rv-foamGlow-{uid}' x='-40%' y='-40%' width='180%' height='180%'><feGaussianBlur stdDeviation='2'/></filter>\
        </defs>\
        <path id='rv-body-{uid}' fill='url(#rv-waterBody-{uid})' opacity='{body_opacity}' d='{body_d_init}'/>\
        <g id='rv-wordmark-{uid}' clip-path='url(#rv-tide-{uid})' font-family='var(--wm-font),Poppins,system-ui' font-weight='700' font-size='88' text-anchor='middle'>\
        <text class='ocean-reveal__letter ocean-reveal__letter--1' x='100' y='330' fill='{o0}'>O</text>\
        <text class='ocean-reveal__letter ocean-reveal__letter--2' x='200' y='330' fill='{o1}'>C</text>\
        <text class='ocean-reveal__letter ocean-reveal__letter--3' x='300' y='330' fill='{o2}'>E</text>\
        <text class='ocean-reveal__letter ocean-reveal__letter--4' x='400' y='330' fill='{o3}'>A</text>\
        <text class='ocean-reveal__letter ocean-reveal__letter--5' x='500' y='330' fill='{o4}'>N</text>\
        </g>\
        <g id='rv-orbG-{uid}'><svg id='rv-orb-{uid}' x='212' y='60' width='176' height='176' viewBox='0 0 48 48' fill='none'>{orb_markup}</svg></g>\
        <g clip-path='url(#rv-ballClip-{uid})'><path id='rv-subCap-{uid}' fill='var(--water-tint)' fill-opacity='0.55' d='{body_d_init}'/></g>\
        <g>\
        <path id='rv-crestGlow-{uid}' fill='none' stroke='var(--water-crest-soft)' stroke-width='4.5' stroke-linecap='round' stroke-opacity='0.3' filter='url(#rv-foamGlow-{uid})' opacity='{crest_opacity}' d='{crest_d_init}'/>\
        <path id='rv-crest-{uid}' fill='none' stroke='var(--water-crest)' stroke-width='2.3' stroke-linecap='round' stroke-opacity='0.8' opacity='{crest_opacity}' d='{crest_d_init}'/>\
        </g>\
        <path id='rv-pendant-{uid}' fill='url(#rv-dripRamp-{uid})' stroke='#0a1f3a' stroke-width='0.6' stroke-opacity='0.4' opacity='{pendant_opacity}' d='{pendant_d_init}'/>\
        <ellipse id='rv-fdrop-{uid}' fill='url(#rv-dripRamp-{uid})' stroke='#0a1f3a' stroke-width='0.5' stroke-opacity='0.35' opacity='0'/>\
        <circle id='rv-sat-{uid}' fill='url(#rv-dripRamp-{uid})' opacity='0'/>\
        </svg>",
        o0 = ocean_fills[0],
        o1 = ocean_fills[1],
        o2 = ocean_fills[2],
        o3 = ocean_fills[3],
        o4 = ocean_fills[4],
        orb_markup = orb_markup,
        body_opacity = body_opacity,
        crest_opacity = crest_opacity,
        pendant_opacity = pendant_opacity,
    );

    if !reduce {
        #[derive(Clone)]
        struct State {
            l: f64,
            amp: f64,
            ph: f64,
            jet_a: f64,
            jet_w: f64,
            g: f64,
            drop_t0: Option<f64>,
            drop_r: f64,
            bob_start: Option<f64>,
            last_t: f64,
            flooded: bool,
        }

        let state: Rc<RefCell<State>> = Rc::new(RefCell::new(State {
            l: 620.0,
            amp: 0.0,
            ph: 0.0,
            jet_a: 0.0,
            jet_w: 12.0,
            g: 0.0,
            drop_t0: None,
            drop_r: 7.0,
            bob_start: None,
            last_t: 0.0,
            flooded: false,
        }));

        let raf_id: Rc<RefCell<i32>> = Rc::new(RefCell::new(0));
        let holder: Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>> = Rc::new(RefCell::new(None));
        let start: Rc<RefCell<Option<f64>>> = Rc::new(RefCell::new(None));

        #[allow(clippy::type_complexity)]
        let cache: Rc<
            RefCell<
                Option<(
                    web_sys::Element,
                    web_sys::Element,
                    web_sys::Element,
                    web_sys::Element,
                    web_sys::Element,
                    web_sys::Element,
                    web_sys::Element,
                    web_sys::Element,
                    web_sys::Element,
                    web_sys::Element,
                )>,
            >,
        > = Rc::new(RefCell::new(None));

        let root_id = format!("rv-orb-{uid}");
        let body_id = format!("rv-body-{uid}");
        let tidep_id = format!("rv-tideP-{uid}");
        let crest_id = format!("rv-crest-{uid}");
        let crest_g_id = format!("rv-crestGlow-{uid}");
        let ballc_id = format!("rv-ballC-{uid}");
        let subcap_id = format!("rv-subCap-{uid}");
        let orbg_id = format!("rv-orbG-{uid}");
        let pendant_id = format!("rv-pendant-{uid}");
        let fdrop_id = format!("rv-fdrop-{uid}");
        let sat_id = format!("rv-sat-{uid}");
        let holder_inner = holder.clone();
        let raf_inner = raf_id.clone();
        let state_clone = state.clone();
        let start_clone = start.clone();

        let cb = Closure::wrap(Box::new(move |ts: f64| {
            let Some(win) = web_sys::window() else { return };
            let Some(doc) = win.document() else { return };
            let Some(_root) = doc.get_element_by_id(&root_id) else {
                return;
            };

            let need = cache.borrow().is_none();
            if need {
                if let (
                    Some(body),
                    Some(tidep),
                    Some(crest),
                    Some(crest_g),
                    Some(ballc),
                    Some(subcap),
                    Some(orbg),
                    Some(pendant),
                    Some(fdrop),
                    Some(sat),
                ) = (
                    doc.get_element_by_id(&body_id),
                    doc.get_element_by_id(&tidep_id),
                    doc.get_element_by_id(&crest_id),
                    doc.get_element_by_id(&crest_g_id),
                    doc.get_element_by_id(&ballc_id),
                    doc.get_element_by_id(&subcap_id),
                    doc.get_element_by_id(&orbg_id),
                    doc.get_element_by_id(&pendant_id),
                    doc.get_element_by_id(&fdrop_id),
                    doc.get_element_by_id(&sat_id),
                ) {
                    *cache.borrow_mut() = Some((
                        body, tidep, crest, crest_g, ballc, subcap, orbg, pendant, fdrop, sat,
                    ));
                }
            }

            if let Some((body, tidep, crest, crest_g, ballc, subcap, orbg, pendant, fdrop, sat)) =
                cache.borrow().as_ref()
            {
                let t0 = {
                    let mut b = start_clone.borrow_mut();
                    if b.is_none() {
                        *b = Some(ts);
                    }
                    b.unwrap()
                };
                let t = (ts - t0) / 1000.0;
                let mut st = state_clone.borrow_mut();

                // v22 labels: pinch=1.6, impact=pinch+0.6=2.2
                let pinch = 1.6;
                let impact = pinch + 0.6;
                let rise_start = impact + 0.35;
                let bob_at = rise_start + 1.45;

                // 1. Formation (0 → pinch)
                if t < pinch {
                    let tau = t / 1.6;
                    st.g = tau * tau * tau * 0.96; // power2.in
                    let _ = pendant.set_attribute("d", &pendant_d(st.g));
                    if t < 0.25 {
                        let _ = pendant.set_attribute("opacity", &format!("{:.3}", t / 0.25));
                    } else {
                        let _ = pendant.set_attribute("opacity", "1");
                    }
                }

                // 2. Pinch
                if t >= pinch {
                    let pt = t - pinch;

                    if pt < 0.5 {
                        let tau = pt / 0.5;
                        let h = 1.0 - (1.0 - tau).powi(3); // power2.out
                        let _ = pendant.set_attribute("d", &stub_d(h));
                    }

                    if pt >= 0.36 && pt < 0.48 {
                        let fade_tau = (pt - 0.36) / 0.12;
                        let _ = pendant.set_attribute("opacity", &format!("{:.3}", 1.0 - fade_tau));
                    } else if pt >= 0.48 {
                        let _ = pendant.set_attribute("opacity", "0");
                    }

                    if pt < 0.09 {
                        let _ = fdrop.set_attribute("opacity", &format!("{:.3}", pt / 0.09));
                    } else if pt < impact - pinch + 0.16 {
                        let _ = fdrop.set_attribute("opacity", "1");
                    } else {
                        let _ = fdrop.set_attribute("opacity", "0");
                    }

                    {
                        let g_end = 0.96;
                        let bulb_r = 3.2 + 5.8 * g_end;
                        let bulb_cy = 221.0 + bulb_r * 0.4 + 2.0 + 34.0 * g_end * g_end;
                        if pt < 0.6 {
                            let tau = pt / 0.6;
                            let cy = bulb_cy + (LAND_Y - 4.0 - bulb_cy) * tau.powi(3); // power2.in
                            let _ = fdrop.set_attribute("cy", &format!("{cy:.1}"));
                        }
                        if pt < 0.09 {
                            let _ = fdrop.set_attribute("cx", &format!("{SCENE_CX}"));
                        }
                    }

                    if st.drop_t0.is_none() {
                        st.drop_t0 = Some(ts);
                        st.drop_r = (3.2 + 5.8 * 0.96) * 0.94;
                    }

                    if pt < 0.09 {
                        let _ = sat.set_attribute("opacity", &format!("{:.3}", pt / 0.09));
                    } else if pt < impact - pinch + 0.13 {
                        let _ = sat.set_attribute("opacity", "1");
                    } else {
                        let _ = sat.set_attribute("opacity", "0");
                    }

                    {
                        let sat_fall_start = 0.05;
                        if pt >= sat_fall_start && pt < sat_fall_start + 0.68 {
                            let tau = (pt - sat_fall_start) / 0.68;
                            let cy = 235.0 + (LAND_Y - 8.0 - 235.0) * tau.powi(3);
                            let _ = sat.set_attribute("cy", &format!("{cy:.1}"));
                        }
                        if pt < sat_fall_start + 0.09 {
                            let _ = sat.set_attribute("cx", &format!("{SCENE_CX}"));
                            let _ = sat.set_attribute("r", "2.3");
                        }
                    }

                    if let Some(dt0) = st.drop_t0 {
                        let td = (ts - dt0) / 1000.0;
                        if td < (impact - pinch) + 0.16 {
                            let osc = 0.34
                                * (-2.6 * td).exp()
                                * (2.0 * std::f64::consts::PI * 4.2 * td).cos();
                            let _ = fdrop.set_attribute(
                                "rx",
                                &format!("{:.2}", st.drop_r * (1.0 - 0.9 * osc)),
                            );
                            let _ = fdrop
                                .set_attribute("ry", &format!("{:.2}", st.drop_r * (1.0 + osc)));
                        }
                    }
                }

                // 3. Film arrives at impact−0.04 (before the impact label).
                if t >= impact - 0.04 {
                    let it = t - impact; // negative until impact

                    if it < 0.10 {
                        let tau = ((it + 0.04) / 0.14).clamp(0.0, 1.0);
                        let ease = 1.0 - (1.0 - tau) * (1.0 - tau); // power1.out
                        let _ = body.set_attribute("opacity", &format!("{:.3}", ease));
                        let _ = crest.set_attribute("opacity", &format!("{:.3}", ease * 0.9));
                        let _ = crest_g.set_attribute("opacity", &format!("{:.3}", ease * 0.9));
                    } else {
                        let _ = body.set_attribute("opacity", "1");
                        let _ = crest.set_attribute("opacity", "0.9");
                        let _ = crest_g.set_attribute("opacity", "0.9");
                    }

                    st.l = LAND_Y;
                    st.amp = 7.0;
                }

                // 4. Impact splash + tide rise
                if t >= impact {
                    let it = t - impact;

                    if it < 0.13 {
                        let tau = it / 0.13;
                        st.jet_a = 24.0 * (1.0 - (1.0 - tau).powi(4)); // power3.out
                    } else if it < 0.63 {
                        let tau = (it - 0.13) / 0.5;
                        st.jet_a = 24.0 * (1.0 - tau.powi(3));
                        st.jet_w = 11.0 + (30.0 - 11.0) * tau.powi(3);
                    } else {
                        st.jet_a = 0.0;
                    }

                    // Drop sinks half-submerged through crest (fade 0.16 at impact)
                    if it < 0.16 {
                        let _ = fdrop.set_attribute("opacity", &format!("{:.3}", 1.0 - it / 0.16));
                    }

                    // Secondary plink
                    if it >= 0.13 && it < 0.48 {
                        let tau = (it - 0.13) / 0.35;
                        let plink = 6.0 * (1.0 - tau.powi(3));
                        st.jet_a = st.jet_a.max(plink);
                    }

                    // Tide rises: L 552→157, amp 7→2 over 2.1s from rise_start
                    if it >= 0.35 {
                        let rt = it - 0.35;
                        if rt < 2.1 {
                            let tau = rt / 2.1;
                            let ease_l = 1.0 - (1.0 - tau).powi(3); // power2.out
                            let ease_a = 1.0 - (1.0 - tau) * (1.0 - tau); // power1.out
                            st.l = LAND_Y + (L_FINAL - LAND_Y) * ease_l;
                            st.amp = 7.0 + (2.0 - 7.0) * ease_a;
                        } else {
                            st.l = L_FINAL;
                            st.amp = 2.0;
                        }
                    }

                    if it >= bob_at - impact && st.bob_start.is_none() {
                        st.bob_start = Some(ts);
                    }
                }

                // Flood signal: once the rising tide passes the Sessions
                // launcher's spot, stamp the landing container so CSS can
                // surface the button in sync with the water.
                if !st.flooded && t >= impact && st.l <= FLOOD_Y {
                    st.flooded = true;
                    if let Ok(Some(container)) = body.closest(".transcript__landing-reveal") {
                        let _ = container.set_attribute("data-flooded", "1");
                    }
                }

                // Per-frame waterline + bob
                let dt = (ts - st.last_t).max(0.0).min(50.0) / 1000.0;
                st.last_t = ts;
                if st.amp > 0.01 {
                    st.ph += dt * 1.1;
                }

                let mut bob = 0.0;
                if let Some(bb) = st.bob_start {
                    let tb = (ts - bb) / 1000.0;
                    let imp =
                        8.5 * (-1.03 * tb).exp() * (2.0 * std::f64::consts::PI * tb / 1.35).sin();
                    bob = imp + 0.75 * wave_dev(SCENE_CX, st.amp, st.ph);
                }

                let bd = body_d(st.l, st.amp, st.ph, st.jet_a, st.jet_w);
                let _ = body.set_attribute("d", &bd);
                let _ = tidep.set_attribute("d", &bd);
                let _ = subcap.set_attribute("d", &bd);
                let wl = wave_top(st.l, st.amp, st.ph, st.jet_a, st.jet_w);
                let _ = crest.set_attribute("d", &wl);
                let _ = crest_g.set_attribute("d", &wl);
                let _ = ballc.set_attribute("cy", &format!("{:.2}", SCENE_CY + bob));
                let _ = orbg.set_attribute("transform", &format!("translate(0 {:.2})", bob));
                drop(st);
            }

            if let Some(c) = holder_inner.borrow().as_ref() {
                if let Ok(id) = win.request_animation_frame(c.as_ref().unchecked_ref()) {
                    *raf_inner.borrow_mut() = id;
                }
            }
        }) as Box<dyn FnMut(f64)>);

        if let Some(win) = web_sys::window() {
            if let Ok(id) = win.request_animation_frame(cb.as_ref().unchecked_ref()) {
                *raf_id.borrow_mut() = id;
            }
        }
        *holder.borrow_mut() = Some(cb);
        let cleanup = send_wrapper::SendWrapper::new((raf_id, holder));
        on_cleanup(move || {
            let (raf_id, holder) = &*cleanup;
            if let Some(w) = web_sys::window() {
                let _ = w.cancel_animation_frame(*raf_id.borrow());
            }
            holder.borrow_mut().take();
        });
    }

    view! { <span style="display:contents" inner_html=html></span> }
}

/// Pending "thinking" card — a compact water-fill plate that rises to ~half
/// height and reveals the word through the tide. Aesthetic proportions follow
/// the v15 load row; the half-fill reveal is the product pending treatment.
/// Class root is `ocean-thinkfill` (not `ocean-thinking`) so it never collides
/// with the composer reasoning-effort `<select class="ocean-thinking">`.
#[component]
pub fn OceanThinking() -> impl IntoView {
    let uid = UID.fetch_add(1, Ordering::Relaxed);
    let reduce = web_sys::window()
        .and_then(|w| {
            w.match_media("(prefers-reduced-motion: reduce)")
                .ok()
                .flatten()
        })
        .map(|m| m.matches())
        .unwrap_or(false);

    // The badge orb rests low in the plate (cy 31), then floats up to ride
    // the waterline as the card half-fills. 26px nested svg reusing the
    // reveal orb's per-uid markup; the translucent water drawn over it
    // tints the submerged half.
    let orb_markup = orb_inner_html(uid);
    let orb_y: f64 = if reduce { 18.0 + (22.0 - 31.0) } else { 18.0 };

    // Settled / reduced-motion: hold at the half-fill frame.
    let y0 = if reduce { 22.0 } else { 46.0 };
    let crest_y = y0 - 1.5;
    let water_d = format!("M0 {y0:.1} Q37.5 {crest_y:.1} 75 {y0:.1} T150 {y0:.1} V44 H0 Z");
    let crest_d = format!("M0 {y0:.1} Q37.5 {crest_y:.1} 75 {y0:.1} T150 {y0:.1}");
    let html = format!(
        "<svg class='ocean-thinkfill__card' viewBox='0 0 150 44' width='150' height='44' aria-hidden='true'>\
        <defs>\
        <linearGradient id='tf-water-{uid}' x1='0' y1='0' x2='0' y2='1'>\
        <stop offset='0' class='ocean-thinkfill__stop-hi' stop-opacity='0.55'/>\
        <stop offset='0.55' class='ocean-thinkfill__stop-mid' stop-opacity='0.72'/>\
        <stop offset='1' class='ocean-thinkfill__stop-lo' stop-opacity='0.88'/>\
        </linearGradient>\
        <clipPath id='tf-clip-{uid}'><path id='tf-clipP-{uid}' d='{water_d}'/></clipPath>\
        </defs>\
        <rect x='0.5' y='0.5' width='149' height='43' rx='10' class='ocean-thinkfill__plate'/>\
        <g id='tf-orb-{uid}' transform='translate(12 {orb_y:.1})'><svg width='26' height='26' viewBox='0 0 48 48' fill='none'>{orb_markup}</svg></g>\
        <path id='tf-waterP-{uid}' class='ocean-thinkfill__water' fill='url(#tf-water-{uid})' d='{water_d}'/>\
        <text class='ocean-thinkfill__label ocean-thinkfill__label--dim' x='88' y='27' text-anchor='middle' font-family='var(--wm-font),Poppins,system-ui' font-weight='600' font-size='14'>thinking…</text>\
        <g clip-path='url(#tf-clip-{uid})'>\
        <text class='ocean-thinkfill__label ocean-thinkfill__label--lit' x='88' y='27' text-anchor='middle' font-family='var(--wm-font),Poppins,system-ui' font-weight='600' font-size='14'>thinking…</text>\
        </g>\
        <path id='tf-crest-{uid}' class='ocean-thinkfill__crest' fill='none' stroke='var(--water-crest)' stroke-width='1.4' stroke-linecap='round' stroke-opacity='0.75' d='{crest_d}'/>\
        </svg>"
    );

    if !reduce {
        let raf_id: Rc<RefCell<i32>> = Rc::new(RefCell::new(0));
        let holder: Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>> = Rc::new(RefCell::new(None));
        let start: Rc<RefCell<Option<f64>>> = Rc::new(RefCell::new(None));
        let water_id = format!("tf-waterP-{uid}");
        let clip_id = format!("tf-clipP-{uid}");
        let crest_id = format!("tf-crest-{uid}");
        let orb_id = format!("tf-orb-{uid}");
        let holder_inner = holder.clone();
        let raf_inner = raf_id.clone();

        let cb = Closure::wrap(Box::new(move |ts: f64| {
            let Some(win) = web_sys::window() else { return };
            let Some(doc) = win.document() else { return };
            let Some(water) = doc.get_element_by_id(&water_id) else {
                return;
            };
            let Some(clip) = doc.get_element_by_id(&clip_id) else {
                return;
            };
            let Some(crest) = doc.get_element_by_id(&crest_id) else {
                return;
            };
            let Some(orb) = doc.get_element_by_id(&orb_id) else {
                return;
            };

            let t0 = {
                let mut b = start.borrow_mut();
                if b.is_none() {
                    *b = Some(ts);
                }
                b.unwrap()
            };
            let t = (ts - t0) / 1000.0;

            // Ease to half-fill over ~1.1s, then idle with a soft crest ride.
            let fill_tau = (t / 1.1).clamp(0.0, 1.0);
            let ease = 1.0 - (1.0 - fill_tau).powi(3); // power2.out
            let base_y = 46.0 + (22.0 - 46.0) * ease;
            let ph = t * 1.35;
            let amp = 1.35 + 0.35 * (ph * 0.7).sin();
            let y = base_y + amp * ph.sin() * 0.35;
            let c1 = y - 1.6 * (ph * 1.1).cos();
            let d = format!("M0 {y:.2} Q37.5 {c1:.2} 75 {y:.2} T150 {y:.2} V44 H0 Z");
            let crest_d = format!("M0 {y:.2} Q37.5 {c1:.2} 75 {y:.2} T150 {y:.2}");
            let _ = water.set_attribute("d", &d);
            let _ = clip.set_attribute("d", &d);
            let _ = crest.set_attribute("d", &crest_d);

            // The orb sits at rest (cy 31) until the tide reaches it, then
            // floats pinned to the waterline — riding the same crest wave.
            let orb_ty = (y - 31.0).min(0.0);
            let _ = orb.set_attribute("transform", &format!("translate(12 {:.2})", 18.0 + orb_ty));

            if let Some(c) = holder_inner.borrow().as_ref() {
                if let Ok(id) = win.request_animation_frame(c.as_ref().unchecked_ref()) {
                    *raf_inner.borrow_mut() = id;
                }
            }
        }) as Box<dyn FnMut(f64)>);

        if let Some(win) = web_sys::window() {
            if let Ok(id) = win.request_animation_frame(cb.as_ref().unchecked_ref()) {
                *raf_id.borrow_mut() = id;
            }
        }
        *holder.borrow_mut() = Some(cb);
        let cleanup = send_wrapper::SendWrapper::new((raf_id, holder));
        on_cleanup(move || {
            let (raf_id, holder) = &*cleanup;
            if let Some(w) = web_sys::window() {
                let _ = w.cancel_animation_frame(*raf_id.borrow());
            }
            holder.borrow_mut().take();
        });
    }

    view! {
        <div class="ocean-thinkfill" role="status" aria-label="Ocean is thinking">
            <span style="display:contents" inner_html=html></span>
        </div>
    }
}
