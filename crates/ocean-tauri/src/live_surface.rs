//! Live surface bundle — the desktop shell serves the promoted web release.
//!
//! WHY: `tauri::generate_context!()` embeds `../../dist` into the binary at
//! COMPILE time. The surface auto-deploy rail promotes every main move into
//! `~/.config/ocean-surface/current` for the browser within minutes, but the
//! packaged Ocean.app only changed when someone ran
//! `scripts/rebuild-tauri-app.sh` — which is how the desktop ran an August-3
//! bundle against a September-2 web surface and read as "not synced".
//!
//! This module makes the shell read the SAME promoted release the proxy
//! serves, from disk, at request time. The origin stays `tauri://localhost`
//! (daemon CORS and every `host.rs` seam are untouched); only where the bytes
//! come from changes. The compiled-in bundle remains the fallback for a
//! machine with no rail (teammate checkout, CI, fresh install), so nothing
//! regresses there.
//!
//! Resolution: `OCEAN_SURFACE_DIST` (the proxy's own variable, so one machine
//! configures both surfaces the same way; empty disables live serving) →
//! `$HOME/.config/ocean-surface/current` → embedded. A root counts as live
//! only while `<root>/index.html` is a regular file; the check runs per
//! request so a promote (an atomic symlink swap) is picked up on the next
//! reload with no restart. Assets are never mixed across sources: a key
//! missing from a live root yields `None`, so Tauri's own SPA fallback serves
//! the live `index.html` — never an embedded file from a different build.
//!
//! Trust note: the release directory is operator-owned and written by the
//! rail, exactly like the proxy's dist. Anything that can rewrite it can
//! already rewrite `/Applications/Ocean.app`; this is not a new boundary.

use std::borrow::Cow;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use tauri::utils::assets::{AssetKey, AssetsIter, CspHash};
use tauri::{AppHandle, Assets, Emitter, Manager, Wry};

/// How often the shell checks whether the promoted release changed.
const RELEASE_POLL_INTERVAL: Duration = Duration::from_secs(10);
/// Emitted to the webview when the promoted release changes underneath a
/// visible window (payload: `{ "revision": "<sha>" }`). A hidden window is
/// reloaded outright instead — nobody is looking at it.
const SURFACE_UPDATED_EVENT: &str = "surface-updated";

/// Where the promoted release lives. Pure so the precedence is unit-testable:
/// `OCEAN_SURFACE_DIST` set and non-empty → that path; set but empty → live
/// serving OFF (embedded only); unset → `$HOME/.config/ocean-surface/current`;
/// no `$HOME` either → embedded only.
fn release_root_from(dist_env: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    match dist_env {
        Some(v) if !v.is_empty() => Some(PathBuf::from(v)),
        Some(_) => None,
        None => home
            .filter(|h| !h.is_empty())
            .map(|h| PathBuf::from(h).join(".config/ocean-surface/current")),
    }
}

/// [`release_root_from`] against the process environment.
pub fn release_root_from_env() -> Option<PathBuf> {
    release_root_from(
        std::env::var_os("OCEAN_SURFACE_DIST").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

/// A root is live only while it holds a real `index.html` (symlinks followed —
/// `current` IS a symlink). Anything else falls back to the embedded bundle.
fn is_live_root(root: &Path) -> bool {
    root.join("index.html").is_file()
}

/// Map an asset key (`/index.html`, `/brand/x.png`) onto a path INSIDE the
/// root. Every component must be a plain name: `..`, a root, a prefix, or an
/// empty key is refused so no request can reach outside the release dir even
/// though the WebView already normalizes URLs before they arrive here.
fn release_asset_path(root: &Path, key: &str) -> Option<PathBuf> {
    let rel = key.trim_start_matches('/');
    if rel.is_empty() {
        return None;
    }
    let rel = Path::new(rel);
    if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(root.join(rel))
}

/// The identity of the bundle at `root`: the rail's `.deploy-sha` marker when
/// present (every promoted release carries one), else the `index.html`
/// modification time. `None` when the root is not live.
fn bundle_identity(root: &Path) -> Option<String> {
    if !is_live_root(root) {
        return None;
    }
    if let Ok(sha) = std::fs::read_to_string(root.join(".deploy-sha")) {
        let sha = sha.trim();
        if !sha.is_empty() {
            return Some(sha.to_owned());
        }
    }
    let modified = std::fs::metadata(root.join("index.html"))
        .and_then(|m| m.modified())
        .ok()?;
    let secs = modified
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(format!("mtime:{secs}"))
}

/// Recursively list the files under `root` as asset keys (`/a/b.css`).
fn walk_release(root: &Path) -> Vec<(String, PathBuf)> {
    fn visit(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, out);
            } else if path.is_file() {
                if let Ok(rel) = path.strip_prefix(root) {
                    let key = format!("/{}", rel.to_string_lossy().replace('\\', "/"));
                    out.push((key, path));
                }
            }
        }
    }
    let mut out = Vec::new();
    visit(root, root, &mut out);
    out.sort();
    out
}

/// Serves the promoted release from disk, falling back to the compiled-in
/// bundle when no live root is usable.
pub struct LiveAssets {
    root: Option<PathBuf>,
    embedded: Box<dyn Assets<Wry>>,
}

impl LiveAssets {
    pub fn new(root: Option<PathBuf>, embedded: Box<dyn Assets<Wry>>) -> Self {
        Self { root, embedded }
    }

    /// The live root, re-validated on every call so a promote or a removed
    /// state dir takes effect without a restart.
    fn live_root(&self) -> Option<&Path> {
        self.root.as_deref().filter(|root| is_live_root(root))
    }
}

impl Assets<Wry> for LiveAssets {
    fn get(&self, key: &AssetKey) -> Option<Cow<'_, [u8]>> {
        match self.live_root() {
            // Live root: serve ONLY from it. A miss is a miss — Tauri's SPA
            // fallback then serves the live index.html, never an embedded file.
            Some(root) => {
                let path = release_asset_path(root, key.as_ref())?;
                if !path.is_file() {
                    return None;
                }
                std::fs::read(path).ok().map(Cow::Owned)
            }
            None => self.embedded.get(key),
        }
    }

    fn iter(&self) -> Box<AssetsIter<'_>> {
        match self.live_root() {
            Some(root) => {
                let entries: Vec<(Cow<'static, str>, Cow<'static, [u8]>)> = walk_release(root)
                    .into_iter()
                    .filter_map(|(key, path)| {
                        std::fs::read(path)
                            .ok()
                            .map(|bytes| (Cow::Owned(key), Cow::Owned(bytes)))
                    })
                    .collect();
                Box::new(entries.into_iter())
            }
            None => self.embedded.iter(),
        }
    }

    /// `tauri.conf.json` ships `csp: null`, so no hashes are consulted. If a
    /// CSP is ever configured, inline-script hashes for the LIVE bundle must
    /// be computed here — the embedded ones describe a different index.html.
    fn csp_hashes(&self, html_path: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        match self.live_root() {
            Some(_) => Box::new(std::iter::empty()),
            None => self.embedded.csp_hashes(html_path),
        }
    }
}

/// Placeholder used only to take the embedded assets OUT of the generated
/// context (`Context::set_assets` swaps and returns the previous value).
struct NoAssets;

impl Assets<Wry> for NoAssets {
    fn get(&self, _key: &AssetKey) -> Option<Cow<'_, [u8]>> {
        None
    }

    fn iter(&self) -> Box<AssetsIter<'_>> {
        Box::new(std::iter::empty())
    }

    fn csp_hashes(&self, _html_path: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        Box::new(std::iter::empty())
    }
}

/// Wrap the generated context's embedded bundle in [`LiveAssets`]. Logs which
/// source will answer the first request so a launch transcript says whether
/// the desktop is on the promoted release or the compiled-in fallback.
pub fn context_with_live_assets(mut context: tauri::Context<Wry>) -> tauri::Context<Wry> {
    let root = release_root_from_env();
    match root.as_deref() {
        Some(r) if is_live_root(r) => eprintln!(
            "[ocean-tauri] surface bundle: LIVE from {} (rev {})",
            r.display(),
            bundle_identity(r).unwrap_or_default()
        ),
        Some(r) => eprintln!(
            "[ocean-tauri] surface bundle: EMBEDDED (no index.html at {})",
            r.display()
        ),
        None => eprintln!("[ocean-tauri] surface bundle: EMBEDDED (live serving disabled)"),
    }
    let embedded = context.set_assets(Box::new(NoAssets));
    context.set_assets(Box::new(LiveAssets::new(root, embedded)));
    context
}

/// Reload the main webview — the same page, re-read from whichever source is
/// current. Bound to Cmd+R (File ▸ Reload Surface) and the tray, and used by
/// the release watcher for a hidden window.
pub fn reload_surface(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if let Err(error) = window.eval("window.location.reload()") {
            eprintln!("[ocean-tauri] reload surface: {error}");
        }
    }
}

/// Which bundle the shell is serving, for the surface's own diagnostics.
#[derive(Clone, Serialize)]
pub struct SurfaceBundleDto {
    /// `live` (promoted release on disk) or `embedded` (compiled-in fallback).
    source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    revision: Option<String>,
}

fn surface_bundle_snapshot(root: Option<&Path>) -> SurfaceBundleDto {
    match root {
        Some(r) if is_live_root(r) => SurfaceBundleDto {
            source: "live",
            root: Some(r.display().to_string()),
            revision: bundle_identity(r),
        },
        _ => SurfaceBundleDto {
            source: "embedded",
            root: None,
            revision: None,
        },
    }
}

/// `surface_bundle` command: report the live/embedded source and revision.
/// Read-only; takes nothing from the caller.
#[tauri::command]
pub fn surface_bundle() -> SurfaceBundleDto {
    surface_bundle_snapshot(release_root_from_env().as_deref())
}

/// Pure decision for one watcher tick: given the previously seen identity and
/// the one just observed, is there a promote to act on? The first observation
/// only records; a root that stops being live is not a promote.
fn release_changed(previous: &Option<String>, observed: &Option<String>) -> bool {
    matches!((previous, observed), (Some(p), Some(o)) if p != o)
}

/// Background watcher: notices a promote of the release root and either
/// reloads a HIDDEN main window outright or emits [`SURFACE_UPDATED_EVENT`]
/// to a visible one (reloading under the operator's hands is hostile — the
/// surface decides how to offer it). Replaces the rail's old "restart Tauri"
/// step, which relaunched a binary whose bundle could not change anyway.
pub fn spawn_release_watcher(app: AppHandle) {
    let Some(root) = release_root_from_env() else {
        return;
    };
    thread::spawn(move || {
        let mut last = bundle_identity(&root);
        loop {
            thread::sleep(RELEASE_POLL_INTERVAL);
            let observed = bundle_identity(&root);
            if release_changed(&last, &observed) {
                let revision = observed.clone().unwrap_or_default();
                eprintln!("[ocean-tauri] surface bundle promoted: {revision}");
                let hidden = app
                    .get_webview_window("main")
                    .map(|w| !w.is_visible().unwrap_or(true))
                    .unwrap_or(false);
                if hidden {
                    reload_surface(&app);
                } else {
                    let _ = app.emit(
                        SURFACE_UPDATED_EVENT,
                        serde_json::json!({ "revision": revision }),
                    );
                }
            }
            if observed.is_some() {
                last = observed;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    struct StubEmbedded;

    impl Assets<Wry> for StubEmbedded {
        fn get(&self, key: &AssetKey) -> Option<Cow<'_, [u8]>> {
            (key.as_ref() == "/embedded.css").then(|| Cow::Borrowed(b"embedded".as_slice()))
        }
        fn iter(&self) -> Box<AssetsIter<'_>> {
            Box::new(std::iter::once((
                Cow::Borrowed("/embedded.css"),
                Cow::Borrowed(b"embedded".as_slice()),
            )))
        }
        fn csp_hashes(&self, _: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
            Box::new(std::iter::empty())
        }
    }

    #[test]
    fn release_root_precedence_dist_env_then_home_then_none() {
        let dist = OsStr::new("/srv/ocean/current");
        let home = OsStr::new("/Users/op");
        assert_eq!(
            release_root_from(Some(dist), Some(home)),
            Some(PathBuf::from("/srv/ocean/current"))
        );
        assert_eq!(
            release_root_from(None, Some(home)),
            Some(PathBuf::from("/Users/op/.config/ocean-surface/current"))
        );
        // Explicitly empty = live serving OFF, even with a home.
        assert_eq!(release_root_from(Some(OsStr::new("")), Some(home)), None);
        assert_eq!(release_root_from(None, None), None);
        assert_eq!(release_root_from(None, Some(OsStr::new(""))), None);
    }

    #[test]
    fn asset_path_refuses_escapes_and_empty_keys() {
        let root = Path::new("/rel");
        assert_eq!(
            release_asset_path(root, "/index.html"),
            Some(PathBuf::from("/rel/index.html"))
        );
        assert_eq!(
            release_asset_path(root, "brand/master.png"),
            Some(PathBuf::from("/rel/brand/master.png"))
        );
        // `Path::components` drops a `.` segment, so this is a plain in-root
        // path, not an escape — it must stay inside the release dir.
        assert_eq!(
            release_asset_path(root, "a/./b"),
            Some(PathBuf::from("/rel/a/b"))
        );
        for hostile in ["", "/", "../x", "/../x", "a/../../b", "/etc/passwd/../x"] {
            assert_eq!(release_asset_path(root, hostile), None, "{hostile}");
        }
    }

    #[test]
    fn live_root_requires_a_real_index_html() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_live_root(dir.path()));
        std::fs::create_dir_all(dir.path().join("index.html")).unwrap();
        assert!(
            !is_live_root(dir.path()),
            "a directory named index.html is not a bundle"
        );
        std::fs::remove_dir(dir.path().join("index.html")).unwrap();
        write(dir.path(), "index.html", "<html>");
        assert!(is_live_root(dir.path()));
    }

    #[test]
    fn live_assets_serve_the_root_and_never_mix_in_embedded_files() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "index.html", "<html>live</html>");
        write(dir.path(), "styles/a.css", "live-css");
        let assets = LiveAssets::new(Some(dir.path().to_path_buf()), Box::new(StubEmbedded));

        let got = assets.get(&AssetKey::from("index.html")).unwrap();
        assert_eq!(&*got, b"<html>live</html>");
        let got = assets.get(&AssetKey::from("styles/a.css")).unwrap();
        assert_eq!(&*got, b"live-css");
        // Present only in the embedded bundle: a miss, so Tauri's SPA
        // fallback stays inside the live release.
        assert!(assets.get(&AssetKey::from("embedded.css")).is_none());
        assert!(assets.get(&AssetKey::from("missing.js")).is_none());

        let keys: Vec<String> = assets.iter().map(|(k, _)| k.into_owned()).collect();
        assert_eq!(keys, vec!["/index.html", "/styles/a.css"]);
    }

    #[test]
    fn live_assets_fall_back_to_embedded_without_a_live_root() {
        let dir = tempfile::tempdir().unwrap(); // no index.html → not live
        for root in [None, Some(dir.path().to_path_buf())] {
            let assets = LiveAssets::new(root, Box::new(StubEmbedded));
            let got = assets.get(&AssetKey::from("embedded.css")).unwrap();
            assert_eq!(&*got, b"embedded");
            assert!(assets.get(&AssetKey::from("index.html")).is_none());
            assert_eq!(assets.iter().count(), 1);
        }
    }

    #[test]
    fn bundle_identity_prefers_the_deploy_sha_marker() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(bundle_identity(dir.path()), None);
        write(dir.path(), "index.html", "<html>");
        assert!(bundle_identity(dir.path()).unwrap().starts_with("mtime:"));
        write(dir.path(), ".deploy-sha", "04d7323\n");
        assert_eq!(bundle_identity(dir.path()).as_deref(), Some("04d7323"));
    }

    #[test]
    fn surface_bundle_reports_live_or_embedded() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(surface_bundle_snapshot(Some(dir.path())).source, "embedded");
        assert_eq!(surface_bundle_snapshot(None).source, "embedded");
        write(dir.path(), "index.html", "<html>");
        write(dir.path(), ".deploy-sha", "abc123");
        let dto = surface_bundle_snapshot(Some(dir.path()));
        assert_eq!(dto.source, "live");
        assert_eq!(dto.revision.as_deref(), Some("abc123"));
        assert!(dto.root.is_some());
    }

    #[test]
    fn release_change_needs_two_live_observations_that_differ() {
        let a = Some("a".to_string());
        let b = Some("b".to_string());
        assert!(
            !release_changed(&None, &a),
            "first observation only records"
        );
        assert!(
            !release_changed(&a, &None),
            "root going away is not a promote"
        );
        assert!(!release_changed(&a, &a));
        assert!(release_changed(&a, &b));
    }
}
