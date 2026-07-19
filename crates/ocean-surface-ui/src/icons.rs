//! Inline SVG icons (game-icons.net, CC-BY 3.0). Each renders at 1em and
//! inherits `currentColor`, so CSS color controls them — they match the
//! site palette (cyan --accent, etc.) wherever they're placed.

use std::cell::RefCell;
use std::rc::Rc;

use leptos::prelude::*;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, ResizeObserver};

// ── Tide Coin (Canvas 2D) ──────────────────────────────────────────────
// Ported 1:1 from deliverables/engines/orbstudio.js kind='tide'.
// The coin is drawn in the canvas backing-store coordinate space S
// (S = CSS px × min(devicePixelRatio, 2)). Geometry is fractional-S,
// matching the 48px design space scaled to the actual canvas size.
// spinning=true churns the waterline via rAF (calm=0); false or
// reduced-motion renders the static calm=1 rest state.

const TAU: f64 = std::f64::consts::TAU;

fn coin_base(ctx: &CanvasRenderingContext2d, s: f64) {
    ctx.clear_rect(0.0, 0.0, s, s);
    let g = ctx.create_linear_gradient(0.0, 0.0, 0.0, s);
    let _ = g.add_color_stop(0.0, "#0F0F10");
    let _ = g.add_color_stop(0.55, "#020203");
    ctx.set_fill_style_canvas_gradient(&g);
    ctx.begin_path();
    let _ = ctx.arc(s / 2.0, s / 2.0, s * (21.0 / 48.0), 0.0, TAU);
    ctx.fill();
}

fn clip_coin(ctx: &CanvasRenderingContext2d, s: f64) {
    ctx.begin_path();
    let _ = ctx.arc(s / 2.0, s / 2.0, s * (20.4 / 48.0), 0.0, TAU);
    ctx.clip();
}

fn coin_top(ctx: &CanvasRenderingContext2d, s: f64) {
    let c = s / 2.0;
    // vignette
    let vig = ctx
        .create_radial_gradient(c, c, 0.0, c, c, s * (20.6 / 48.0))
        .unwrap();
    let _ = vig.add_color_stop(0.62, "rgba(0,0,0,0)");
    let _ = vig.add_color_stop(1.0, "rgba(0,0,0,0.55)");
    ctx.set_fill_style_canvas_gradient(&vig);
    ctx.begin_path();
    let _ = ctx.arc(c, c, s * (20.6 / 48.0), 0.0, TAU);
    ctx.fill();
    // occlusion
    let occ = ctx
        .create_radial_gradient(c, c, 0.0, c, c, s * (20.4 / 48.0))
        .unwrap();
    let _ = occ.add_color_stop(0.78, "rgba(0,0,0,0)");
    let _ = occ.add_color_stop(0.94, "rgba(0,0,0,0.26)");
    let _ = occ.add_color_stop(1.0, "rgba(0,0,0,0.72)");
    ctx.set_fill_style_canvas_gradient(&occ);
    ctx.begin_path();
    let _ = ctx.arc(c, c, s * (20.4 / 48.0), 0.0, TAU);
    ctx.fill();
    // gloss — upper-left source, fades outward
    let glo = ctx
        .create_radial_gradient(c, s * 0.18, 0.0, c, s * 0.18, s * 0.6)
        .unwrap();
    let _ = glo.add_color_stop(0.0, "rgba(207,244,255,0.15)");
    let _ = glo.add_color_stop(0.55, "rgba(207,244,255,0)");
    ctx.set_fill_style_canvas_gradient(&glo);
    ctx.begin_path();
    let _ = ctx.arc(c, c, s * (20.4 / 48.0), 0.0, TAU);
    ctx.fill();
    // rim stroke
    let rim = ctx.create_linear_gradient(0.0, s * (3.0 / 48.0), 0.0, s * (45.0 / 48.0));
    let _ = rim.add_color_stop(0.0, "rgba(177,183,186,0.45)");
    let _ = rim.add_color_stop(1.0, "rgba(107,112,115,0.45)");
    ctx.set_stroke_style_canvas_gradient(&rim);
    ctx.set_line_width(0.5_f64.max(s * (0.55 / 48.0)));
    ctx.begin_path();
    let _ = ctx.arc(c, c, s * (20.7 / 48.0), 0.0, TAU);
    ctx.stroke();
}

/// Draw the tide water body + crest inside an already-clipped canvas.
/// `amp` = 1.0 when churning, 0.25 at rest.
fn draw_tide_water(ctx: &CanvasRenderingContext2d, s: f64, t: f64, amp: f64) {
    let lvl = s * 0.46 + s * 0.012 * (0.8 * t).sin() * amp;
    let dev = |x: f64| -> f64 {
        s * (0.030 * ((x / s) * 9.2 + 1.9 * t).sin()
            + 0.016 * ((x / s) * 16.4 - 1.3 * t + 2.0).sin())
            * amp
    };
    let step = 2.0_f64.max(s / 44.0);

    // ── water body ──
    ctx.begin_path();
    ctx.move_to(-2.0, lvl + dev(0.0));
    let mut x = 0.0;
    while x <= s + step {
        ctx.line_to(x, lvl + dev(x));
        x += step;
    }
    ctx.line_to(s + 2.0, s + 2.0);
    ctx.line_to(-2.0, s + 2.0);
    ctx.close_path();
    let wg = ctx.create_linear_gradient(0.0, lvl - s * 0.05, 0.0, s);
    let _ = wg.add_color_stop(0.0, "rgba(46,166,198,0.52)");
    let _ = wg.add_color_stop(0.5, "rgba(28,99,176,0.60)");
    let _ = wg.add_color_stop(1.0, "rgba(10,22,66,0.78)");
    ctx.set_fill_style_canvas_gradient(&wg);
    ctx.fill();

    // ── subsurface strata echoes ──
    ctx.set_stroke_style_str("rgba(5,16,31,0.5)");
    ctx.set_line_width(0.6_f64.max(s * 0.008));
    for b in 1..=3 {
        let by = lvl + (s - lvl) * (b as f64 / 4.0);
        ctx.begin_path();
        let mut x = 0.0;
        while x <= s + step {
            let y = by + dev(x + (b * 17) as f64) * (0.5 / b as f64);
            if x == 0.0 {
                ctx.move_to(-2.0, y);
            } else {
                ctx.line_to(x, y);
            }
            x += step;
        }
        ctx.stroke();
    }

    // ── crest (double-stroke waterline for foam) ──
    // Draw the waterline path into the current path, then stroke it twice
    // with different width/opacity for the foam effect.
    let crest_path = |ctx: &CanvasRenderingContext2d| {
        ctx.begin_path();
        let mut x = 0.0;
        while x <= s + step {
            let y = lvl + dev(x);
            if x == 0.0 {
                ctx.move_to(-2.0, y);
            } else {
                ctx.line_to(x, y);
            }
            x += step;
        }
    };
    ctx.set_stroke_style_str("rgba(191,239,248,0.30)");
    ctx.set_line_width(1.4_f64.max(s * 0.030));
    crest_path(ctx);
    ctx.stroke();
    ctx.set_stroke_style_str("rgba(234,252,255,0.85)");
    ctx.set_line_width(0.7_f64.max(s * 0.012));
    crest_path(ctx);
    ctx.stroke();
}

/// Full tide frame: coin base → water → coin top.
fn draw_tide_frame(ctx: &CanvasRenderingContext2d, s: f64, t: f64, spinning: bool) {
    coin_base(ctx, s);
    ctx.save();
    clip_coin(ctx, s);
    let amp = if spinning { 1.0 } else { 0.25 };
    draw_tide_water(ctx, s, t, amp);
    ctx.restore();
    coin_top(ctx, s);
}

/// Holder for the self-rescheduling rAF callback (outlives registration;
/// dropped on unmount).
type RafHolder = Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>;

/// Holder for the reveal-watch `ResizeObserver` callback (outlives registration;
/// dropped on first draw / unmount). Mirrors the observatory precedent.
type ResizeHolder = Rc<RefCell<Option<Closure<dyn FnMut(js_sys::Array, ResizeObserver)>>>>;

/// Admit the first *real* layout: the canvas must measure at least 2 CSS px on
/// its smaller side before we size the backing store and paint. Below that the
/// element is collapsed or not yet laid out (e.g. inside a `display:none`
/// ancestor). Pure so it can be unit-tested without a DOM.
fn layout_admits(css_w: i32, css_h: i32) -> bool {
    (css_w.min(css_h) as f64) >= 2.0
}

/// Whether the churning rAF loop should run: only when the badge is `spinning`
/// and the user has not asked to reduce motion. The static rest frame is always
/// painted regardless, so a `false` here means "static frame only, no loop."
/// Pure decider.
fn wants_animation_loop(spinning: bool, reduce: bool) -> bool {
    spinning && !reduce
}

/// Measure the canvas and, once it has a real layout, size the DPR-scaled
/// backing store and paint the tide frame (starting the churn loop when
/// `spinning` and motion is allowed). Returns `true` once it has painted (or
/// hit an unrecoverable 2D-context error) so the caller stops deferring;
/// `false` while the element is still unlaid-out, so a `ResizeObserver` can
/// retry on the first non-zero layout. Callable from both the mount `Effect`
/// and the reveal observer — the DPR/backing-store math and reduced-motion
/// pause are identical on both paths.
fn draw_wave_badge(
    el: &HtmlCanvasElement,
    spinning: bool,
    raf_id: &Rc<RefCell<i32>>,
    holder: &RafHolder,
    start: &Rc<RefCell<Option<f64>>>,
) -> bool {
    let Some(win) = web_sys::window() else {
        return false;
    };

    // Measure CSS pixel size of the 1em canvas.
    let css_w = el.offset_width();
    let css_h = el.offset_height();
    if !layout_admits(css_w, css_h) {
        return false; // not laid out yet — let the observer retry on reveal
    }
    let css_px = css_w.min(css_h) as f64;

    let dpr = win.device_pixel_ratio().min(2.0);
    let s = (css_px * dpr).round();
    el.set_width(s as u32);
    el.set_height(s as u32);

    let Ok(ctx) = (|| -> Result<CanvasRenderingContext2d, JsValue> {
        el.get_context("2d")
            .map_err(|_| JsValue::from("getContext"))?
            .ok_or_else(|| JsValue::from("no 2d"))?
            .dyn_into::<CanvasRenderingContext2d>()
            .map_err(|_| JsValue::from("cast"))
    })() else {
        return true; // context unrecoverable — a re-layout won't fix it
    };

    // Always draw the static rest frame first so the canvas is never blank.
    draw_tide_frame(&ctx, s, 0.0, false);

    if !spinning {
        return true;
    }

    // Paused under prefers-reduced-motion: the static frame is enough.
    let reduce = win
        .match_media("(prefers-reduced-motion: reduce)")
        .ok()
        .flatten()
        .map(|m| m.matches())
        .unwrap_or(false);
    if !wants_animation_loop(spinning, reduce) {
        return true;
    }

    // Self-rescheduling rAF loop. DPR-scaled S is already the backing-store
    // pixel size, passed straight to the tide engine.
    let raf_id2 = raf_id.clone();
    let holder2 = holder.clone();
    let start2 = start.clone();
    let ctx2 = ctx.clone();

    let cb = Closure::wrap(Box::new(move |ts: f64| {
        let t0 = {
            let mut b = start2.borrow_mut();
            if b.is_none() {
                *b = Some(ts);
            }
            b.unwrap()
        };
        let t = (ts - t0) / 1000.0;
        draw_tide_frame(&ctx2, s, t, true);

        if let Some(c) = holder2.borrow().as_ref() {
            if let Some(win) = web_sys::window() {
                if let Ok(id) = win.request_animation_frame(c.as_ref().unchecked_ref()) {
                    *raf_id2.borrow_mut() = id;
                }
            }
        }
    }) as Box<dyn FnMut(f64)>);

    if let Some(win) = web_sys::window() {
        if let Ok(id) = win.request_animation_frame(cb.as_ref().unchecked_ref()) {
            *raf_id.borrow_mut() = id;
        }
    }
    *holder.borrow_mut() = Some(cb);
    true
}

// ── WaveBadge component ────────────────────────────────────────────────

/// The Ocean badge — a Canvas 2D tide coin (waterline churns while loading,
/// settles calm at rest). `spinning=true` drives a rAF loop paused under
/// `prefers-reduced-motion`; `spinning=false` draws the static rest frame
/// once. `compact` is passed as a CSS class for sizing; the canvas measures
/// its own CSS pixel box at mount time. DPR capped at 2.
#[component]
pub fn WaveBadge(
    #[prop(optional)] spinning: bool,
    #[prop(optional)] compact: bool,
) -> impl IntoView {
    let canvas_ref: NodeRef<leptos::html::Canvas> = NodeRef::new();
    let raf_id: Rc<RefCell<i32>> = Rc::new(RefCell::new(0));
    let holder: RafHolder = Rc::new(RefCell::new(None));
    let start: Rc<RefCell<Option<f64>>> = Rc::new(RefCell::new(None));
    // Reveal watch: armed only when the canvas is not laid out at mount (a
    // collapsed / display:none ancestor). Disconnected on first draw and on
    // unmount. Precedent: observatory/scene.rs.
    let resize_observer: Rc<RefCell<Option<ResizeObserver>>> = Rc::new(RefCell::new(None));
    let resize_callback: ResizeHolder = Rc::new(RefCell::new(None));

    // Cancel pending rAF, drop the callback, and tear down the reveal observer
    // on unmount.
    let c_raf = raf_id.clone();
    let c_holder = holder.clone();
    let c_observer = resize_observer.clone();
    let c_resize_callback = resize_callback.clone();
    let cleanup = send_wrapper::SendWrapper::new((c_raf, c_holder, c_observer, c_resize_callback));
    on_cleanup(move || {
        let (raf_id, holder, resize_observer, resize_callback) = &*cleanup;
        if let Some(w) = web_sys::window() {
            let _ = w.cancel_animation_frame(*raf_id.borrow());
        }
        holder.borrow_mut().take();
        if let Some(observer) = resize_observer.borrow().as_ref() {
            observer.disconnect();
        }
        resize_observer.borrow_mut().take();
        resize_callback.borrow_mut().take();
    });

    let cls = if compact {
        "wave-badge wave-badge--compact"
    } else {
        "wave-badge"
    };

    // Only reactive read: the canvas mounting (None -> Some). A NodeRef fires
    // this exactly once, so we can't rely on it to re-run when a collapsed
    // ancestor is later revealed — that admission is handled by the observer
    // below.
    let effect_raf = raf_id.clone();
    let effect_holder = holder.clone();
    let effect_start = start.clone();
    let effect_observer = resize_observer.clone();
    let effect_callback = resize_callback.clone();
    Effect::new(move |_| {
        let Some(el) = canvas_ref.get() else { return };

        // Visible-at-mount path: draw immediately, exactly as before. No
        // observer is armed, so there is no extra work or blank-frame flash.
        if draw_wave_badge(&el, spinning, &effect_raf, &effect_holder, &effect_start) {
            return;
        }

        // Mounted but not laid out (collapsed / display:none ancestor). Rather
        // than bail forever, observe the canvas for its first non-zero layout
        // and paint on reveal. Modeled on observatory/scene.rs.
        if effect_observer.borrow().is_some() {
            return; // already armed
        }
        let cb_el = el.clone();
        let cb_raf = effect_raf.clone();
        let cb_holder = effect_holder.clone();
        let cb_start = effect_start.clone();
        let cb_observer = effect_observer.clone();
        let callback = Closure::wrap(Box::new(
            move |_entries: js_sys::Array, _observer: ResizeObserver| {
                // ResizeObserver fires once on observe() (still zero-sized) and
                // again on reveal. Draw only when a real layout arrives, then
                // disconnect so this fires exactly once for the badge's life.
                if draw_wave_badge(&cb_el, spinning, &cb_raf, &cb_holder, &cb_start) {
                    if let Some(observer) = cb_observer.borrow().as_ref() {
                        observer.disconnect();
                    }
                }
            },
        ) as Box<dyn FnMut(js_sys::Array, ResizeObserver)>);
        let Ok(observer) = ResizeObserver::new(callback.as_ref().unchecked_ref()) else {
            return;
        };
        observer.observe(&el);
        *effect_callback.borrow_mut() = Some(callback);
        *effect_observer.borrow_mut() = Some(observer);
    });

    view! {
        <canvas
            node_ref=canvas_ref
            class=cls
            style="width:1em; height:1em; display:block"
            aria-hidden="true"
        />
    }
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

/// Up-arrow — the composer Send action (submit). Stroke family.
#[component]
pub fn Send() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M12 19V6" />
            <path d="M6 12l6-6 6 6" />
        </svg>
    }
}

/// Filled rounded square — the composer Stop action (halt the running turn).
#[component]
pub fn Stop() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 24 24" width="1em" height="1em"
             fill="currentColor" aria-hidden="true">
            <rect x="6" y="6" width="12" height="12" rx="2.5" />
        </svg>
    }
}

/// Single person — human room participant (replaces "🧑"). Rounded 2px
/// stroke to match the Groups two-person mark. (QA-006)
#[component]
pub fn Person() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2" />
            <circle cx="12" cy="7" r="4" />
        </svg>
    }
}

/// Robot head — agent room participant (replaces "🤖"). Antenna + two eye
/// ticks so it reads "agent" at chip size. (QA-006)
#[component]
pub fn Robot() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <rect x="5" y="9" width="14" height="10" rx="2" />
            <line x1="12" y1="5" x2="12" y2="9" />
            <circle cx="12" cy="4" r="1" />
            <line x1="9" y1="13" x2="9" y2="15" />
            <line x1="15" y1="13" x2="15" y2="15" />
        </svg>
    }
}

/// Cog — bot room participant (replaces "⚙"). Hub + rim + eight teeth.
/// (QA-006)
#[component]
pub fn Cog() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <circle cx="12" cy="12" r="6" />
            <circle cx="12" cy="12" r="2" />
            <line x1="12" y1="3" x2="12" y2="6" />
            <line x1="12" y1="18" x2="12" y2="21" />
            <line x1="3" y1="12" x2="6" y2="12" />
            <line x1="18" y1="12" x2="21" y2="12" />
            <line x1="5.6" y1="5.6" x2="7.8" y2="7.8" />
            <line x1="16.2" y1="16.2" x2="18.4" y2="18.4" />
            <line x1="5.6" y1="18.4" x2="7.8" y2="16.2" />
            <line x1="16.2" y1="7.8" x2="18.4" y2="5.6" />
        </svg>
    }
}

/// Wrench — tool room participant (replaces "🔧"). (QA-006)
#[component]
pub fn Wrench() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z" />
        </svg>
    }
}

/// Four-point spark — system room participant / system rows (replaces "✦").
/// (QA-006)
#[component]
pub fn Spark() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M12 3l2.2 6.8L21 12l-6.8 2.2L12 21l-2.2-6.8L3 12l6.8-2.2z" />
        </svg>
    }
}

/// Diagonal cross — close/dismiss affordance (replaces "✕"). (QA-006)
#[component]
pub fn Close() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <line x1="6" y1="6" x2="18" y2="18" />
            <line x1="18" y1="6" x2="6" y2="18" />
        </svg>
    }
}

/// Clockwise circular arrow — refresh affordance (replaces "↻"). (QA-006)
#[component]
pub fn Refresh() -> impl IntoView {
    view! {
        <svg class="icon icon--stroke" viewBox="0 0 24 24" width="1em" height="1em"
             fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <polyline points="23 4 23 10 17 10" />
            <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10" />
        </svg>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── layout admission (pure decider) ─────────────────────────────────
    // The bug was a permanent bail when the badge measured < 2 CSS px. These
    // pin the admission boundary that both the mount Effect and the reveal
    // observer share, so a re-measure on reveal admits the same badge that a
    // pre-layout measure rejects. (Contract tests a + b: the DOM-level
    // "draws on mount" / "draws on reveal" behavior is driven by re-running
    // this decider from the observer — see the source-structure tests below.)

    #[test]
    fn layout_admits_a_measured_badge() {
        // A visible 1em badge measures well above the 2px floor.
        assert!(layout_admits(24, 24));
        assert!(layout_admits(16, 16));
    }

    #[test]
    fn layout_rejects_a_collapsed_badge() {
        // display:none / collapsed ancestor → zero box, not yet drawable.
        assert!(!layout_admits(0, 0));
        assert!(!layout_admits(1, 1));
    }

    #[test]
    fn layout_admission_boundary_is_two_css_px() {
        assert!(!layout_admits(1, 1));
        assert!(layout_admits(2, 2));
    }

    #[test]
    fn layout_uses_the_smaller_dimension() {
        // A zero-height (but wide) box is still not laid out.
        assert!(!layout_admits(200, 0));
        assert!(!layout_admits(0, 200));
        assert!(layout_admits(200, 2));
    }

    // ── motion gate (pure decider) ──────────────────────────────────────
    // Contract test d: reduced-motion (and the non-spinning case) must paint
    // the static frame but never start the rAF loop.

    #[test]
    fn animation_loop_runs_only_when_spinning_and_motion_allowed() {
        assert!(wants_animation_loop(true, false));
        assert!(!wants_animation_loop(true, true)); // reduced-motion → no loop
        assert!(!wants_animation_loop(false, false)); // static badge → no loop
        assert!(!wants_animation_loop(false, true));
    }

    // ── source-structure invariants ─────────────────────────────────────
    // WASM/DOM behavior can't run under `cargo test` (native), so these assert
    // the wiring the contract requires, per the repo's include_str! pattern
    // (see palette.rs). They guard against regressing the reveal/teardown
    // structure without a headless browser.
    const SRC: &str = include_str!("icons.rs");

    #[test]
    fn draw_paints_static_frame_before_the_motion_gate() {
        // Contract: the static rest-frame draw must precede the spinning /
        // reduced-motion early-returns, so every admitted layout paints even
        // when the loop never starts.
        let f = SRC
            .find("fn draw_wave_badge(")
            .expect("draw_wave_badge exists");
        let body = &SRC[f..];
        let static_draw = body
            .find("draw_tide_frame(&ctx, s, 0.0, false)")
            .expect("static rest frame drawn");
        let spin_gate = body.find("if !spinning {").expect("spinning gate");
        let motion_gate = body
            .find("wants_animation_loop(spinning, reduce)")
            .expect("motion gate");
        assert!(
            static_draw < spin_gate && spin_gate < motion_gate,
            "static frame must paint before the spinning/reduced-motion gates"
        );
    }

    #[test]
    fn mount_draws_before_arming_the_observer() {
        // Contract test a: the visible-at-mount path draws and returns without
        // arming an observer (no extra work, no blank-frame flash).
        let e = SRC.find("Effect::new(move |_| {").expect("mount effect");
        let body = &SRC[e..];
        let draw = body
            .find("if draw_wave_badge(&el, spinning,")
            .expect("mount tries the draw first");
        let observe = body.find("observer.observe(&el)").expect("observer arm");
        assert!(
            draw < observe,
            "the mount effect must attempt the draw before arming the observer"
        );
    }

    #[test]
    fn observer_redraws_and_disconnects_after_first_layout() {
        // Contract test b + c: the observer callback re-runs the draw and, on
        // success, disconnects so it fires exactly once.
        let c = SRC
            .find("move |_entries: js_sys::Array, _observer: ResizeObserver|")
            .expect("resize callback");
        let body = &SRC[c..];
        let redraw = body
            .find("if draw_wave_badge(&cb_el, spinning,")
            .expect("callback re-runs the draw on reveal");
        let disconnect = body
            .find("observer.disconnect()")
            .expect("callback disconnects after a successful draw");
        assert!(
            redraw < disconnect,
            "the observer must draw before disconnecting"
        );
    }

    #[test]
    fn cleanup_tears_down_observer_and_raf() {
        // Contract test c (unmount half): on_cleanup cancels the rAF, drops the
        // loop callback, and disconnects + drops the reveal observer.
        let cu = SRC.find("on_cleanup(move || {").expect("on_cleanup");
        let body = &SRC[cu..];
        let end = body.find("});").expect("cleanup body closes");
        let cleanup = &body[..end];
        assert!(
            cleanup.contains("cancel_animation_frame"),
            "cleanup cancels the pending rAF"
        );
        assert!(
            cleanup.contains("holder.borrow_mut().take()"),
            "cleanup drops the rAF loop callback"
        );
        assert!(
            cleanup.contains("observer.disconnect()"),
            "cleanup disconnects the reveal observer"
        );
        assert!(
            cleanup.contains("resize_observer.borrow_mut().take()"),
            "cleanup drops the observer handle"
        );
        assert!(
            cleanup.contains("resize_callback.borrow_mut().take()"),
            "cleanup drops the observer callback"
        );
    }
}
