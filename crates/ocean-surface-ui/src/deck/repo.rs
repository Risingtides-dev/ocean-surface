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

                    }.into_any(),
                }}
                <GitHubSection daemon=daemon.clone() />
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

// ---------------------------------------------------------------------------
// GitHub depth section (TASK-32) — lives beneath the local section in RepoPanel
// ---------------------------------------------------------------------------

// use imports for GitHubSection
use crate::daemon::{
    fetch_gh_checks, fetch_gh_pull_detail, fetch_gh_pulls, fetch_gh_reviews, GhCheckRun,
    GhPullDetail, GhPullListItem, GhRate, GhReview,
};

/// Pure helper — choose the highest-authority review state from a vec.
/// Priority: approved > changes > commented > pending.
fn top_review_state(reviews: &[crate::daemon::GhReview]) -> &'static str {
    let mut best = 4u8;
    for r in reviews {
        let pri = match r.state.as_str() {
            "approved" => 1,
            "changes_requested" | "changes" => 2,
            "commented" => 3,
            _ => 4,
        };
        if pri < best {
            best = pri;
        }
    }
    match best {
        1 => "approved",
        2 => "changes",
        3 => "commented",
        _ => "pending",
    }
}

/// Check-summary descriptor for a collapsed row.
#[derive(Debug, Clone)]
enum CheckSummary {
    Empty,
    AllPass,
    Failing { passing: usize, total: usize },
    InProgress { total: usize },
}

fn compute_check_summary(checks: &[GhCheckRun]) -> CheckSummary {
    if checks.is_empty() {
        return CheckSummary::Empty;
    }
    let mut passing = 0usize;
    let mut failing = 0usize;
    let mut pending = 0usize;
    for c in checks {
        match c.conclusion.as_deref() {
            Some("success") => passing += 1,
            Some("failure" | "timed_out" | "action_required") => failing += 1,
            _ => match c.status.as_str() {
                "completed" => passing += 1,
                "queued" | "in_progress" => pending += 1,
                _ => {}
            },
        }
    }
    let total = passing + failing + pending;
    if total == 0 {
        return CheckSummary::Empty;
    }
    if failing > 0 {
        CheckSummary::Failing { passing, total }
    } else if pending == total {
        CheckSummary::InProgress { total }
    } else {
        CheckSummary::AllPass
    }
}

/// Compute inline CSS for a label pill — 20% composite over #141414.
fn label_pill_style(raw: &str) -> String {
    if raw.len() != 6 || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return "background:transparent; color:var(--fg-3);".into();
    }
    let r = u8::from_str_radix(&raw[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&raw[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&raw[4..6], 16).unwrap_or(0);
    let r_comp = ((r as f32) * 0.2 + 0x14u8 as f32 * 0.8) as u8;
    let g_comp = ((g as f32) * 0.2 + 0x14u8 as f32 * 0.8) as u8;
    let b_comp = ((b as f32) * 0.2 + 0x14u8 as f32 * 0.8) as u8;
    format!(
        "background:rgb({},{},{}); color:var(--fg);",
        r_comp, g_comp, b_comp
    )
}

/// The GitHub depth section rendered below the local repo state.
#[component]
#[allow(clippy::redundant_locals)]
pub fn GitHubSection(daemon: crate::daemon::Daemon) -> impl IntoView {
    let pulls: RwSignal<Vec<GhPullListItem>> = RwSignal::new(Vec::new());
    let rate: RwSignal<Option<GhRate>> = RwSignal::new(None);
    let state_filter: RwSignal<String> = RwSignal::new("open".into());
    let page: RwSignal<u32> = RwSignal::new(1);
    let has_more: RwSignal<bool> = RwSignal::new(false);
    let loading: RwSignal<bool> = RwSignal::new(false);
    let error: RwSignal<Option<String>> = RwSignal::new(None);
    let expanded_pr: RwSignal<Option<u64>> = RwSignal::new(None);
    let detail_cache: RwSignal<std::collections::HashMap<u64, GhPullDetail>> =
        RwSignal::new(std::collections::HashMap::new());
    let checks_cache: RwSignal<std::collections::HashMap<String, Vec<GhCheckRun>>> =
        RwSignal::new(std::collections::HashMap::new());
    let reviews_cache: RwSignal<std::collections::HashMap<u64, Vec<crate::daemon::GhReview>>> =
        RwSignal::new(std::collections::HashMap::new());
    let generation: RwSignal<u64> = RwSignal::new(0);

    // ── Fetch logic ──────────────────────────────────────────────────
    {
        let daemon = daemon.clone();
        Effect::new(move |_| {
            let project_id = daemon.project.get();
            let Some(ref pid) = project_id else {
                pulls.set(Vec::new());
                rate.set(None);
                has_more.set(false);
                loading.set(false);
                error.set(None);
                return;
            };
            if pid.is_empty() {
                pulls.set(Vec::new());
                rate.set(None);
                has_more.set(false);
                loading.set(false);
                error.set(None);
                return;
            }
            let gen = generation.get();
            let st = state_filter.get_untracked();
            let pg = page.get_untracked();
            let base = daemon.url.get_untracked();
            let pid_clone = pid.clone();
            let gen_s = generation;
            let pulls_s = pulls;
            let rate_s = rate;
            let has_more_s = has_more;
            let load_s = loading;
            let err_s = error;
            loading.set(true);
            spawn_local(async move {
                match fetch_gh_pulls(&base, &pid_clone, &st, pg, 10).await {
                    Some(env) if env.ok => {
                        if gen_s.get_untracked() != gen {
                            return;
                        }
                        pulls_s.set(env.pulls);
                        rate_s.set(Some(env.rate));
                        has_more_s.set(env.has_more);
                        err_s.set(None);
                    }
                    Some(_) => {
                        if gen_s.get_untracked() != gen {
                            return;
                        }
                        err_s.set(Some("GitHub API error.".into()));
                    }
                    None => {
                        if gen_s.get_untracked() != gen {
                            return;
                        }
                        err_s.set(Some(
                            "Could not reach GitHub. Check daemon connection.".into(),
                        ));
                    }
                }
                if gen_s.get_untracked() == gen {
                    load_s.set(false);
                }
            });
        });
    }

    // ── Rate countdown timer ──────────────────────────────────────────
    let rate_reset_countdown: RwSignal<Option<String>> = RwSignal::new(None);
    {
        let rate = rate;
        let countdown = rate_reset_countdown;
        let interval_handle: RwSignal<Option<IntervalHandle>> = RwSignal::new(None);
        Effect::new(move |_| {
            if let Some(h) = interval_handle.get_untracked() {
                h.clear();
            }
            let Some(ref rt) = rate.get() else {
                countdown.set(None);
                return;
            };
            let reset = rt.reset;
            let update = move || {
                let now = js_sys::Date::now() as u64 / 1000;
                if now >= reset {
                    countdown.set(Some("resetting…".into()));
                } else {
                    let secs = reset - now;
                    let mins = secs / 60;
                    let s = secs % 60;
                    countdown.set(Some(format!("resets in {}m{}s", mins, s)));
                }
            };
            update();
            if let Ok(h) = set_interval_with_handle(update, std::time::Duration::from_secs(15)) {
                interval_handle.set(Some(h));
            }
        });
    }

    let open_count = move || {
        if state_filter.get() == "open" && !loading.get() {
            let n = pulls.get().len();
            if has_more.get() {
                format!("{}+", n)
            } else {
                n.to_string()
            }
        } else {
            String::new()
        }
    };

    // Pre-clone daemon for closures that re-capture it.
    let daemon2 = daemon.clone();

    view! {
        <div class="deck-repo-gh">
            // ── Section header ───────────────────────────────────────
            <div class="deck-repo-gh-header">
                <span class="deck-repo-gh-label">"GitHub"</span>
                <div class="deck-repo-gh-header-right">
                    {move || rate.get().map(|r| {
                        let remaining = r.remaining;
                        let cls = if remaining == 0 {
                            "deck-repo-gh-rate deck-repo-gh-rate--err"
                        } else if remaining < 10 {
                            "deck-repo-gh-rate deck-repo-gh-rate--warn"
                        } else {
                            "deck-repo-gh-rate"
                        };
                        let text = if remaining == 0 {
                            rate_reset_countdown.get()
                                .unwrap_or_else(|| format!("{} remaining", remaining))
                        } else {
                            format!("API: {} remaining", remaining)
                        };
                        view! { <span class={cls}>{text}</span> }
                    })}
                    <button
                        class="deck-repo-gh-refresh"
                        on:click={
                            let generation = generation;
                            let expanded_pr = expanded_pr;
                            let detail_cache = detail_cache;
                            let checks_cache = checks_cache;
                            let reviews_cache = reviews_cache;
                            let page = page;
                            move |_| {
                                generation.update(|g| *g += 1);
                                expanded_pr.set(None);
                                detail_cache.set(std::collections::HashMap::new());
                                checks_cache.set(std::collections::HashMap::new());
                                reviews_cache.set(std::collections::HashMap::new());
                                page.set(1);
                            }
                        }
                        title="Refresh"
                    >
                        "↻"
                    </button>
                </div>
            </div>

            // ── State toggle ─────────────────────────────────────────
            <div class="deck-repo-gh-toggle">
                {["open", "closed", "all"].iter().map(|&st| {
                    let active = move || state_filter.get() == st;
                    let label = match st {
                        "open" => format!("open ({})", open_count()),
                        _ => st.to_string(),
                    };
                    view! {
                        <button
                            class={move || if active() {
                                "deck-repo-gh-toggle-btn deck-repo-gh-toggle-btn--active"
                            } else {
                                "deck-repo-gh-toggle-btn"
                            }}
                            on:click={
                                let state_filter = state_filter;
                                let generation = generation;
                                let expanded_pr = expanded_pr;
                                let page = page;
                                move |_| {
                                    state_filter.set(st.to_string());
                                    page.set(1);
                                    expanded_pr.set(None);
                                    generation.update(|g| *g += 1);
                                }
                            }
                        >
                            {label.clone()}
                        </button>
                    }
                }).collect::<Vec<_>>()}
            </div>

            // ── Loading shimmer ──────────────────────────────────────
            {move || loading.get().then(|| view! {
                <div class="deck-repo-gh-shimmer-list">
                    {(0..5).map(|_| view! {
                        <div class="deck-repo-gh-shimmer-row"></div>
                    }).collect::<Vec<_>>()}
                </div>
            })}

            // ── Error banner ─────────────────────────────────────────
            {move || error.get().map(|msg| view! {
                <div class="deck-repo-gh-error">{msg}</div>
            })}

            // ── PR list ──────────────────────────────────────────────
            <div class="deck-repo-gh-list">
                {move || {
                    if loading.get() || (pulls.get().is_empty() && error.get().is_none()) {
                        ().into_any()
                    } else {
                        let d = daemon2.clone();
                        pulls.get().iter().map({
                            let d = d;
                            move |pr| {
                                let daemon_local = d.clone();
                                pr_row(
                                    pr, daemon_local, expanded_pr, detail_cache, checks_cache,
                                    reviews_cache, generation,
                                )
                            }
                        }).collect::<Vec<_>>().into_any()
                    }
                }}
            </div>

            // ── Load more ────────────────────────────────────────────
            {move || {
                if has_more.get() && !loading.get() && !pulls.get().is_empty() {
                    view! {
                        <button
                            class="deck-repo-gh-load-more"
                            on:click={
                                let page = page;
                                let generation = generation;
                                move |_| {
                                    page.update(|p| *p += 1);
                                    generation.update(|g| *g += 1);
                                }
                            }
                        >
                            "Load more"
                        </button>
                    }.into_any()
                } else {
                    ().into_any()
                }
            }}

            // ── Empty state ──────────────────────────────────────────
            {move || {
                if !loading.get() && error.get().is_none() && pulls.get().is_empty()
                    && daemon.project.get().is_some()
                {
                    let msg = match state_filter.get().as_str() {
                        "open" => "No open pull requests.",
                        "closed" => "No closed pull requests.",
                        _ => "No pull requests.",
                    };
                    view! {
                        <div class="deck-repo-gh-empty">
                            <span class="deck-repo-gh-empty-icon">"⎉"</span>
                            <p class="deck-repo-gh-empty-text">{msg}</p>
                        </div>
                    }.into_any()
                } else {
                    ().into_any()
                }
            }}
        </div>
    }
}

/// Renders a single PR row (collapsed + expanded detail).
#[allow(clippy::redundant_locals)]
fn pr_row(
    pr: &GhPullListItem,
    daemon: crate::daemon::Daemon,
    expanded_pr: RwSignal<Option<u64>>,
    detail_cache: RwSignal<std::collections::HashMap<u64, GhPullDetail>>,
    checks_cache: RwSignal<std::collections::HashMap<String, Vec<GhCheckRun>>>,
    reviews_cache: RwSignal<std::collections::HashMap<u64, Vec<GhReview>>>,
    generation: RwSignal<u64>,
) -> impl IntoView {
    let number = pr.number;
    let is_expanded = move || expanded_pr.get() == Some(number);
    let head_sha = pr.head_sha.clone();

    // Label pills (max 3 + overflow)
    let label_pills: Vec<_> = pr
        .labels
        .iter()
        .take(3)
        .map(|l| {
            let style = label_pill_style(&l.color);
            let name = l.name.clone();
            view! {
                <span class="deck-repo-gh-label-pill" style={style}>{name}</span>
            }
        })
        .collect();
    let overflow = if pr.labels.len() > 3 {
        Some(format!("+{}", pr.labels.len() - 3))
    } else {
        None
    };

    let check_summary = {
        let ck = checks_cache.get_untracked();
        ck.get(&head_sha).map(|c| compute_check_summary(c))
    };
    let top_review = reviews_cache
        .get_untracked()
        .get(&number)
        .map(|reviews| top_review_state(reviews));

    let draft_prefix = if pr.draft {
        Some(view! { <span class="deck-repo-gh-pr-draft">"[DRAFT]"</span> })
    } else {
        None
    };

    view! {
        <div class="deck-repo-gh-pr">
            <button
                class="deck-repo-gh-pr-row"
                aria-expanded=move || if is_expanded() { "true" } else { "false" }
                aria-controls=format!("gh-pr-detail-{}", number)
                on:click={
                    let expanded_pr = expanded_pr;
                    let detail_cache = detail_cache;
                    let checks_cache = checks_cache;
                    let reviews_cache = reviews_cache;
                    let daemon = daemon.clone();
                    let generation = generation;
                    let project_id = daemon.project.get_untracked().unwrap_or_default();
                    let head_sha = head_sha.clone();
                    move |_| {
                        let prev = expanded_pr.get_untracked();
                        if prev == Some(number) {
                            expanded_pr.set(None);
                            return;
                        }
                        expanded_pr.set(Some(number));
                        let need_detail = !detail_cache.get_untracked().contains_key(&number);
                        let need_checks = !checks_cache.get_untracked().contains_key(&head_sha);
                        let need_reviews = !reviews_cache.get_untracked().contains_key(&number);
                        if need_detail || need_checks || need_reviews {
                            let base = daemon.url.get_untracked();
                            let pid = project_id.clone();
                            let gen = generation.get_untracked();
                            let dc = detail_cache;
                            let cc = checks_cache;
                            let rc = reviews_cache;
                            let sha = head_sha.clone();
                            spawn_local(async move {
                                if need_detail {
                                    if let Some(env) = fetch_gh_pull_detail(&base, &pid, number).await {
                                        if env.ok && generation.get_untracked() == gen {
                                            dc.update(|map| {
                                                map.insert(number, env.pull);
                                            });
                                        }
                                    }
                                }
                                if need_checks {
                                    if let Some(env) = fetch_gh_checks(&base, &pid, &sha).await {
                                        if env.ok && generation.get_untracked() == gen {
                                            cc.update(|map| {
                                                map.insert(sha.clone(), env.checks);
                                            });
                                        }
                                    }
                                }
                                if need_reviews {
                                    if let Some(env) = fetch_gh_reviews(&base, &pid, number, 1, 10).await {
                                        if env.ok && generation.get_untracked() == gen {
                                            rc.update(|map| {
                                                map.insert(number, env.reviews);
                                            });
                                        }
                                    }
                                }
                            });
                        }
                    }
                }
            >
                <span class="deck-repo-gh-pr-number">{format!("#{}", number)}</span>
                <span class="deck-repo-gh-pr-title">
                    {draft_prefix}
                    {pr.title.clone()}
                </span>
                <div class="deck-repo-gh-pr-labels">
                    {label_pills}
                    {overflow.map(|t| view! {
                        <span class="deck-repo-gh-label-pill deck-repo-gh-label-pill--overflow">{t}</span>
                    })}
                </div>
            </button>
            <div class="deck-repo-gh-pr-meta">
                <span class="deck-repo-gh-pr-author">{pr.author.clone()}</span>
                <span class="deck-repo-gh-pr-sep">" · "</span>
                <span class="deck-repo-gh-pr-branch">{pr.branch.clone()}</span>
                {check_summary.map(|cs| {
                    view! {
                        <span class="deck-repo-gh-check-bar">
                            {match cs {
                                CheckSummary::Empty => view! { <span class="deck-repo-gh-check-text">"No checks"</span> }.into_any(),
                                CheckSummary::AllPass => view! { <span class="deck-repo-gh-check-text deck-repo-gh-check-text--ok">"All checks pass"</span> }.into_any(),
                                CheckSummary::Failing { passing, total } => view! {
                                    <span class="deck-repo-gh-check-text deck-repo-gh-check-text--err">{format!("✗ {}/{} checks", passing, total)}</span>
                                }.into_any(),
                                CheckSummary::InProgress { total } => view! {
                                    <span class="deck-repo-gh-check-text deck-repo-gh-check-text--warn">{format!("{} checks pending", total)}</span>
                                }.into_any(),
                            }}
                        </span>
                    }
                })}
                {top_review.map(|state| {
                    let (cls, label) = match state {
                        "approved" => ("deck-repo-gh-review--approved", "approved"),
                        "changes" => ("deck-repo-gh-review--changes", "changes"),
                        "commented" => ("deck-repo-gh-review--commented", "commented"),
                        _ => ("deck-repo-gh-review--pending", "pending"),
                    };
                    view! { <span class={format!("deck-repo-gh-review {}", cls)}>{label}</span> }
                })}
            </div>

            // Expanded detail
            {move || is_expanded().then(|| {
                let detail = detail_cache.get().get(&number).cloned();
                let checks: Option<Vec<GhCheckRun>> = checks_cache.get().get(&head_sha).cloned();
                let reviews = reviews_cache.get().get(&number).cloned();
                view! {
                    <div
                        class="deck-repo-gh-detail"
                        id=format!("gh-pr-detail-{}", number)
                        role="region"
                    >
                        {detail.as_ref().map(|d| view! {
                            <div class="deck-repo-gh-body">
                                <pre class="deck-repo-gh-body-text">
                                    {d.body.clone()}
                                    {d.body_truncated.then(|| view! {
                                        <span class="deck-repo-gh-body-truncated">"… (truncated)"</span>
                                    })}
                                </pre>
                            </div>
                        })}
                        {checks.as_ref().map(|cs| view! {
                            <div class="deck-repo-gh-checks">
                                <div class="deck-repo-gh-subhead">"Checks"</div>
                                {if cs.is_empty() {
                                    view! { <span class="deck-repo-gh-check-text">"No checks"</span> }.into_any()
                                } else {
                                    cs.iter().map(|c| {
                                        let (icon, icon_cls) = match c.conclusion.as_deref() {
                                            Some("success") => ("✓", "deck-repo-gh-check-icon--ok"),
                                            Some("failure" | "timed_out" | "action_required") => ("✗", "deck-repo-gh-check-icon--err"),
                                            _ => match c.status.as_str() {
                                                "queued" | "in_progress" => ("◌", "deck-repo-gh-check-icon--warn"),
                                                _ => ("⬚", "deck-repo-gh-check-icon--muted"),
                                            }
                                        };
                                        view! {
                                            <div class="deck-repo-gh-check-row">
                                                <span class={format!("deck-repo-gh-check-icon {}", icon_cls)}>{icon}</span>
                                                <span class="deck-repo-gh-check-name">{c.name.clone()}</span>
                                                <span class="deck-repo-gh-check-conclusion">{c.conclusion.clone().unwrap_or_default()}</span>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>().into_any()
                                }}
                            </div>
                        })}
                        {reviews.as_ref().map(|rs| view! {
                            <div class="deck-repo-gh-reviews">
                                <div class="deck-repo-gh-subhead">"Reviews"</div>
                                {if rs.is_empty() {
                                    view! { <span class="deck-repo-gh-check-text">"No reviews"</span> }.into_any()
                                } else {
                                    rs.iter().map(|r| {
                                        let dot_cls = match r.state.as_str() {
                                            "approved" => "deck-repo-gh-review-dot--approved",
                                            "changes_requested" | "changes" => "deck-repo-gh-review-dot--changes",
                                            "commented" => "deck-repo-gh-review-dot--commented",
                                            _ => "deck-repo-gh-review-dot--pending",
                                        };
                                        view! {
                                            <div class="deck-repo-gh-review-row">
                                                <span class={format!("deck-repo-gh-review-dot {}", dot_cls)}></span>
                                                <span class="deck-repo-gh-review-reviewer">{r.reviewer.clone()}</span>
                                                <span class="deck-repo-gh-review-state">{r.state.clone()}</span>
                                                <span class="deck-repo-gh-review-body">{r.body.clone()}</span>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>().into_any()
                                }}
                            </div>
                        })}
                    </div>
                }
            })}
        </div>
    }
}
