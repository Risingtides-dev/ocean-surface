//! Swipe deck — an agent-dealt decision queue (protocol kind: `"deck"`).
//!
//! The agent deals a stack of cards; the operator judges each one with a
//! horizontal swipe (or the fallback action buttons). Left/right mean
//! whatever the deck's `actions` props say — archive/prioritize,
//! reject/approve, forget/retain — so one component covers every triage
//! situation. A card expands in place (tap) to reveal `detail_md` before
//! judging.
//!
//! Props shape:
//! ```json
//! { "title": "Inbox triage",
//!   "actions": { "left":  { "label": "Archive",    "variant": "danger"  },
//!                "right": { "label": "Prioritize", "variant": "success" } },
//!   "cards": [ { "id": "em-1", "badge": "urgent", "title": "…",
//!                "summary": "…", "detail_md": "…", "suggested": "right" } ] }
//! ```
//!
//! Events relayed to the agent (all `component_wait`-able):
//! - `card_swiped`   `{ card_id, direction, action_label }` — one verdict.
//! - `card_expanded` `{ card_id }` — operator opened the detail view.
//! - `deck_drained`  `{ judged: [{ card_id, direction }] }` — every card
//!   judged; the batch verdict in deal order.
//!
//! NOT related to the Context Deck side panels (`src/deck/`) — all class
//! names here are `swipedeck-*` to stay clear of `deck.css`.

use leptos::prelude::*;
use serde_json::{json, Value};
use wasm_bindgen::JsCast;

use crate::daemon::Daemon;

/// Horizontal drag distance (px) that commits a verdict on release.
const SWIPE_THRESHOLD: f64 = 90.0;
/// Depth cards rendered behind the top card.
const MAX_BEHIND: usize = 2;
/// Pointer travel (px) under which a release still counts as a tap.
const TAP_SLOP: f64 = 6.0;

/// One card, parsed out of the props JSON.
#[derive(Clone, PartialEq)]
struct DeckCard {
    id: String,
    badge: String,
    title: String,
    summary: String,
    detail_md: String,
    /// Agent's ghosted recommendation: "left" | "right" | "".
    suggested: String,
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

fn parse_cards(props: &Value) -> Vec<DeckCard> {
    props
        .get("cards")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .enumerate()
                .map(|(i, c)| {
                    let mut id = str_of(c, "id");
                    if id.is_empty() {
                        id = format!("card-{i}");
                    }
                    DeckCard {
                        id,
                        badge: str_of(c, "badge"),
                        title: str_of(c, "title"),
                        summary: str_of(c, "summary"),
                        detail_md: str_of(c, "detail_md"),
                        suggested: str_of(c, "suggested"),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `(label, variant)` for one swipe direction, with defaults.
fn parse_action(props: &Value, dir: &str, default_label: &str, default_variant: &str) -> (String, String) {
    let a = props.get("actions").and_then(|a| a.get(dir));
    let label = a
        .and_then(|v| v.get("label"))
        .and_then(|v| v.as_str())
        .unwrap_or(default_label)
        .to_string();
    let variant = a
        .and_then(|v| v.get("variant"))
        .and_then(|v| v.as_str())
        .unwrap_or(default_variant)
        .to_string();
    (label, variant)
}

/// A swipeable decision-queue card stack. See module docs for the contract.
#[component]
pub fn SwipeDeckView(component_id: String, kind_props: Value, daemon: Daemon) -> impl IntoView {
    let title = str_of(&kind_props, "title");
    let (left_label, left_variant) = parse_action(&kind_props, "left", "Dismiss", "danger");
    let (right_label, right_variant) = parse_action(&kind_props, "right", "Accept", "success");
    let cards = parse_cards(&kind_props);
    let total = cards.len();

    // (card_id, direction) verdicts in deal order; length doubles as the
    // index of the current top card.
    let judged: RwSignal<Vec<(String, String)>> = RwSignal::new(Vec::new());
    let expanded = RwSignal::new(false);
    let dragging = RwSignal::new(false);
    let drag_x = RwSignal::new(0.0_f64);
    let start_x = RwSignal::new(0.0_f64);
    // |drag| at the last pointerup, so the click handler can tell a tap from
    // the tail end of a drag (pointerup resets drag_x before click fires).
    let last_travel = RwSignal::new(0.0_f64);
    let drained = RwSignal::new(false);

    // Commit a verdict on the current top card and advance the deck.
    let judge = {
        let cards = cards.clone();
        let daemon = daemon.clone();
        let component_id = component_id.clone();
        let left_label = left_label.clone();
        let right_label = right_label.clone();
        move |direction: &str| {
            if drained.get_untracked() {
                return;
            }
            let mut done = judged.get_untracked();
            let Some(card) = cards.get(done.len()) else {
                return;
            };
            let action_label = if direction == "left" {
                left_label.clone()
            } else {
                right_label.clone()
            };
            daemon.send_component_event(
                component_id.clone(),
                json!({
                    "type": "card_swiped",
                    "payload": {
                        "card_id": card.id,
                        "direction": direction,
                        "action_label": action_label,
                    }
                }),
            );
            done.push((card.id.clone(), direction.to_string()));
            let all_judged = done.len() == cards.len();
            judged.set(done.clone());
            drag_x.set(0.0);
            dragging.set(false);
            expanded.set(false);
            if all_judged {
                drained.set(true);
                let verdicts: Vec<Value> = done
                    .iter()
                    .map(|(id, dir)| json!({ "card_id": id, "direction": dir }))
                    .collect();
                daemon.send_component_event(
                    component_id.clone(),
                    json!({ "type": "deck_drained", "payload": { "judged": verdicts } }),
                );
            }
        }
    };

    // Tap-to-expand relays card_expanded once per open.
    let toggle_expand = {
        let cards = cards.clone();
        let daemon = daemon.clone();
        let component_id = component_id.clone();
        move || {
            let idx = judged.get_untracked().len();
            let Some(card) = cards.get(idx) else { return };
            let now_open = !expanded.get_untracked();
            expanded.set(now_open);
            if now_open {
                daemon.send_component_event(
                    component_id.clone(),
                    json!({ "type": "card_expanded", "payload": { "card_id": card.id } }),
                );
            }
        }
    };

    let counter = move || {
        let done = judged.get().len();
        format!("{} / {}", done.min(total), total)
    };

    // The card stack: depth cards behind (static), top card interactive.
    let stack = {
        let cards = cards.clone();
        let judge = judge.clone();
        let toggle_expand = toggle_expand.clone();
        let left_label = left_label.clone();
        let right_label = right_label.clone();
        move || {
            let idx = judged.get().len();
            if total == 0 {
                return view! { <div class="swipedeck-empty">"No cards dealt."</div> }.into_any();
            }
            if idx >= total {
                return view! {
                    <div class="swipedeck-empty swipedeck-empty--drained">
                        {format!("All {total} judged")}
                    </div>
                }
                .into_any();
            }

            // Behind cards, deepest first so the DOM stacks naturally.
            let behind: Vec<_> = (1..=MAX_BEHIND)
                .rev()
                .filter_map(|depth| cards.get(idx + depth).cloned().map(|c| (depth, c)))
                .map(|(depth, card)| {
                    view! {
                        <div
                            class="swipedeck-card swipedeck-card--behind"
                            style=format!(
                                "transform: translateY({}px) scale({}); z-index: {};",
                                depth * 10,
                                1.0 - depth as f64 * 0.045,
                                MAX_BEHIND + 1 - depth,
                            )
                        >
                            <div class="swipedeck-card__title">{card.title.clone()}</div>
                        </div>
                    }
                })
                .collect();

            let card = cards[idx].clone();
            let card_style = move || {
                let x = drag_x.get();
                let rot = (x / 18.0).clamp(-12.0, 12.0);
                let transition = if dragging.get() { "none" } else { "transform 0.25s ease" };
                format!(
                    "transform: translateX({x}px) rotate({rot}deg); transition: {transition}; z-index: {};",
                    MAX_BEHIND + 2,
                )
            };
            let left_hint_style = move || {
                let x = drag_x.get();
                let o = ((-x - 16.0) / SWIPE_THRESHOLD).clamp(0.0, 1.0);
                format!("opacity: {o};")
            };
            let right_hint_style = move || {
                let x = drag_x.get();
                let o = ((x - 16.0) / SWIPE_THRESHOLD).clamp(0.0, 1.0);
                format!("opacity: {o};")
            };

            let on_down = move |ev: web_sys::PointerEvent| {
                dragging.set(true);
                start_x.set(ev.client_x() as f64);
                if let Some(el) = ev
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                {
                    let _ = el.set_pointer_capture(ev.pointer_id());
                }
            };
            let on_move = move |ev: web_sys::PointerEvent| {
                if dragging.get_untracked() {
                    drag_x.set(ev.client_x() as f64 - start_x.get_untracked());
                }
            };
            let judge_up = judge.clone();
            let on_up = move |_ev: web_sys::PointerEvent| {
                if !dragging.get_untracked() {
                    return;
                }
                dragging.set(false);
                let x = drag_x.get_untracked();
                last_travel.set(x.abs());
                if x <= -SWIPE_THRESHOLD {
                    judge_up("left");
                } else if x >= SWIPE_THRESHOLD {
                    judge_up("right");
                } else {
                    drag_x.set(0.0);
                }
            };
            let on_cancel = move |_ev: web_sys::PointerEvent| {
                dragging.set(false);
                drag_x.set(0.0);
            };
            let expand_click = toggle_expand.clone();
            let on_click = move |_| {
                if last_travel.get_untracked() < TAP_SLOP {
                    expand_click();
                }
            };

            let suggest_chip = {
                let s = card.suggested.clone();
                let ll = left_label.clone();
                let rl = right_label.clone();
                match s.as_str() {
                    "left" => Some(format!("suggests ← {ll}")),
                    "right" => Some(format!("suggests {rl} →")),
                    _ => None,
                }
            };
            let detail = card.detail_md.clone();
            let badge = card.badge.clone();

            view! {
                <div class="swipedeck-stack">
                    {behind}
                    <div
                        class="swipedeck-card swipedeck-card--top"
                        class:swipedeck-card--expanded=move || expanded.get()
                        style=card_style
                        on:pointerdown=on_down
                        on:pointermove=on_move
                        on:pointerup=on_up
                        on:pointercancel=on_cancel
                        on:click=on_click
                        role="group"
                        aria-label=format!("card {} of {}: {}", idx + 1, total, card.title)
                    >
                        <div class="swipedeck-card__hint swipedeck-card__hint--left" style=left_hint_style>
                            {left_label.clone()}
                        </div>
                        <div class="swipedeck-card__hint swipedeck-card__hint--right" style=right_hint_style>
                            {right_label.clone()}
                        </div>
                        <div class="swipedeck-card__head">
                            {(!badge.is_empty()).then(|| view! {
                                <span class=format!("swipedeck-card__badge swipedeck-card__badge--{badge}")>
                                    {badge.clone()}
                                </span>
                            })}
                            {suggest_chip.map(|s| view! {
                                <span class="swipedeck-card__suggest">{s}</span>
                            })}
                        </div>
                        <div class="swipedeck-card__title">{card.title.clone()}</div>
                        {(!card.summary.is_empty()).then(|| view! {
                            <div class="swipedeck-card__summary">{card.summary.clone()}</div>
                        })}
                        {move || {
                            (expanded.get() && !detail.is_empty()).then(|| view! {
                                <div
                                    class="swipedeck-card__detail"
                                    inner_html=crate::markdown::render(&detail)
                                    on:click=crate::host::open_external_link_click
                                ></div>
                            })
                        }}
                        {(!card.detail_md.is_empty()).then(|| view! {
                            <div class="swipedeck-card__expand-hint">
                                {move || if expanded.get() { "tap to collapse" } else { "tap for detail" }}
                            </div>
                        })}
                    </div>
                </div>
            }
            .into_any()
        }
    };

    let judge_left = judge.clone();
    let judge_right = judge.clone();
    let buttons_disabled = move || drained.get() || total == 0;

    view! {
        <div class="component-swipedeck">
            <div class="swipedeck-head">
                {(!title.is_empty()).then(|| view! { <span class="swipedeck-head__title">{title.clone()}</span> })}
                <span class="swipedeck-head__counter">{counter}</span>
            </div>
            <div class="swipedeck-stage">{stack}</div>
            <div class="swipedeck-actions">
                <button
                    class=format!("swipedeck-btn swipedeck-btn--{left_variant}")
                    type="button"
                    prop:disabled=buttons_disabled
                    on:click=move |_| judge_left("left")
                >
                    {format!("← {left_label}")}
                </button>
                <button
                    class=format!("swipedeck-btn swipedeck-btn--{right_variant}")
                    type="button"
                    prop:disabled=buttons_disabled
                    on:click=move |_| judge_right("right")
                >
                    {format!("{right_label} →")}
                </button>
            </div>
        </div>
    }
}
