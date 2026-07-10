//! Ocean Surface — Leptos entry.
//!
//! Mounts the App component to <body>. Both the browser PWA build (via
//! Trunk) and the future Tauri shell load this same binary; what changes
//! between them is just the host (browser vs WKWebView).
//!
//! Float mode: add `?float=1` (or `#float`) to the URL to activate the
//! lightweight floating chat corridor instead of the full cockpit.

use leptos::prelude::*;

mod app;
mod call;
mod canvas;
mod components;
mod council;
mod daemon;
mod deck;
mod host;
mod icons;
mod livekit;
mod loader;
mod markdown;
mod model;
mod palette;
mod place_call;
mod rooms;
mod sessions;
mod slash_menu;
mod transcript;
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
    if float_mode_active() {
        mount_to_body(FloatingApp);
    } else {
        mount_to_body(App);
    }
}
