//! Repo panel — local-first repo state (branch, dirty/staged, ahead/behind,
//! recent commits) via crate::host::repo_state, refreshed on .git changes.
//! Owned by the RepoPanel workstream (feat/desktop-repo-panel).
//!
//! Root resolution: the active session's workspace root from
//! `daemon.cwd` (RwSignal<String>) — same source FilesPanel uses.

use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::daemon::Daemon;
use crate::host::{self, RepoState};

// ---------------------------------------------------------------------------
// Pure helpers — unit-testable without WASM
// ---------------------------------------------------------------------------

/// Format a Unix epoch timestamp as a compact relative-time string.
///
/// `epoch` is seconds since the Unix epoch (as emitted by git2).
/// `now` is the reference time provided by the caller.
///
/// Buckets: just now (0–59s), Xm ago, Xh ago, Xd ago, Xw ago, Xmo ago, Xy ago.
fn relative_time(epoch: i64, now: i64) -> String {
    if epoch <= 0 || now <= 0 {
        return "unknown".into();
    }
    let diff = now.saturating_sub(epoch);
    if diff < 60 {
        "just now".into()
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else if diff < 604800 {
        format!("{}d ago", diff / 86400)
    } else if diff < 2419200 {
        format!("{}w ago", diff / 604800)
    } else if diff < 31536000 {
        // 12 thirty-day months < one 365-day year: clamp so diffs in
        // [360d, 365d) read "11mo ago", never "12mo ago".
        format!("{}mo ago", (diff / 2592000).min(11))
    } else {
        format!("{}y ago", diff / 31536000)
    }
}

/// Format ahead/behind counts as badges — `None` when zero so the view
/// renders nothing (nonzero → rendered, zero → hidden).
fn format_ahead(ahead: usize) -> Option<String> {
    if ahead > 0 {
        Some(format!("\u{2191}{}", ahead))
    } else {
        None
    }
}

fn format_behind(behind: usize) -> Option<String> {
    if behind > 0 {
        Some(format!("\u{2193}{}", behind))
    } else {
        None
    }
}

/// Selection logic: should the panel render repo content or an empty state?
#[derive(Debug, Clone)]
enum PanelMode {
    /// Not running in Tauri — render the "native shell only" message.
    NotTauri,
    /// Tauri, but no repo found at the current root (or still loading).
    NoRepo(Option<String>),
    /// Tauri + valid repo state.
    Repo(RepoState),
}

fn classify(is_tauri: bool, state: &Option<RepoState>, error: &Option<String>) -> PanelMode {
    if !is_tauri {
        return PanelMode::NotTauri;
    }
    match state {
        Some(repo) if !repo.branch.is_empty() => PanelMode::Repo(repo.clone()),
        _ => PanelMode::NoRepo(error.clone()),
    }
}

// ---------------------------------------------------------------------------
// RepoPanel component
// ---------------------------------------------------------------------------

/// Repo panel — local-first repo state for the active session's workspace.
///
/// Data flow:
/// 1. Root = `daemon.cwd` (RwSignal<String>) — the active session's workspace root.
/// 2. On mount + on root change: `host::repo_state(&root)`.
/// 3. Live refresh (Tauri only): `watch_paths([root/.git])` → `on_path_changed`
///    re-fetches repo_state (latest-wins, no queue).
/// 4. Poll fallback OFF — event-driven only.
/// 5. Non-Tauri: renders a quiet "native shell only" empty state.
#[component]
pub fn RepoPanel(daemon: Daemon) -> impl IntoView {
    let state: RwSignal<Option<RepoState>> = RwSignal::new(None);
    let error: RwSignal<Option<String>> = RwSignal::new(None);

    // Snapshot of the current time for relative-time display. Stays frozen
    // after mount — relative labels ("3m ago") do not need live ticking.
    let now: i64 = (js_sys::Date::now() as i64) / 1000;

    // Fetch + watch whenever the active session's root changes.
    // Effect runs immediately (initial fetch) and re-runs when daemon.cwd changes.
    Effect::new(move |_| {
        let root = daemon.cwd.get();
        if root.trim().is_empty() {
            return;
        }

        let state = state;
        let error = error;
        let root_for_fetch = root.clone();
        let root_for_watch = root_for_fetch.clone();

        spawn_local(async move {
            match host::repo_state(&root_for_fetch).await {
                Some(s) => {
                    state.set(Some(s));
                    error.set(None);
                }
                None => {
                    state.set(None);
                    error.set(Some("not a git repository".into()));
                }
            }
        });

        // Wire up live refresh on .git directory changes (Tauri-only path;
        // watch_paths / on_path_changed are no-ops on non-Tauri hosts).
        let git_path = format!("{}/.git", root);
        spawn_local(async move {
            let _ = host::watch_paths(&[git_path]).await;
        });

        host::on_path_changed(move |_ev| {
            let state = state;
            let root = root_for_watch.clone();
            spawn_local(async move {
                if let Some(s) = host::repo_state(&root).await {
                    state.set(Some(s));
                }
            });
        });
    });

    let is_tauri = host::running_in_tauri();

    move || {
        let mode = classify(is_tauri, &state.get(), &error.get());

        view! {
            <div class="deck-repo-panel">
                {match mode {
                    PanelMode::NotTauri => view! {
                        <div class="deck-repo-empty">
                            <span class="deck-repo-empty-icon">"⎇"</span>
                            <p class="deck-repo-empty-text">
                                "Repo panel is available in the native Ocean Desktop shell."
                            </p>
                        </div>
                    }.into_any(),

                    PanelMode::NoRepo(msg) => view! {
                        <div class="deck-repo-empty">
                            <span class="deck-repo-empty-icon">"⎇"</span>
                            <p class="deck-repo-empty-text">
                                {msg.unwrap_or_else(|| "Loading repo state...".into())}
                            </p>
                        </div>
                    }.into_any(),

                    PanelMode::Repo(repo) => view! {
                        // ── Branch line ──────────────────────────────
                        <div class="deck-repo-branch">
                            <span class="deck-repo-branch-icon">"⎇"</span>
                            <span class="deck-repo-branch-name">{repo.branch.clone()}</span>
                            {format_ahead(repo.ahead).map(|t| view! {
                                <span class="deck-repo-ahead">{t}</span>
                            })}
                            {format_behind(repo.behind).map(|t| view! {
                                <span class="deck-repo-behind">{t}</span>
                            })}
                        </div>

                        // ── Working state (quiet when clean) ────────
                        {if repo.dirty > 0 || repo.staged > 0 {
                            view! {
                                <div class="deck-repo-working">
                                    {if repo.staged > 0 {
                                        view! {
                                            <span class="deck-repo-staged">
                                                "\u{25cf}" {repo.staged} " staged"
                                            </span>
                                        }.into_any()
                                    } else {
                                        ().into_any()
                                    }}
                                    {if repo.dirty > 0 {
                                        view! {
                                            <span class="deck-repo-dirty">
                                                "\u{25cb}" {repo.dirty} " modified"
                                            </span>
                                        }.into_any()
                                    } else {
                                        ().into_any()
                                    }}
                                </div>
                            }.into_any()
                        } else {
                            ().into_any()
                        }}

                        // ── Recent commits ──────────────────────────
                        <div class="deck-repo-commits">
                            {repo.commits.iter().map(|c| {
                                view! {
                                    <div class="deck-repo-commit">
                                        <code class="deck-repo-commit-id">{c.id_short.clone()}</code>
                                        <span class="deck-repo-commit-summary">{c.summary.clone()}</span>
                                        <span class="deck-repo-commit-time">
                                            {relative_time(c.when_epoch, now)}
                                        </span>
                                    </div>
                                }
                            }).collect::<Vec<_>>()}
                        </div>

                        // ── PR / CI extension point ─────────────────
                        // GH-API depth (PR status, CI checks, review state)
                        // arrives later via daemon tools. The panel schema
                        // reserves space here; render nothing yet.
                    }.into_any(),
                }}
            </div>
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── relative_time ────────────────────────────────────────────────

    #[test]
    fn relative_time_just_now() {
        assert_eq!(relative_time(1000, 1000), "just now");
        assert_eq!(relative_time(1000, 1059), "just now");
    }

    #[test]
    fn relative_time_minutes() {
        assert_eq!(relative_time(1000, 1060), "1m ago");
        assert_eq!(relative_time(1000, 1240), "4m ago");
        assert_eq!(relative_time(1000, 4599), "59m ago");
    }

    #[test]
    fn relative_time_hours() {
        assert_eq!(relative_time(1000, 4600), "1h ago");
        assert_eq!(relative_time(1000, 46000), "12h ago");
        assert_eq!(relative_time(1000, 87399), "23h ago");
    }

    #[test]
    fn relative_time_days() {
        assert_eq!(relative_time(1000, 1000 + 86400), "1d ago");
        assert_eq!(relative_time(1000, 1000 + 259200), "3d ago");
        assert_eq!(relative_time(1000, 1000 + 604799), "6d ago");
    }

    #[test]
    fn relative_time_weeks() {
        assert_eq!(relative_time(1000, 1000 + 604800), "1w ago");
        assert_eq!(relative_time(1000, 1000 + 1209600), "2w ago");
        assert_eq!(relative_time(1000, 1000 + 2419199), "3w ago");
    }

    #[test]
    fn relative_time_months() {
        assert_eq!(relative_time(1000, 1000 + 2592000), "1mo ago");
        assert_eq!(relative_time(1000, 1000 + 2592000 * 6), "6mo ago");
        assert_eq!(relative_time(1000, 1000 + 31535999), "11mo ago");
    }

    #[test]
    fn relative_time_years() {
        assert_eq!(relative_time(1000, 1000 + 31536000), "1y ago");
        assert_eq!(relative_time(1000, 1000 + 31536000 * 5), "5y ago");
    }

    #[test]
    fn relative_time_epoch_in_future() {
        // If epoch > now, diff saturates to 0 → "just now"
        assert_eq!(relative_time(2000, 1000), "just now");
    }

    #[test]
    fn relative_time_zero_or_negative() {
        assert_eq!(relative_time(0, 1000), "unknown");
        assert_eq!(relative_time(1000, 0), "unknown");
        assert_eq!(relative_time(-1, 1000), "unknown");
    }

    // ── format_ahead / format_behind ─────────────────────────────────

    #[test]
    fn format_ahead_zero_is_none() {
        assert_eq!(format_ahead(0), None);
    }

    #[test]
    fn format_ahead_nonzero() {
        assert_eq!(format_ahead(1), Some("\u{2191}1".into()));
        assert_eq!(format_ahead(5), Some("\u{2191}5".into()));
    }

    #[test]
    fn format_behind_zero_is_none() {
        assert_eq!(format_behind(0), None);
    }

    #[test]
    fn format_behind_nonzero() {
        assert_eq!(format_behind(3), Some("\u{2193}3".into()));
    }

    // ── classify (empty-state selection) ─────────────────────────────

    #[test]
    fn classify_not_tauri_always_empty() {
        let state: Option<RepoState> = None;
        let error: Option<String> = None;
        let mode = classify(false, &state, &error);
        assert!(matches!(mode, PanelMode::NotTauri));
    }

    #[test]
    fn classify_tauri_no_repo_state() {
        let state: Option<RepoState> = None;
        let error = Some("not a git repository".into());
        let mode = classify(true, &state, &error);
        match &mode {
            PanelMode::NoRepo(Some(msg)) => assert!(msg.contains("not a git")),
            other => panic!("expected NoRepo with message, got {:?}", other),
        }
    }

    #[test]
    fn classify_tauri_repo_with_empty_branch_is_no_repo() {
        let repo = RepoState {
            branch: String::new(),
            ..Default::default()
        };
        let state = Some(repo);
        let error: Option<String> = None;
        let mode = classify(true, &state, &error);
        assert!(matches!(mode, PanelMode::NoRepo(_)));
    }

    #[test]
    fn classify_tauri_repo_valid_branch_is_repo() {
        let repo = RepoState {
            branch: "main".into(),
            ahead: 2,
            behind: 1,
            dirty: 0,
            staged: 3,
            commits: vec![],
        };
        let state = Some(repo.clone());
        let mode = classify(true, &state, &None);
        match &mode {
            PanelMode::Repo(r) => {
                assert_eq!(r.branch, "main");
                assert_eq!(r.ahead, 2);
                assert_eq!(r.behind, 1);
                assert_eq!(r.staged, 3);
            }
            other => panic!("expected Repo, got {:?}", other),
        }
    }

    #[test]
    fn classify_tauri_error_with_no_state() {
        let state: Option<RepoState> = None;
        let error = Some("some error".into());
        let mode = classify(true, &state, &error);
        match &mode {
            PanelMode::NoRepo(Some(msg)) => assert_eq!(msg, "some error"),
            other => panic!("expected NoRepo with error message, got {:?}", other),
        }
    }
}
