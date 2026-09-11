//! Source guards for the desktop half of `ocean://` deep links.
//!
//! `parse_deep_link` is unit-tested in `app.rs` and holds what a URL MEANS.
//! Nothing in this crate holds whether a URL ever arrives: that depends on
//! three files in `crates/ocean-tauri`, none of which this crate compiles —
//! the scheme registration in `tauri.conf.json`, the `deep-link:default`
//! capability in `capabilities/default.json`, and the shell's `on_open_url`
//! handler that re-emits each URL to the webview as a `deep-link` event.
//!
//! Deleting any one of them leaves every gate in this repo green, `cargo
//! check` on the shell green, and the feature silently dead: the OS either
//! never launches Ocean for `ocean://…` at all, or launches it and the plugin
//! refuses the call, or the shell receives the URL and tells nobody. The
//! failure is indistinguishable from a mistyped link, which is why it needs a
//! scan and not a code review.

mod common;

use common::read;

#[test]
fn the_shell_registers_the_ocean_scheme_with_the_os() {
    let conf = read("crates/ocean-tauri/tauri.conf.json");
    let value: serde_json::Value =
        serde_json::from_str(&conf).expect("tauri.conf.json must be valid JSON");
    let schemes = value
        .pointer("/plugins/deep-link/desktop/schemes")
        .and_then(serde_json::Value::as_array)
        .expect(
            "tauri.conf.json must declare plugins.deep-link.desktop.schemes — \
             without it the bundled app is not the handler for ocean:// and the \
             OS never launches it for a link",
        );
    assert!(
        schemes.iter().any(|s| s.as_str() == Some("ocean")),
        "the `ocean` scheme must stay registered; every ocean://session and \
         ocean://room link depends on it",
    );
}

#[test]
fn the_main_window_keeps_the_deep_link_capability() {
    let caps = read("crates/ocean-tauri/capabilities/default.json");
    let value: serde_json::Value =
        serde_json::from_str(&caps).expect("capabilities/default.json must be valid JSON");
    let permissions = value
        .get("permissions")
        .and_then(serde_json::Value::as_array)
        .expect("the default capability must list permissions");
    assert!(
        permissions
            .iter()
            .any(|p| p.as_str() == Some("deep-link:default")),
        "the main window must keep `deep-link:default`; the plugin's commands \
         are ACL-gated (unlike generate_handler! commands), so removing it \
         leaves the scheme registered and the handler unreachable",
    );
    assert!(
        value
            .get("windows")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|windows| windows.iter().any(|w| w.as_str() == Some("main"))),
        "the capability must still apply to the `main` window — the only one \
         the bundle opens",
    );
}

#[test]
fn the_shell_forwards_every_opened_url_to_the_webview() {
    let shell = read("crates/ocean-tauri/src/lib.rs");
    assert!(
        shell.contains("app.deep_link().on_open_url("),
        "the shell must subscribe to opened URLs, or the OS hands Ocean a link \
         and nothing in the app ever hears about it",
    );
    assert!(
        shell.contains(r#"handle.emit("deep-link", url.to_string())"#),
        "each opened URL must be re-emitted as the `deep-link` event \
         host::on_deep_link subscribes to",
    );
    assert!(
        shell.contains("show_main_window(&handle);"),
        "an ocean:// link must bring the window forward; the menubar-app \
         pattern hides it on close, so a link to a hidden Ocean would open the \
         room behind an invisible window",
    );
}
