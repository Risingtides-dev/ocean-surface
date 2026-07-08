//! Inline SVG icons (game-icons.net, CC-BY 3.0). Each renders at 1em and
//! inherits `currentColor`, so CSS color controls them — they match the
//! site palette (cyan --accent, etc.) wherever they're placed.

use leptos::prelude::*;

/// The Ocean badge — the brand mark: a circular emblem holding the depth
/// ramp top-to-bottom (sunlit dome arc → two drifting swells → the solid
/// deep-water wave → the abyss bar). Hand-drawn on a 48×48 grid, clipped to
/// the circle; every element takes its color from a `--ocean-*` ramp token
/// via CSS (`.wave-badge__*`), never inline. With `spinning=true` the
/// swells drift at parallax speeds — the loading state. Waves repeat every
/// 12 units and the drift translates exactly one period, so the loop is
/// seamless.
#[component]
pub fn WaveBadge(#[prop(optional)] spinning: bool) -> impl IntoView {
    let class = if spinning { "wave-badge is-spinning" } else { "wave-badge" };
    view! {
        <svg class=class viewBox="0 0 48 48" width="1em" height="1em"
             fill="none" aria-hidden="true">
            <defs>
                <clipPath id="wave-badge-clip">
                    <circle cx="24" cy="24" r="20.4" />
                </clipPath>
            </defs>
            // The coin: a matte face DARKER than the page, with a thin rim
            // relief — the mark is embossed into a badge, not floating
            // strokes. Geometry + colors are pixel-measured from
            // docs/assets/ocean-logo-circular-neumorphic-reference.png.
            <circle class="wave-badge__face" cx="24" cy="24" r="21" />
            <circle class="wave-badge__ring" cx="24" cy="24" r="20.6" />
            <g clip-path="url(#wave-badge-clip)" stroke-linecap="round">
                // The dome runs nearly CONCENTRIC with the rim (R≈20.9 vs
                // rim 21): apex centerline y6.6, legs dive to (8.5,13.5) /
                // (39.5,13.5) — a wide canopy hugging the circle, not a
                // small floating cap.
                <path class="wave-badge__arc" d="M 8.5 13.5 A 20.9 20.9 0 0 1 39.5 13.5" />
                // Swells: gentle — crest-to-trough 1.9 (4.5% of the face),
                // period 22, centerlines y18.4 / y27.1. Drawn one period
                // past each edge so the 22-unit drift loops seamlessly.
                <path class="wave-badge__w1" d="M -22 18.4 q 5.5 -1.9 11 0 t 11 0 t 11 0 t 11 0 t 11 0 t 11 0 t 11 0 t 11 0" />
                <path class="wave-badge__w2" d="M -22 27.1 q 5.5 -1.9 11 0 t 11 0 t 11 0 t 11 0 t 11 0 t 11 0 t 11 0 t 11 0" />
                // Deep-water band: thicker than the swells (top y33.6 →
                // bottom y39.6), half-period phase offset (inverted bulge).
                <path class="wave-badge__fill" d="M -22 33.6 q 5.5 1.7 11 0 t 11 0 t 11 0 t 11 0 t 11 0 t 11 0 t 11 0 t 11 0 L 66 39.6 L -22 39.6 Z" />
                // Abyss bar: bottom chord hugging the rim (y43.75, ~40% of
                // the face wide).
                <path class="wave-badge__bar" d="M 15.5 43.75 L 32.5 43.75" />
            </g>
        </svg>
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
