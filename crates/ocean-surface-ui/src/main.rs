//! Ocean Surface — Leptos entry.
//!
//! Mounts the App component to <body>. Both the browser PWA build (via
//! Trunk) and the future Tauri shell load this same binary; what changes
//! between them is just the host (browser vs WKWebView).
//!
//! Float mode: add `?float=1` (or `#float`) to the URL to activate the
//! lightweight floating chat corridor instead of the full cockpit.

use leptos::prelude::*;

mod agents;
mod app;
mod attachments;
mod call;
mod canvas;
mod components;
mod council;
mod daemon;
mod deck;
mod host;
mod icons;
mod island;
mod island_dynamic;
mod livekit;
mod loader;
mod markdown;
mod markdown_stream;
mod model;
mod observatory;
mod palette;
mod place_call;
mod room_artifacts;
mod room_invite;
mod room_markdown;
mod room_messages;
mod room_repo;
mod room_summary;
mod room_workspace_panel;
mod rooms;
mod rooms_workspace;
mod search;
mod sessions;
mod slash_menu;
mod transcript;
mod transcript_media;
mod tts;
mod voice;
mod widget;
mod workspace;
mod workspace_browser;

use app::App;
use widget::{float_mode_active, FloatingApp};

fn main() {
    console_error_panic_hook::set_once();
    _ = console_log::init_with_level(log::Level::Info);
    // Boot provenance: which commit is this bundle? (`OCEAN_SURFACE_REV` is
    // embedded by build.rs; also forces per-commit dist hashes — see build.rs.)
    log::info!("ocean-surface {}", env!("OCEAN_SURFACE_REV"));
    if float_mode_active() {
        mount_to_body(FloatingApp);
    } else {
        mount_to_body(App);
    }
}
