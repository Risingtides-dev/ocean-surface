use std::cell::{Cell, RefCell};
use std::rc::Rc;

use leptos::prelude::*;
use wasm_bindgen::{closure::Closure, JsCast};
use web_sys::{CanvasRenderingContext2d, ResizeObserver};

use super::domain::{short_id, ExecutionPhase, ExecutionState, IntegrityState, ObservatoryState};
use super::layout::{
    build_layout, grid_to_screen, CubicleLayout, FloorLayout, StationLayout, CUBICLE_DEPTH, TILE_H,
    TILE_W,
};

type RafHolder = Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>;
type ResizeHolder = Rc<RefCell<Option<Closure<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>>>>;

#[derive(Clone)]
struct Palette {
    water: String,
    water_dither: String,
    rock_side: String,
    rock_top: String,
    deck_a: String,
    deck_b: String,
    wall_top: String,
    wall_face: String,
    wall_dark: String,
    desk_top: String,
    desk_side: String,
    wood: String,
    plant: String,
    metal: String,
    metal_hi: String,
    screen_off: String,
    screen_on: String,
    green: String,
    red: String,
    amber: String,
    tether: String,
    packet: String,
    skin: String,
    outfit: String,
    ink: String,
}

impl Palette {
    fn load() -> Self {
        Self {
            water: css_token("--floor-water"),
            water_dither: css_token("--floor-water-dither"),
            rock_side: css_token("--floor-rock-side"),
            rock_top: css_token("--floor-rock-top"),
            deck_a: css_token("--floor-deck-a"),
            deck_b: css_token("--floor-deck-b"),
            wall_top: css_token("--floor-wall-top"),
            wall_face: css_token("--floor-wall-face"),
            wall_dark: css_token("--floor-wall-dark"),
            desk_top: css_token("--floor-desk-top"),
            desk_side: css_token("--floor-desk-side"),
            wood: css_token("--floor-wood"),
            plant: css_token("--floor-plant"),
            metal: css_token("--floor-metal"),
            metal_hi: css_token("--floor-metal-hi"),
            screen_off: css_token("--floor-screen-off"),
            screen_on: css_token("--floor-screen-on"),
            green: css_token("--floor-lamp-green"),
            red: css_token("--floor-lamp-red"),
            amber: css_token("--floor-lamp-amber"),
            tether: css_token("--floor-line"),
            packet: css_token("--floor-packet"),
            skin: css_token("--floor-skin"),
            outfit: css_token("--floor-outfit-blue"),
            ink: css_token("--floor-ink"),
        }
    }
}

fn css_token(name: &str) -> String {
    web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.document_element())
        .and_then(|root| web_sys::window()?.get_computed_style(&root).ok().flatten())
        .and_then(|style| style.get_property_value(name).ok())
        .map(|value| value.trim().to_owned())
        .unwrap_or_default()
}

#[component]
pub fn FloorScene(
    state: RwSignal<ObservatoryState>,
    selected: RwSignal<Option<String>>,
    zoom: RwSignal<f64>,
) -> impl IntoView {
    let viewport_ref: NodeRef<leptos::html::Div> = NodeRef::new();
    let canvas_ref: NodeRef<leptos::html::Canvas> = NodeRef::new();
    let viewport_size = RwSignal::new((0_i32, 0_i32));
    let layout = Memo::new(move |_| build_layout(&state.get()));
    let palette = Rc::new(Palette::load());
    let raf_id = Rc::new(RefCell::new(0_i32));
    let holder: RafHolder = Rc::new(RefCell::new(None));
    let running = Rc::new(Cell::new(false));

    let resize_observer = Rc::new(RefCell::new(None::<ResizeObserver>));
    let resize_callback: ResizeHolder = Rc::new(RefCell::new(None));
    let setup_observer = resize_observer.clone();
    let setup_callback = resize_callback.clone();
    Effect::new(move |_| {
        if setup_observer.borrow().is_some() {
            return;
        }
        let Some(viewport) = viewport_ref.get() else {
            return;
        };
        viewport_size.set((viewport.client_width(), viewport.client_height()));
        let measured_viewport = viewport.clone();
        let callback = Closure::wrap(Box::new(
            move |_entries: js_sys::Array, _observer: web_sys::ResizeObserver| {
                viewport_size.set((
                    measured_viewport.client_width(),
                    measured_viewport.client_height(),
                ));
            },
        )
            as Box<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>);
        let Ok(observer) = ResizeObserver::new(callback.as_ref().unchecked_ref()) else {
            return;
        };
        observer.observe(&viewport);
        *setup_callback.borrow_mut() = Some(callback);
        *setup_observer.borrow_mut() = Some(observer);
    });

    let cleanup_raf = raf_id.clone();
    let cleanup_holder = holder.clone();
    let cleanup_running = running.clone();
    let cleanup_observer = resize_observer.clone();
    let cleanup_resize_callback = resize_callback.clone();
    let cleanup = send_wrapper::SendWrapper::new((
        cleanup_raf,
        cleanup_holder,
        cleanup_running,
        cleanup_observer,
        cleanup_resize_callback,
    ));
    on_cleanup(move || {
        let (raf_id, holder, running, resize_observer, resize_callback) = &*cleanup;
        if let Some(window) = web_sys::window() {
            let _ = window.cancel_animation_frame(*raf_id.borrow());
        }
        if let Some(observer) = resize_observer.borrow().as_ref() {
            observer.disconnect();
        }
        resize_observer.borrow_mut().take();
        resize_callback.borrow_mut().take();
        running.set(false);
        holder.borrow_mut().take();
    });

    let loop_canvas = canvas_ref;
    let loop_holder = holder.clone();
    let loop_raf = raf_id.clone();
    let loop_running = running.clone();
    let loop_palette = palette.clone();
    let callback = Closure::wrap(Box::new(move |timestamp: f64| {
        draw_current(
            loop_canvas,
            state,
            selected,
            zoom,
            layout,
            &loop_palette,
            timestamp,
        );
        if state.get_untracked().needs_animation() && !prefers_reduced_motion() {
            if let (Some(window), Some(callback)) =
                (web_sys::window(), loop_holder.borrow().as_ref())
            {
                if let Ok(id) = window.request_animation_frame(callback.as_ref().unchecked_ref()) {
                    *loop_raf.borrow_mut() = id;
                    return;
                }
            }
        }
        loop_running.set(false);
    }) as Box<dyn FnMut(f64)>);
    *holder.borrow_mut() = Some(callback);

    let effect_holder = holder.clone();
    let effect_raf = raf_id.clone();
    let effect_running = running.clone();
    let effect_palette = palette.clone();
    Effect::new(move |_| {
        let current = state.get();
        let _ = selected.get();
        let _ = zoom.get();
        let _ = layout.get();
        let _ = viewport_size.get();
        draw_current(
            canvas_ref,
            state,
            selected,
            zoom,
            layout,
            &effect_palette,
            0.0,
        );
        if current.needs_animation() && !prefers_reduced_motion() && !effect_running.replace(true) {
            if let (Some(window), Some(callback)) =
                (web_sys::window(), effect_holder.borrow().as_ref())
            {
                if let Ok(id) = window.request_animation_frame(callback.as_ref().unchecked_ref()) {
                    *effect_raf.borrow_mut() = id;
                } else {
                    effect_running.set(false);
                }
            }
        }
    });

    view! {
        <div
            node_ref=viewport_ref
            class="ocean-floor__viewport"
            tabindex="0"
            aria-label="Ocean Floor scene; use the execution list for complete keyboard navigation"
        >
            <div
                class="ocean-floor__world"
                style=move || {
                    let layout = layout.get();
                    let zoom = zoom.get();
                    format!(
                        "width:{:.0}px;height:{:.0}px",
                        layout.width * zoom,
                        layout.height * zoom,
                    )
                }
            >
                <canvas node_ref=canvas_ref class="ocean-floor__canvas" aria-hidden="true" tabindex="-1"></canvas>
                <div class="ocean-floor__proxies" aria-label="Reactive execution cubicles">
                    <For
                        each=move || layout.get().stations
                        key=|station| station.execution_id.clone()
                        children=move |station| {
                            let station_id = station.execution_id.clone();
                            let station_id_selected = station.execution_id.clone();
                            let station_id_attention = station.execution_id.clone();
                            let station_id_label = station.execution_id.clone();
                            let station_id_name = station.execution_id.clone();
                            let station_id_phase = station.execution_id.clone();
                            let station_id_phase_class = station.execution_id.clone();
                            let station_slot = station.slot;
                            let station_pos = station.clone();
                            view! {
                                <button
                                    class="ocean-floor__station-proxy"
                                    class:is-selected=move || selected.get().as_deref() == Some(station_id_selected.as_str())
                                    class:is-attention=move || state.get().nodes.get(&station_id_attention).is_some_and(|node| node.permission_waiting || matches!(node.phase, ExecutionPhase::Error | ExecutionPhase::TimedOut))
                                    type="button"
                                    style=move || proxy_style(&layout.get(), &station_pos, zoom.get())
                                    aria-label=move || state.get().nodes.get(&station_id_label).map(|node| station_aria_label(node, station_slot)).unwrap_or_else(|| format!("cubicle {}", station_slot + 1))
                                    on:click=move |_| selected.set(Some(station_id.clone()))
                                >
                                    <span class="ocean-floor__station-label" aria-hidden="true">
                                        <strong>{move || {
                                            let label = state.get().nodes.get(&station_id_name).map(ExecutionState::label).unwrap_or_else(|| short_id(&station_id_name));
                                            format!("C-{:02} · {label}", station_slot + 1)
                                        }}</strong>
                                        <span
                                            class=move || state.get().nodes.get(&station_id_phase_class).map(|node| format!("ocean-floor__station-phase is-{}", phase_class(node.phase))).unwrap_or_else(|| "ocean-floor__station-phase".into())
                                        >
                                            {move || state.get().nodes.get(&station_id_phase).map(|node| node.phase.label()).unwrap_or("unknown")}
                                        </span>
                                    </span>
                                </button>
                            }
                        }
                    />
                </div>
            </div>
        </div>
    }
}

fn centered_axis(layout_span: f64, value: f64, minimum: f64, zoom: f64) -> String {
    let half = layout_span * zoom / 2.0;
    let local = (value - minimum) * zoom;
    format!("calc(50% - {half:.1}px + {local:.1}px)")
}

fn proxy_style(layout: &FloorLayout, station: &StationLayout, zoom: f64) -> String {
    let left = centered_axis(layout.width, station.screen_x - 44.0, layout.min_x, zoom);
    let top = centered_axis(layout.height, station.screen_y - 86.0, layout.min_y, zoom);
    let label_bottom = if (station.grid_x + station.grid_y) & 1 == 0 {
        -38.0
    } else {
        -58.0
    };
    format!(
        "left:{left};top:{top};width:{:.0}px;height:{:.0}px;--floor-label-bottom:{label_bottom:.0}px",
        88.0 * zoom,
        110.0 * zoom,
    )
}

fn station_aria_label(node: &ExecutionState, slot: usize) -> String {
    let activity = if node.permission_waiting {
        "permission waiting".to_owned()
    } else if node.tools.is_empty() {
        node.phase.label().to_owned()
    } else {
        format!("{}; {} tools running", node.phase.label(), node.tools.len())
    };
    format!("cubicle {}; {}; {activity}", slot + 1, node.label())
}

fn phase_class(phase: ExecutionPhase) -> &'static str {
    match phase {
        ExecutionPhase::Admitted => "admitted",
        ExecutionPhase::Running => "running",
        ExecutionPhase::Finished => "complete",
        ExecutionPhase::Error => "error",
        ExecutionPhase::Canceled => "interrupted",
        ExecutionPhase::TimedOut => "timed-out",
    }
}

fn draw_current(
    canvas_ref: NodeRef<leptos::html::Canvas>,
    state: RwSignal<ObservatoryState>,
    selected: RwSignal<Option<String>>,
    zoom: RwSignal<f64>,
    layout: Memo<FloorLayout>,
    palette: &Palette,
    timestamp: f64,
) {
    let Some(canvas) = canvas_ref.get() else {
        return;
    };
    let css_width = f64::from(canvas.client_width().max(1));
    let css_height = f64::from(canvas.client_height().max(1));
    let pixel_ratio = web_sys::window()
        .map(|window| window.device_pixel_ratio().clamp(1.0, 2.0))
        .unwrap_or(1.0);
    let width = (css_width * pixel_ratio).round() as u32;
    let height = (css_height * pixel_ratio).round() as u32;
    if canvas.width() != width {
        canvas.set_width(width);
    }
    if canvas.height() != height {
        canvas.set_height(height);
    }
    let Ok(Some(context)) = canvas.get_context("2d") else {
        return;
    };
    let Ok(context) = context.dyn_into::<CanvasRenderingContext2d>() else {
        return;
    };
    context.set_image_smoothing_enabled(false);
    let _ = context.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
    context.clear_rect(0.0, 0.0, f64::from(width), f64::from(height));
    let _ = context.scale(pixel_ratio, pixel_ratio);

    let state = state.get_untracked();
    let layout = layout.get_untracked();
    let zoom = zoom.get_untracked();
    draw_scene(
        &context,
        &state,
        &layout,
        selected.get_untracked().as_deref(),
        zoom,
        timestamp,
        css_width,
        css_height,
        palette,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_scene(
    context: &CanvasRenderingContext2d,
    state: &ObservatoryState,
    layout: &FloorLayout,
    selected: Option<&str>,
    zoom: f64,
    timestamp: f64,
    viewport_width: f64,
    viewport_height: f64,
    palette: &Palette,
) {
    draw_water(context, viewport_width, viewport_height, palette);

    let offset_x = ((viewport_width - layout.width * zoom) / 2.0).max(0.0);
    let offset_y = ((viewport_height - layout.height * zoom) / 2.0).max(0.0);
    context.save();
    let _ = context.translate(offset_x, offset_y);
    let _ = context.scale(zoom, zoom);
    let _ = context.translate(-layout.min_x, -layout.min_y);

    draw_cubicle_modules(context, state, layout, palette);
    draw_tethers(context, state, layout, palette);
    for station in &layout.stations {
        if let Some(node) = state.nodes.get(&station.execution_id) {
            draw_station(
                context,
                station,
                node,
                selected == Some(station.execution_id.as_str()),
                state.integrity,
                timestamp,
                palette,
            );
        }
    }
    context.restore();
}

fn draw_water(context: &CanvasRenderingContext2d, width: f64, height: f64, palette: &Palette) {
    context.set_fill_style_str(&palette.water);
    context.fill_rect(0.0, 0.0, width, height);
    context.set_fill_style_str(&palette.water_dither);
    let mut y = 8_i32;
    while f64::from(y) < height {
        let stagger = if (y / 16) % 2 == 0 { 8 } else { 20 };
        let mut x = stagger;
        while f64::from(x) < width {
            context.fill_rect(f64::from(x), f64::from(y), 2.0, 2.0);
            if (x / 24 + y / 16) % 5 == 0 {
                context.fill_rect(f64::from(x + 3), f64::from(y), 4.0, 1.0);
            }
            x += 32;
        }
        y += 20;
    }
}

fn draw_cubicle_modules(
    context: &CanvasRenderingContext2d,
    state: &ObservatoryState,
    layout: &FloorLayout,
    palette: &Palette,
) {
    for cubicle in &layout.cubicles {
        if let Some(node) = state.nodes.get(&cubicle.execution_id) {
            draw_cubicle_module(context, cubicle, node, palette);
        }
    }
}

/// Paint one self-contained cubicle. Its constant 5x5 footprint is the visual
/// module added when an execution is admitted; no neighboring cubicle geometry
/// is consulted, so existing modules never resize when the floor grows.
fn draw_cubicle_module(
    context: &CanvasRenderingContext2d,
    cubicle: &CubicleLayout,
    node: &ExecutionState,
    palette: &Palette,
) {
    let [top, right, bottom, left] = cubicle_outline(cubicle);
    context.set_global_alpha(0.58);
    polygon(
        context,
        &[
            (top.0 + 12.0, top.1 + 18.0),
            (right.0 + 16.0, right.1 + 18.0),
            (bottom.0 + 16.0, bottom.1 + CUBICLE_DEPTH + 28.0),
            (left.0 + 12.0, left.1 + CUBICLE_DEPTH + 28.0),
        ],
        &palette.ink,
    );
    context.set_global_alpha(1.0);

    polygon(
        context,
        &[
            right,
            bottom,
            (bottom.0, bottom.1 + CUBICLE_DEPTH),
            (right.0, right.1 + CUBICLE_DEPTH),
        ],
        &palette.rock_side,
    );
    polygon(
        context,
        &[
            bottom,
            left,
            (left.0, left.1 + CUBICLE_DEPTH),
            (bottom.0, bottom.1 + CUBICLE_DEPTH),
        ],
        &palette.wall_dark,
    );

    for local_y in 0..cubicle.tiles_h {
        for local_x in 0..cubicle.tiles_w {
            let (x, y) = grid_to_screen(cubicle.grid_x + local_x, cubicle.grid_y + local_y);
            let color = if (local_x + local_y) % 2 == 0 {
                &palette.deck_a
            } else {
                &palette.deck_b
            };
            diamond(context, x, y, TILE_W / 2.0, TILE_H / 2.0, color);
            if local_x == cubicle.tiles_w - 1 || local_y == cubicle.tiles_h - 1 {
                context.set_fill_style_str(&palette.wall_dark);
                context.fill_rect(x - 1.0, y + TILE_H / 2.0 - 2.0, 3.0, 3.0);
            }
        }
    }

    let root_accent = cubicle_root_accent(cubicle, palette);
    for local_x in 0..cubicle.tiles_w {
        let (x, y) = grid_to_screen(cubicle.grid_x + local_x, cubicle.grid_y);
        draw_wall_segment(
            context,
            (x, y - TILE_H / 2.0),
            (x + TILE_W / 2.0, y),
            &palette.wall_face,
            root_accent,
            palette,
        );
    }
    for local_y in 0..cubicle.tiles_h {
        let (x, y) = grid_to_screen(cubicle.grid_x, cubicle.grid_y + local_y);
        draw_wall_segment(
            context,
            (x, y - TILE_H / 2.0),
            (x - TILE_W / 2.0, y),
            &palette.wall_dark,
            root_accent,
            palette,
        );
    }

    let status = node_status_color(node, palette);
    draw_beacon(context, right.0 - 13.0, right.1 - 7.0, status, palette);

    // Deterministic furnishings increase visual density without inventing
    // behavior. They are architecture, not activity indicators.
    let (storage_x, storage_y) = grid_to_screen(cubicle.grid_x + 3, cubicle.grid_y + 1);
    draw_storage(context, storage_x, storage_y - 5.0, cubicle.slot, palette);
    let (plant_x, plant_y) = grid_to_screen(cubicle.grid_x + 1, cubicle.grid_y + 3);
    draw_planter(context, plant_x, plant_y - 2.0, palette);

    for fraction in [0.25, 0.5, 0.75] {
        let x = left.0 + (bottom.0 - left.0) * fraction;
        let y = left.1 + (bottom.1 - left.1) * fraction;
        context.set_fill_style_str(&palette.metal);
        context.fill_rect(x - 3.0, y + 2.0, 6.0, CUBICLE_DEPTH - 4.0);
        context.set_fill_style_str(&palette.metal_hi);
        context.fill_rect(x - 3.0, y + 2.0, 2.0, CUBICLE_DEPTH - 4.0);
    }
}

fn cubicle_outline(cubicle: &CubicleLayout) -> [(f64, f64); 4] {
    let (top_x, top_y) = grid_to_screen(cubicle.grid_x, cubicle.grid_y);
    let (right_x, right_y) = grid_to_screen(cubicle.grid_x + cubicle.tiles_w - 1, cubicle.grid_y);
    let (bottom_x, bottom_y) = grid_to_screen(
        cubicle.grid_x + cubicle.tiles_w - 1,
        cubicle.grid_y + cubicle.tiles_h - 1,
    );
    let (left_x, left_y) = grid_to_screen(cubicle.grid_x, cubicle.grid_y + cubicle.tiles_h - 1);
    [
        (top_x, top_y - TILE_H / 2.0),
        (right_x + TILE_W / 2.0, right_y),
        (bottom_x, bottom_y + TILE_H / 2.0),
        (left_x - TILE_W / 2.0, left_y),
    ]
}

fn draw_wall_segment(
    context: &CanvasRenderingContext2d,
    start: (f64, f64),
    end: (f64, f64),
    face: &str,
    accent: &str,
    palette: &Palette,
) {
    const WALL_H: f64 = 46.0;
    polygon(
        context,
        &[
            (start.0, start.1 - WALL_H),
            (end.0, end.1 - WALL_H),
            end,
            start,
        ],
        face,
    );
    polygon(
        context,
        &[
            (start.0, start.1 - WALL_H - 4.0),
            (end.0, end.1 - WALL_H - 4.0),
            (end.0, end.1 - WALL_H),
            (start.0, start.1 - WALL_H),
        ],
        &palette.wall_top,
    );
    polygon(
        context,
        &[
            (start.0, start.1 - 12.0),
            (end.0, end.1 - 12.0),
            (end.0, end.1 - 9.0),
            (start.0, start.1 - 9.0),
        ],
        accent,
    );
}

fn cubicle_root_accent<'a>(cubicle: &CubicleLayout, palette: &'a Palette) -> &'a str {
    match cubicle.accent_index {
        0 => &palette.screen_on,
        1 => &palette.outfit,
        _ => &palette.wood,
    }
}

fn node_status_color<'a>(node: &ExecutionState, palette: &'a Palette) -> &'a str {
    if node.permission_waiting {
        return &palette.amber;
    }
    match node.phase {
        ExecutionPhase::Finished => &palette.green,
        ExecutionPhase::Error | ExecutionPhase::TimedOut => &palette.red,
        ExecutionPhase::Canceled => &palette.tether,
        ExecutionPhase::Running | ExecutionPhase::Admitted => &palette.amber,
    }
}

fn draw_storage(
    context: &CanvasRenderingContext2d,
    x: f64,
    y: f64,
    slot: usize,
    palette: &Palette,
) {
    diamond(context, x, y, 18.0, 9.0, &palette.wood);
    context.set_fill_style_str(&palette.wood);
    context.fill_rect(x - 15.0, y, 30.0, 24.0);
    context.set_fill_style_str(&palette.wall_dark);
    context.fill_rect(x - 12.0, y + 5.0, 24.0, 3.0);
    context.fill_rect(x - 12.0, y + 12.0, 24.0, 3.0);
    context.set_fill_style_str(if slot % 2 == 0 {
        &palette.screen_on
    } else {
        &palette.packet
    });
    context.fill_rect(x + 7.0, y + 18.0, 3.0, 3.0);
}

fn draw_planter(context: &CanvasRenderingContext2d, x: f64, y: f64, palette: &Palette) {
    diamond(context, x, y, 11.0, 6.0, &palette.wood);
    context.set_fill_style_str(&palette.wood);
    context.fill_rect(x - 7.0, y, 14.0, 11.0);
    context.set_fill_style_str(&palette.plant);
    context.fill_rect(x - 2.0, y - 16.0, 5.0, 16.0);
    context.fill_rect(x - 8.0, y - 13.0, 7.0, 5.0);
    context.fill_rect(x + 2.0, y - 10.0, 8.0, 5.0);
}

fn draw_beacon(context: &CanvasRenderingContext2d, x: f64, y: f64, color: &str, palette: &Palette) {
    context.set_fill_style_str(&palette.ink);
    context.fill_rect(x - 4.0, y - 8.0, 10.0, 12.0);
    context.set_fill_style_str(&palette.metal_hi);
    context.fill_rect(x - 2.0, y - 7.0, 6.0, 8.0);
    context.set_fill_style_str(color);
    context.fill_rect(x - 1.0, y - 6.0, 4.0, 4.0);
}

fn draw_tethers(
    context: &CanvasRenderingContext2d,
    state: &ObservatoryState,
    layout: &FloorLayout,
    palette: &Palette,
) {
    let positions: std::collections::BTreeMap<_, _> = layout
        .stations
        .iter()
        .map(|station| {
            (
                station.execution_id.as_str(),
                (station.screen_x, station.screen_y),
            )
        })
        .collect();
    for edge in state.edges.values() {
        let (Some((from_x, from_y)), Some((to_x, to_y))) = (
            positions.get(edge.parent_execution_id.as_str()),
            positions.get(edge.child_execution_id.as_str()),
        ) else {
            continue;
        };
        let midpoint_y = ((*from_y + *to_y) / 2.0).round();
        let path = [
            (*from_x, *from_y + 22.0),
            (*from_x, midpoint_y),
            (*to_x, midpoint_y),
            (*to_x, *to_y + 22.0),
        ];
        context.set_line_width(5.0);
        polyline(context, &path, &palette.ink);
        context.set_line_width(2.0);
        polyline(context, &path, &palette.tether);
        context.set_fill_style_str(&palette.packet);
        context.fill_rect(*to_x - 2.0, *to_y + 18.0, 4.0, 4.0);
    }
}

fn draw_station(
    context: &CanvasRenderingContext2d,
    station: &StationLayout,
    node: &ExecutionState,
    selected: bool,
    integrity: IntegrityState,
    timestamp: f64,
    palette: &Palette,
) {
    let x = station.screen_x;
    let y = station.screen_y;
    let reduced_motion = prefers_reduced_motion();
    let faded = node.phase == ExecutionPhase::Canceled;
    if faded {
        context.set_global_alpha(0.5);
    }

    if node.permission_waiting {
        let pulse = if reduced_motion || ((timestamp / 400.0) as i32) & 1 == 0 {
            1.0
        } else {
            0.56
        };
        context.set_global_alpha(if faded { pulse * 0.5 } else { pulse });
        outline_diamond(context, x, y - 8.0, 72.0, 37.0, 5.0, &palette.amber);
        outline_diamond(context, x, y - 8.0, 64.0, 33.0, 2.0, &palette.red);
        context.set_global_alpha(if faded { 0.5 } else { 1.0 });
    }

    if selected {
        outline_diamond(context, x, y, 67.0, 34.0, 3.0, &palette.packet);
    }
    diamond(context, x + 4.0, y + 8.0, 57.0, 29.0, &palette.ink);
    diamond(context, x, y, 58.0, 29.0, &palette.metal);
    diamond(context, x, y - 3.0, 51.0, 25.0, &palette.metal_hi);

    draw_chair(context, x, y - 14.0, palette);
    draw_console(
        context,
        x,
        y - 45.0,
        node,
        timestamp,
        reduced_motion,
        palette,
    );
    draw_actor(context, x, y - 5.0, node, palette);
    draw_tool_ports(context, x, y, node, timestamp, reduced_motion, palette);

    if node.phase == ExecutionPhase::Finished {
        draw_completion_crate(context, x + 37.0, y - 7.0, palette);
    }

    if integrity != IntegrityState::Live {
        context.set_global_alpha(if faded { 0.28 } else { 0.46 });
        context.set_fill_style_str(&palette.ink);
        context.fill_rect(x - 58.0, y - 88.0, 116.0, 116.0);
        context.set_fill_style_str(&palette.rock_top);
        let mut veil_y = y - 86.0;
        while veil_y < y + 26.0 {
            let mut veil_x = x - 56.0;
            while veil_x < x + 56.0 {
                context.fill_rect(veil_x, veil_y, 2.0, 2.0);
                veil_x += 8.0;
            }
            veil_y += 8.0;
        }
        context.set_global_alpha(if faded { 0.5 } else { 1.0 });
    }

    context.set_global_alpha(1.0);
}

fn draw_chair(context: &CanvasRenderingContext2d, x: f64, y: f64, palette: &Palette) {
    context.set_fill_style_str(&palette.ink);
    context.fill_rect(x - 12.0, y - 21.0, 24.0, 24.0);
    context.set_fill_style_str(&palette.metal);
    context.fill_rect(x - 9.0, y - 19.0, 18.0, 19.0);
    context.set_fill_style_str(&palette.metal_hi);
    context.fill_rect(x - 7.0, y - 17.0, 3.0, 14.0);
    context.set_fill_style_str(&palette.ink);
    context.fill_rect(x - 3.0, y + 1.0, 6.0, 9.0);
}

fn draw_console(
    context: &CanvasRenderingContext2d,
    x: f64,
    y: f64,
    node: &ExecutionState,
    timestamp: f64,
    reduced_motion: bool,
    palette: &Palette,
) {
    // Raised CRT housing and an isometric keyboard deck.
    context.set_fill_style_str(&palette.ink);
    context.fill_rect(x - 25.0, y - 31.0, 52.0, 33.0);
    context.set_fill_style_str(&palette.metal);
    context.fill_rect(x - 23.0, y - 33.0, 46.0, 29.0);
    context.set_fill_style_str(&palette.metal_hi);
    context.fill_rect(x - 23.0, y - 33.0, 46.0, 4.0);
    context.fill_rect(x - 23.0, y - 29.0, 4.0, 25.0);

    let screen_color = if matches!(
        node.phase,
        ExecutionPhase::Running | ExecutionPhase::Admitted
    ) {
        &palette.screen_on
    } else {
        &palette.screen_off
    };
    let glow = if node.phase == ExecutionPhase::Running && !reduced_motion {
        0.66 + ((timestamp / 600.0).sin().abs() * 0.34)
    } else {
        1.0
    };
    context.set_global_alpha(glow);
    context.set_fill_style_str(screen_color);
    context.fill_rect(x - 16.0, y - 25.0, 32.0, 14.0);
    context.set_global_alpha(1.0);
    if node.phase == ExecutionPhase::Running {
        // Static traces communicate an active screen without inventing progress.
        // Only luminance pulses while the recorded running state holds.
        context.set_fill_style_str(&palette.packet);
        context.fill_rect(x - 12.0, y - 21.0, 22.0, 2.0);
        context.fill_rect(x - 12.0, y - 16.0, 14.0, 2.0);
        context.fill_rect(x + 5.0, y - 16.0, 5.0, 2.0);
    }

    let lamp = match node.phase {
        ExecutionPhase::Finished => &palette.green,
        ExecutionPhase::Error | ExecutionPhase::TimedOut => &palette.red,
        ExecutionPhase::Canceled => &palette.tether,
        ExecutionPhase::Running | ExecutionPhase::Admitted => &palette.amber,
    };
    context.set_fill_style_str(&palette.ink);
    context.fill_rect(x + 17.0, y - 31.0, 8.0, 8.0);
    context.set_fill_style_str(lamp);
    context.fill_rect(x + 19.0, y - 29.0, 4.0, 4.0);

    diamond(context, x, y + 2.0, 36.0, 18.0, &palette.desk_top);
    polygon(
        context,
        &[
            (x - 36.0, y + 2.0),
            (x, y + 20.0),
            (x, y + 27.0),
            (x - 36.0, y + 9.0),
        ],
        &palette.desk_side,
    );
    polygon(
        context,
        &[
            (x + 36.0, y + 2.0),
            (x, y + 20.0),
            (x, y + 27.0),
            (x + 36.0, y + 9.0),
        ],
        &palette.wall_dark,
    );
    context.set_fill_style_str(&palette.screen_off);
    for key_y in 0..2 {
        for key_x in 0..5 {
            context.fill_rect(
                x - 20.0 + f64::from(key_x) * 8.0 - f64::from(key_y) * 2.0,
                y + 1.0 + f64::from(key_y) * 5.0,
                5.0,
                2.0,
            );
        }
    }
}

fn draw_actor(
    context: &CanvasRenderingContext2d,
    x: f64,
    y: f64,
    node: &ExecutionState,
    palette: &Palette,
) {
    let slumped = matches!(node.phase, ExecutionPhase::Error | ExecutionPhase::TimedOut);
    let lean = node.phase == ExecutionPhase::Running;
    let offset_x = if lean { 2.0 } else { 0.0 };
    let offset_y = if slumped { 5.0 } else { 0.0 };

    // Boots and seated legs.
    context.set_fill_style_str(&palette.ink);
    context.fill_rect(x - 9.0 + offset_x, y - 4.0 + offset_y, 7.0, 12.0);
    context.fill_rect(x + 2.0 + offset_x, y - 4.0 + offset_y, 7.0, 12.0);
    context.set_fill_style_str(&palette.outfit);
    context.fill_rect(x - 11.0 + offset_x, y - 20.0 + offset_y, 22.0, 18.0);
    context.set_fill_style_str(&palette.metal_hi);
    context.fill_rect(x - 8.0 + offset_x, y - 17.0 + offset_y, 3.0, 11.0);

    // Head, hair, face, and one-pixel eyes keep the sprite readable at 50%.
    context.set_fill_style_str(&palette.ink);
    context.fill_rect(x - 8.0 + offset_x, y - 34.0 + offset_y, 16.0, 6.0);
    context.fill_rect(x - 9.0 + offset_x, y - 30.0 + offset_y, 18.0, 5.0);
    context.set_fill_style_str(&palette.skin);
    context.fill_rect(x - 7.0 + offset_x, y - 27.0 + offset_y, 14.0, 10.0);
    context.set_fill_style_str(&palette.ink);
    context.fill_rect(x - 4.0 + offset_x, y - 24.0 + offset_y, 2.0, 2.0);
    context.fill_rect(x + 3.0 + offset_x, y - 24.0 + offset_y, 2.0, 2.0);

    if node.permission_waiting {
        context.set_fill_style_str(&palette.outfit);
        context.fill_rect(x + 9.0, y - 31.0, 5.0, 16.0);
        context.set_fill_style_str(&palette.skin);
        context.fill_rect(x + 9.0, y - 39.0, 6.0, 9.0);
    } else if node.phase == ExecutionPhase::Finished {
        context.set_fill_style_str(&palette.skin);
        context.fill_rect(x + 8.0, y - 16.0, 9.0, 5.0);
        context.set_fill_style_str(&palette.green);
        context.fill_rect(x + 14.0, y - 18.0, 5.0, 8.0);
    } else {
        context.set_fill_style_str(&palette.skin);
        context.fill_rect(x - 15.0 + offset_x, y - 17.0 + offset_y, 6.0, 5.0);
        context.fill_rect(x + 9.0 + offset_x, y - 17.0 + offset_y, 6.0, 5.0);
    }
}

fn draw_tool_ports(
    context: &CanvasRenderingContext2d,
    x: f64,
    y: f64,
    node: &ExecutionState,
    timestamp: f64,
    reduced_motion: bool,
    palette: &Palette,
) {
    for (index, _) in node.tools.values().take(4).enumerate() {
        let side = if index % 2 == 0 { -1.0 } else { 1.0 };
        let row = (index / 2) as f64;
        let port_x = x + side * 52.0;
        let port_y = y - 18.0 + row * 22.0;
        diamond(context, port_x, port_y, 14.0, 7.0, &palette.ink);
        context.set_fill_style_str(&palette.metal_hi);
        context.fill_rect(port_x - 8.0, port_y - 13.0, 16.0, 12.0);
        context.set_fill_style_str(&palette.metal);
        context.fill_rect(port_x - 6.0, port_y - 11.0, 12.0, 8.0);
        let frame = if reduced_motion {
            index as i32 % 4
        } else {
            ((timestamp / 125.0) as i32 + index as i32) % 4
        };
        context.set_fill_style_str(&palette.screen_on);
        match frame {
            0 => context.fill_rect(port_x - 1.0, port_y - 10.0, 3.0, 3.0),
            1 => context.fill_rect(port_x + 2.0, port_y - 8.0, 3.0, 3.0),
            2 => context.fill_rect(port_x - 1.0, port_y - 6.0, 3.0, 3.0),
            _ => context.fill_rect(port_x - 4.0, port_y - 8.0, 3.0, 3.0),
        }
    }
}

fn draw_completion_crate(context: &CanvasRenderingContext2d, x: f64, y: f64, palette: &Palette) {
    diamond(context, x, y, 13.0, 7.0, &palette.green);
    polygon(
        context,
        &[
            (x - 13.0, y),
            (x, y + 7.0),
            (x, y + 19.0),
            (x - 13.0, y + 12.0),
        ],
        &palette.metal,
    );
    polygon(
        context,
        &[
            (x + 13.0, y),
            (x, y + 7.0),
            (x, y + 19.0),
            (x + 13.0, y + 12.0),
        ],
        &palette.ink,
    );
    context.set_fill_style_str(&palette.packet);
    context.fill_rect(x - 2.0, y + 9.0, 4.0, 4.0);
}

fn polygon(context: &CanvasRenderingContext2d, points: &[(f64, f64)], color: &str) {
    let Some((first_x, first_y)) = points.first().copied() else {
        return;
    };
    context.begin_path();
    context.move_to(first_x, first_y);
    for (x, y) in points.iter().copied().skip(1) {
        context.line_to(x, y);
    }
    context.close_path();
    context.set_fill_style_str(color);
    context.fill();
}

fn polyline(context: &CanvasRenderingContext2d, points: &[(f64, f64)], color: &str) {
    let Some((first_x, first_y)) = points.first().copied() else {
        return;
    };
    context.begin_path();
    context.move_to(first_x.round(), first_y.round());
    for (x, y) in points.iter().copied().skip(1) {
        context.line_to(x.round(), y.round());
    }
    context.set_stroke_style_str(color);
    context.stroke();
}

fn diamond(
    context: &CanvasRenderingContext2d,
    x: f64,
    y: f64,
    radius_x: f64,
    radius_y: f64,
    color: &str,
) {
    polygon(
        context,
        &[
            (x, y - radius_y),
            (x + radius_x, y),
            (x, y + radius_y),
            (x - radius_x, y),
        ],
        color,
    );
}

fn outline_diamond(
    context: &CanvasRenderingContext2d,
    x: f64,
    y: f64,
    radius_x: f64,
    radius_y: f64,
    width: f64,
    color: &str,
) {
    context.begin_path();
    context.move_to(x, y - radius_y);
    context.line_to(x + radius_x, y);
    context.line_to(x, y + radius_y);
    context.line_to(x - radius_x, y);
    context.close_path();
    context.set_stroke_style_str(color);
    context.set_line_width(width);
    context.stroke();
}

fn prefers_reduced_motion() -> bool {
    web_sys::window()
        .and_then(|window| window.match_media("(prefers-reduced-motion: reduce)").ok())
        .flatten()
        .map(|query| query.matches())
        .unwrap_or(false)
}
