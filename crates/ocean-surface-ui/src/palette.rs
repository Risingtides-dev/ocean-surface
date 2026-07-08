//! Command palette — the deep-menu registry (⌘K UI, typed Command registry
//! with scope + availability predicates). One registry drives the palette,
//! the native menubar, and the header overflow.
//! Owned by the Palette workstream (feat/desktop-palette); real commands are
//! wired at integration time (app.rs), not here.

use leptos::ev::{self, KeyboardEvent};
use leptos::prelude::*;

// ── Types ──────────────────────────────────────────────────────────────────

/// The scope a command belongs to — used for grouping and as a visible tag
/// in palette rows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum CommandScope {
    Session,
    Files,
    Repo,
    Browser,
    App,
}

impl CommandScope {
    /// Human-readable label for the scope group header in the palette.
    fn label(self) -> &'static str {
        match self {
            CommandScope::Session => "Session",
            CommandScope::Files => "Files",
            CommandScope::Repo => "Repo",
            CommandScope::Browser => "Browser",
            CommandScope::App => "App",
        }
    }

    /// Stable sort order for scope groups in the palette.
    fn order(self) -> usize {
        match self {
            CommandScope::Session => 0,
            CommandScope::Files => 1,
            CommandScope::Repo => 2,
            CommandScope::Browser => 3,
            CommandScope::App => 4,
        }
    }
}

/// A single command in the deep-menu registry. One `Command` added to the
/// registry appears in the ⌘K palette, the native menubar, *and* the header
/// overflow — the integrator registers it once.
#[derive(Clone)]
pub struct Command {
    /// Stable identifier (e.g. `"new-session"`). Used for deduplication
    /// and event routing.
    pub id: &'static str,
    /// Display title shown in the palette row.
    pub title: String,
    /// Optional subtitle / supplementary text (e.g. a keyboard shortcut hint).
    pub hint: Option<String>,
    /// Scope for grouping and visual tagging.
    pub scope: CommandScope,
    /// Availability predicate evaluated at open/filter time. Host-gating
    /// (e.g. Tauri-only commands) happens here: the integrator wires a
    /// `Signal<bool>` that reflects the current host.
    pub enabled: Signal<bool>,
    /// Action to run when the command is selected and Enter is pressed
    /// (or the row is clicked).
    pub run: Callback<()>,
}

/// A cloneable, signal-backed command registry. Create one with
/// [`CommandRegistry::new`], register commands via [`register`](CommandRegistry::register),
/// and pass it to [`PaletteView`]. Registration after mount works — the
/// palette reacts when commands are added later.
///
/// # Example (integrator's app.rs)
///
/// ```ignore
/// let registry = CommandRegistry::new();
/// registry.register(Command {
///     id: "toggle-deck",
///     title: "Toggle Context Deck".into(),
///     hint: None,
///     scope: CommandScope::App,
///     enabled: Signal::derive(move || show_deck.get()),
///     run: Callback::new(move |_| show_deck.update(|v| *v = !*v)),
/// });
///
/// view! { <PaletteView registry=registry /> }
/// ```
#[derive(Clone)]
pub struct CommandRegistry {
    commands: RwSignal<Vec<Command>>,
}

impl CommandRegistry {
    /// Create an empty command registry, ready for registration.
    pub fn new() -> Self {
        Self {
            commands: RwSignal::new(Vec::new()),
        }
    }

    /// Register a command. Safe to call from any reactive scope; the palette
    /// picks up new commands on the next open or filter cycle.
    pub fn register(&self, cmd: Command) {
        self.commands.update(|cmds| cmds.push(cmd));
    }

    /// Run the enabled command identified by `id`. Returns `true` when a
    /// matching, currently-enabled command was found and its callback fired;
    /// `false` when the id is unknown or the command is registered but gated
    /// off by its `enabled` signal (a disabled command is refused, never run).
    ///
    /// This is the dispatch entry point event-driven callers share with the
    /// ⌘K palette: the native menubar emits a `menu-command` event carrying a
    /// command id, the host bridge hands the id here, and the registry routes
    /// it to the same `Callback` the palette runs. Unknown ids (a stray menu
    /// id that is not a palette command) resolve to a no-op `false`.
    pub fn run(&self, id: &str) -> bool {
        let Some(cmd) = self
            .commands
            .get_untracked()
            .into_iter()
            .find(|c| c.id == id)
        else {
            return false;
        };
        if cmd.enabled.get_untracked() {
            cmd.run.run(());
            true
        } else {
            false
        }
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Fuzzy matching ─────────────────────────────────────────────────────────

/// Fuzzy subsequence score for `query` against `target`.
///
/// Returns `None` when `query` is not a subsequence of `target`
/// (case-insensitive). Higher scores are better matches.
///
/// Scoring heuristics (all additive):
/// - **Exact prefix** — matching the start of the target (ti == qi): +3.0
/// - **Word boundary** — matching after `-`, `_`, `/`, `.`, space, or a
///   camelCase boundary: +5.0
/// - **Consecutive** — contiguous run of matched chars: +2.0 × run length
/// - **Start of target** — first char matches position 0: +10.0
///
/// These bonuses compound, so `"fp"` against `"FilesPanel"` scores:
///   'f' at 0 → +10 (start) +3 (prefix) = 13, 'p' at 5 → +5 (camelCase
///   boundary) +2 (consecutive=1) = 7 → total 20.
fn fuzzy_score(query: &str, target: &str) -> Option<f64> {
    if query.is_empty() {
        return Some(0.0);
    }

    let query_lower = query.to_lowercase();
    let target_lower = target.to_lowercase();
    let query_chars: Vec<char> = query_lower.chars().collect();
    let target_chars: Vec<char> = target_lower.chars().collect();
    // Boundary detection needs the ORIGINAL case — lowercasing erases
    // camelCase transitions. (Assumes 1:1 char mapping under to_lowercase;
    // command titles are ASCII, and a drift would only skew a bonus.)
    let target_orig: Vec<char> = target.chars().collect();

    let mut score: f64 = 0.0;
    let mut qi: usize = 0; // cursor in query
    let mut prev_match: Option<usize> = None;
    let mut consecutive: usize = 0;

    for (ti, tc) in target_chars.iter().enumerate() {
        if qi >= query_chars.len() {
            break;
        }
        if *tc != query_chars[qi] {
            continue;
        }

        let mut char_score: f64 = 1.0;

        // Start-of-target bonus: the very first character matched at position 0.
        if ti == 0 {
            char_score += 10.0;
        }
        // Word-boundary bonus: after a separator or camelCase transition.
        if ti < target_orig.len() && is_word_boundary(&target_orig, ti) {
            char_score += 5.0;
        }
        // Consecutive bonus: adjacent matches in target.
        if let Some(prev) = prev_match {
            if ti == prev + 1 {
                consecutive += 1;
                char_score += 2.0 * consecutive as f64;
            } else {
                consecutive = 0;
            }
        }
        // Prefix bonus: the first N chars of query match the first N of target.
        if qi == ti {
            char_score += 3.0;
        }

        score += char_score;
        prev_match = Some(ti);
        qi += 1;
    }

    if qi == query_chars.len() {
        Some(score)
    } else {
        None
    }
}

/// True when `chars[idx]` is at a word boundary: index 0, after a separator
/// (` `, `-`, `_`, `/`, `.`), or at a camelCase transition (lower→upper).
fn is_word_boundary(chars: &[char], idx: usize) -> bool {
    if idx == 0 {
        return true;
    }
    let prev = chars[idx - 1];
    let curr = chars[idx];
    if matches!(prev, ' ' | '-' | '_' | '/' | '.') {
        return true;
    }
    prev.is_lowercase() && curr.is_uppercase()
}

// ── Selection helpers ──────────────────────────────────────────────────────

/// Wrap-forward: `(idx + 1) % len`. Returns 0 for empty lists.
fn next_index(idx: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (idx + 1) % len
    }
}

/// Wrap-backward: `(idx + len - 1) % len`. Returns 0 for empty lists.
fn prev_index(idx: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (idx + len - 1) % len
    }
}

// ── Internal working type ──────────────────────────────────────────────────

#[derive(Clone)]
struct ScoredCommand {
    command: Command,
    score: f64,
}

impl PartialEq for ScoredCommand {
    fn eq(&self, other: &Self) -> bool {
        self.command.id == other.command.id && self.score == other.score
    }
}

// ── PaletteView component ──────────────────────────────────────────────────

/// The ⌘K command palette — a deep-menu overlay that opens on Cmd+K (macOS)
/// or Ctrl+K (everywhere else). Types are grouped by [`CommandScope`];
/// disabled commands are hidden. Fuzzy subsequence matching with scoring
/// keeps the most relevant commands at the top of each group.
///
/// # Keyboard
///
/// | Key          | Action                                      |
/// |-------------|---------------------------------------------|
/// | ⌘K / Ctrl+K | Open palette, focus input, reset query      |
/// | Esc          | Close palette                               |
/// | ↑ / ↓        | Move selection (wraps)                       |
/// | Enter        | Run selected command, close palette          |
/// | Backdrop     | click closes palette                         |
#[component]
pub fn PaletteView(registry: CommandRegistry) -> impl IntoView {
    let open = RwSignal::new(false);
    let query = RwSignal::new(String::new());
    let selected_index = RwSignal::new(0usize);
    let input_ref: NodeRef<leptos::html::Input> = NodeRef::new();

    // ── Window-level keydown: Cmd+K / Ctrl+K opens the palette ──────────
    let _key_listener = window_event_listener(ev::keydown, move |e: KeyboardEvent| {
        // metaKey on macOS, ctrlKey elsewhere. We check both so Cmd+K works
        // on Mac and Ctrl+K works everywhere else.
        if (e.meta_key() || e.ctrl_key()) && e.key().to_lowercase() == "k" {
            e.prevent_default();
            query.set(String::new());
            selected_index.set(0);
            open.set(true);
        }
    });
    on_cleanup(move || _key_listener.remove());

    // ── Auto-focus the input when the palette opens ──────────────────────
    Effect::new(move |_| {
        if open.get() {
            if let Some(el) = input_ref.get() {
                let _ = el.focus();
            }
        }
    });

    // ── Filter + group + score ───────────────────────────────────────────
    // Re-derived every time `query` or `registry.commands` changes.
    let filtered_groups: Memo<Vec<(CommandScope, Vec<ScoredCommand>)>> =
        Memo::new(move |_| {
            let q = query.get();
            let q = q.trim();
            let cmds = registry.commands.get();

            let mut groups: Vec<(CommandScope, Vec<ScoredCommand>)> = Vec::new();

            for cmd in cmds.iter() {
                // Host/state gating: disabled commands are invisible.
                if !cmd.enabled.get() {
                    continue;
                }

                let score = if q.is_empty() {
                    0.0
                } else {
                    match fuzzy_score(q, &cmd.title) {
                        Some(s) => s,
                        None => continue, // no match → hidden
                    }
                };

                let group = groups.iter_mut().find(|(s, _)| *s == cmd.scope);
                if let Some((_, cmds)) = group {
                    cmds.push(ScoredCommand {
                        command: cmd.clone(),
                        score,
                    });
                } else {
                    groups.push((
                        cmd.scope,
                        vec![ScoredCommand {
                            command: cmd.clone(),
                            score,
                        }],
                    ));
                }
            }

            // Sort within each group by score descending (best first).
            for (_, cmds) in groups.iter_mut() {
                cmds.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }

            // Sort groups by scope order.
            groups.sort_by_key(|(s, _)| s.order());

            groups
        });

    // ── Flat list for index-based navigation ─────────────────────────────
    let flat_commands: Memo<Vec<ScoredCommand>> = Memo::new(move |_| {
        filtered_groups
            .get()
            .into_iter()
            .flat_map(|(_, cmds)| cmds)
            .collect()
    });

    let total_count = move || flat_commands.get().len();

    // ── Keyboard navigation inside the palette ───────────────────────────
    let on_input_keydown = move |ev: KeyboardEvent| {
        match ev.key().as_str() {
            "Escape" => {
                open.set(false);
            }
            "ArrowDown" => {
                ev.prevent_default();
                selected_index.update(|i| *i = next_index(*i, total_count()));
            }
            "ArrowUp" => {
                ev.prevent_default();
                selected_index.update(|i| *i = prev_index(*i, total_count()));
            }
            "Enter" => {
                ev.prevent_default();
                let idx = selected_index.get_untracked();
                let cmds = flat_commands.get_untracked();
                if let Some(sc) = cmds.get(idx) {
                    sc.command.run.run(());
                    open.set(false);
                }
            }
            _ => {}
        }
    };

    // ── View ─────────────────────────────────────────────────────────────
    view! {
        <Show when=move || open.get()>
            // Backdrop — clicking outside the panel closes the palette.
            <div class="palette-overlay" on:click=move |_| open.set(false)>
                // Panel — stop propagation so clicks inside don't close.
                <div class="palette-panel" on:click=|ev| ev.stop_propagation()>
                    <input
                        class="palette-input"
                        type="text"
                        placeholder="Type a command…"
                        node_ref=input_ref
                        prop:value=move || query.get()
                        on:input=move |ev| {
                            query.set(event_target_value(&ev));
                            selected_index.set(0);
                        }
                        on:keydown=on_input_keydown
                    />

                    <div class="palette-results">
                        <Show
                            when=move || !flat_commands.get().is_empty()
                            fallback=move || view! {
                                <div class="palette-empty">
                                    {move || {
                                        if query.get().trim().is_empty() {
                                            "No commands available"
                                        } else {
                                            "No matching commands"
                                        }
                                    }}
                                </div>
                            }
                        >
                            <For
                                each=move || filtered_groups.get()
                                key=|(scope, _)| *scope as usize
                                children=move |(scope, cmds)| {
                                    // Compute global index of each command in the
                                    // flat list so we can check selection state.
                                    let flat = flat_commands.get();
                                    view! {
                                        <div class="palette-group">
                                            <div class="palette-group-label">
                                                {scope.label()}
                                            </div>
                                            {
                                                cmds.into_iter().map(|sc| {
                                                    let cmd_id = sc.command.id;
                                                    let global_idx = flat.iter()
                                                        .position(|f| f.command.id == cmd_id)
                                                        .unwrap_or(0);
                                                    let is_selected = move || {
                                                        selected_index.get() == global_idx
                                                    };
                                                    let run = sc.command.run;
                                                    view! {
                                                        <div
                                                            class="palette-row"
                                                            class:palette-row--selected=is_selected
                                                            on:click=move |_| {
                                                                run.run(());
                                                                open.set(false);
                                                            }
                                                        >
                                                            <span class="palette-row-title">
                                                                {sc.command.title}
                                                            </span>
                                                            {if let Some(hint) = sc.command.hint {
                                                                view! {
                                                                    <span class="palette-row-hint">
                                                                        {hint}
                                                                    </span>
                                                                }.into_any()
                                                            } else {
                                                                ().into_any()
                                                            }}
                                                            <span class="palette-row-kbd">"↵"</span>
                                                        </div>
                                                    }
                                                }).collect::<Vec<_>>()
                                            }
                                        </div>
                                    }
                                }
                            />
                        </Show>
                    </div>
                </div>
            </div>
        </Show>
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── fuzzy_score ─────────────────────────────────────────────────────

    #[test]
    fn empty_query_scores_zero() {
        assert_eq!(fuzzy_score("", "anything"), Some(0.0));
    }

    #[test]
    fn exact_match_scores_positive() {
        let score = fuzzy_score("files", "files").expect("should match");
        assert!(score > 0.0);
    }

    #[test]
    fn exact_prefix_scores_higher_than_word_boundary() {
        // "fp" against "FilesPanel" — 'F' is prefix + start, 'P' is camelCase.
        // "fp" against "SomeFilePanel" — 'f' is camelCase, 'p' is camelCase.
        let prefix_score = fuzzy_score("fp", "FilesPanel").expect("should match");
        let boundary_score = fuzzy_score("fp", "SomeFilePanel").expect("should match");

        assert!(
            prefix_score > boundary_score,
            "exact prefix ({prefix_score}) should outscore word-boundary-only ({boundary_score})"
        );
    }

    #[test]
    fn word_boundary_scores_higher_than_scattered() {
        // "fp" hits camelCase boundaries in "SomeFilePanel" (F, P).
        let boundary = fuzzy_score("fp", "SomeFilePanel").expect("should match");
        // "mp" in "somefilepanel" — mid-word, no boundaries, no start anchor.
        // (A start-anchored control like "sp" would reap start+prefix+idx-0
        // bonuses and test anchoring, not scatter.)
        let scattered = fuzzy_score("mp", "somefilepanel").expect("should match");

        assert!(
            boundary > scattered,
            "word boundary ({boundary}) should outscore scattered ({scattered})"
        );
    }

    #[test]
    fn scattered_subsequence_still_matches() {
        // Every char of query appears in target but spread apart.
        let score = fuzzy_score("smp", "somefilepanel").expect("should match");
        assert!(score > 0.0);
    }

    #[test]
    fn missing_char_returns_none() {
        assert_eq!(fuzzy_score("xyz", "abcdef"), None);
    }

    #[test]
    fn case_insensitive() {
        let lower = fuzzy_score("files", "FILES").expect("should match");
        let upper = fuzzy_score("FILES", "files").expect("should match");
        assert_eq!(lower, upper);
    }

    #[test]
    fn consecutive_bonus_compounds() {
        // "fil" all consecutive in "files" — should score higher than "fls" scattered.
        let consecutive_score = fuzzy_score("fil", "files").expect("should match");
        let scattered_score = fuzzy_score("fls", "files").expect("should match");
        assert!(
            consecutive_score > scattered_score,
            "consecutive ({consecutive_score}) > scattered ({scattered_score})"
        );
    }

    #[test]
    fn camelcase_boundary_detected() {
        let score = fuzzy_score("fp", "FilePanel").expect("should match");
        // 'f' at start (start+prefix+word_boundary), 'p' at camelCase boundary
        assert!(score > 10.0); // start(10) + prefix(3) + word_boundary(5) + base(1) = 19 minimum
    }

    // ── selection helpers ────────────────────────────────────────────────

    #[test]
    fn next_index_wraps_around() {
        assert_eq!(next_index(0, 3), 1);
        assert_eq!(next_index(1, 3), 2);
        assert_eq!(next_index(2, 3), 0);
    }

    #[test]
    fn prev_index_wraps_around() {
        assert_eq!(prev_index(0, 3), 2);
        assert_eq!(prev_index(2, 3), 1);
        assert_eq!(prev_index(1, 3), 0);
    }

    #[test]
    fn next_index_empty_returns_zero() {
        assert_eq!(next_index(0, 0), 0);
        assert_eq!(next_index(5, 0), 0);
    }

    #[test]
    fn prev_index_empty_returns_zero() {
        assert_eq!(prev_index(0, 0), 0);
        assert_eq!(prev_index(5, 0), 0);
    }

    #[test]
    fn prev_index_single_item() {
        assert_eq!(prev_index(0, 1), 0);
    }

    #[test]
    fn next_index_single_item() {
        assert_eq!(next_index(0, 1), 0);
    }

    // ── CommandScope ordering ────────────────────────────────────────────

    #[test]
    fn scope_order_is_stable() {
        let scopes = [
            CommandScope::Session,
            CommandScope::Files,
            CommandScope::Repo,
            CommandScope::Browser,
            CommandScope::App,
        ];
        for (i, s) in scopes.iter().enumerate() {
            assert_eq!(s.order(), i, "scope {s:?} should have order {i}");
        }
    }

    // ── CommandRegistry ──────────────────────────────────────────────────

    #[test]
    fn registry_starts_empty() {
        let registry = CommandRegistry::new();
        assert!(registry.commands.get_untracked().is_empty());
    }

    #[test]
    fn registry_register_adds_command() {
        let registry = CommandRegistry::new();
        let enabled = RwSignal::new(true);
        let cmd = Command {
            id: "test.cmd",
            title: "Test Command".into(),
            hint: None,
            scope: CommandScope::App,
            enabled: Signal::from(enabled),
            run: Callback::new(|_| {}),
        };
        registry.register(cmd);
        assert_eq!(registry.commands.get_untracked().len(), 1);
    }

    #[test]
    fn run_dispatches_enabled_command_by_id() {
        let registry = CommandRegistry::new();
        let fired = RwSignal::new(0u32);
        let fired_run = fired;
        registry.register(Command {
            id: "toggle-files",
            title: "Toggle Files".into(),
            hint: None,
            scope: CommandScope::Files,
            enabled: Signal::from(RwSignal::new(true)),
            run: Callback::new(move |_| fired_run.update(|n| *n += 1)),
        });

        // Known + enabled: dispatches and reports true.
        assert!(registry.run("toggle-files"));
        assert_eq!(fired.get_untracked(), 1);

        // Unknown id: no dispatch, still reports false; counter unchanged.
        assert!(!registry.run("no-such-command"));
        assert_eq!(fired.get_untracked(), 1);
    }

    #[test]
    fn run_refuses_disabled_command() {
        let registry = CommandRegistry::new();
        let fired = RwSignal::new(0u32);
        let enabled = RwSignal::new(false);
        registry.register(Command {
            id: "open-council",
            title: "Open Council".into(),
            hint: None,
            scope: CommandScope::App,
            enabled: Signal::from(enabled),
            run: Callback::new(move |_| fired.update(|n| *n += 1)),
        });

        // Registered but disabled: refused — no callback, returns false.
        assert!(!registry.run("open-council"));
        assert_eq!(fired.get_untracked(), 0);

        // Flip the enabled signal: now it dispatches.
        enabled.set(true);
        assert!(registry.run("open-council"));
        assert_eq!(fired.get_untracked(), 1);
    }

    // ── is_word_boundary ─────────────────────────────────────────────────

    #[test]
    fn word_boundary_at_start() {
        assert!(is_word_boundary(&['a', 'b'], 0));
    }

    #[test]
    fn word_boundary_after_space() {
        assert!(is_word_boundary(&['a', ' ', 'b'], 2));
    }

    #[test]
    fn word_boundary_after_hyphen() {
        assert!(is_word_boundary(&['a', '-', 'b'], 2));
    }

    #[test]
    fn word_boundary_after_underscore() {
        assert!(is_word_boundary(&['a', '_', 'b'], 2));
    }

    #[test]
    fn word_boundary_camelcase() {
        // 'a' (lowercase) → 'B' (uppercase)
        assert!(is_word_boundary(&['a', 'B'], 1));
    }

    #[test]
    fn no_boundary_mid_word() {
        assert!(!is_word_boundary(&['a', 'b', 'c'], 1));
        assert!(!is_word_boundary(&['A', 'B', 'C'], 1)); // all uppercase = acronym
    }

    #[test]
    fn scope_labels_are_stable() {
        assert_eq!(CommandScope::Session.label(), "Session");
        assert_eq!(CommandScope::Files.label(), "Files");
        assert_eq!(CommandScope::Repo.label(), "Repo");
        assert_eq!(CommandScope::Browser.label(), "Browser");
        assert_eq!(CommandScope::App.label(), "App");
    }
}
