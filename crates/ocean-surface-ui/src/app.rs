//! Top-level app shell. Owns the Daemon, mounts the transcript + composer.

use leptos::ev::{self, SubmitEvent};
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::components::{PermissionPrompts, PinnedRail};
use crate::daemon::{daemon_url_from_env, Daemon};
use crate::deck::browser::BrowserCockpit;
use crate::deck::files::FilesPanel;
use crate::deck::repo::RepoPanel;
use crate::deck::DeckPanel;
use crate::host::DaemonStatus;
use crate::island_dynamic::{DynamicIsland, IslandMode};
use crate::model::{Block, Role, Turn};
use crate::palette::{Command, CommandRegistry, CommandScope, PaletteView};
use crate::rooms::{RoomStage, Rooms, RoomsPanel};
use crate::sessions::SessionsPanel;
use crate::slash_menu::{SlashMenu, SlashRow};
use crate::transcript::Transcript;
use crate::voice::VoiceOrb;
use crate::workspace::WorkspaceFocus;

const COMPOSER_MIN_HEIGHT_PX: i32 = 32;
const COMPOSER_MAX_HEIGHT_PX: i32 = 240;

/// localStorage key for the workspace pane's open/collapse state ("1" open,
/// "0" collapsed; absent defaults to open — the pane is the desktop shell's
/// primary surface). Persisted so a relaunch restores it.
const WORKSPACE_OPEN_KEY: &str = "ocean.workspace.open";

fn composer_height_px(scroll_height: i32) -> i32 {
    scroll_height.clamp(COMPOSER_MIN_HEIGHT_PX, COMPOSER_MAX_HEIGHT_PX)
}

fn composer_overflow_y(scroll_height: i32) -> &'static str {
    if scroll_height > COMPOSER_MAX_HEIGHT_PX {
        "auto"
    } else {
        "hidden"
    }
}

fn should_submit_composer_key(key: &str, shift: bool, is_composing: bool) -> bool {
    key == "Enter" && !shift && !is_composing
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SurfaceVoiceLayout {
    center_stage: bool,
    docked: bool,
}

/// Compute the voice-chat root classes from stage and component counts.
/// Baseline is the component count captured when voice started; current is
/// the live count. A pre-existing card must not pre-dock a new session.
fn surface_voice_layout(
    stage: crate::voice::realtime::RealtimeStage,
    baseline_count: Option<usize>,
    current_count: usize,
) -> SurfaceVoiceLayout {
    use crate::voice::realtime::RealtimeStage;
    match stage {
        RealtimeStage::Off => SurfaceVoiceLayout {
            center_stage: false,
            docked: false,
        },
        _ => match baseline_count {
            // No baseline captured yet — stay center-stage.
            None => SurfaceVoiceLayout {
                center_stage: true,
                docked: false,
            },
            Some(baseline) if current_count > baseline => SurfaceVoiceLayout {
                center_stage: false,
                docked: true,
            },
            Some(_) => SurfaceVoiceLayout {
                center_stage: true,
                docked: false,
            },
        },
    }
}

fn fit_composer_textarea(el: &web_sys::HtmlTextAreaElement) {
    let style = el.clone().unchecked_into::<web_sys::HtmlElement>().style();
    let _ = style.set_property("height", "auto");
    let scroll_height = el.scroll_height();
    let height = composer_height_px(scroll_height);
    let _ = style.set_property("height", &format!("{height}px"));
    let _ = style.set_property("overflow-y", composer_overflow_y(scroll_height));
}

fn reset_composer_textarea(el: &web_sys::HtmlTextAreaElement) {
    let style = el.clone().unchecked_into::<web_sys::HtmlElement>().style();
    let _ = style.set_property("height", &format!("{COMPOSER_MIN_HEIGHT_PX}px"));
    let _ = style.set_property("overflow-y", "hidden");
}

/// Whether the surface window currently has focus (`document.hasFocus()`).
/// Defaults to `true` when the document can't be read so an off-focus
/// notification is never fired on an uncertain state.
fn window_focused() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.has_focus().ok())
        .unwrap_or(true)
}

/// Read a localStorage value as an owned string (None when storage is
/// unavailable or the key is unset). Mirrors the helper style in workspace.rs
/// so the pane and the shell persist layout the same way.
fn ls_get(key: &str) -> Option<String> {
    web_sys::window()
        .and_then(|w| w.local_storage().ok())
        .and_then(|r| r)
        .and_then(|r| r.get_item(key).ok())
        .flatten()
}

/// Write a localStorage value; silently no-ops when storage is unavailable
/// (private mode, etc.) — same graceful degradation as workspace.rs.
fn ls_set(key: &str, val: &str) {
    let _ = web_sys::window()
        .and_then(|w| w.local_storage().ok())
        .and_then(|r| r)
        .map(|r| r.set_item(key, val));
}

/// Scope label for composer `/` popover rows. Mirrors the private
/// `CommandScope::label()` in palette.rs (kept private there so that crate
/// owns the display strings) so app.rs can build [`SlashRow`]s without editing
/// palette.rs.
fn scope_label(scope: CommandScope) -> &'static str {
    match scope {
        CommandScope::Session => "Session",
        CommandScope::Files => "Files",
        CommandScope::Repo => "Repo",
        CommandScope::Browser => "Browser",
        CommandScope::App => "App",
    }
}

/// Dispatch a composer `/` command. Arg-taking commands (`/model`, `/thinking`)
/// are handled here so the slash popover and the ⌘K palette run identical code;
/// `/clear` + `/help` (and every other id) delegate to the registry's own `run`
/// callback via `registry.run`, which refuses disabled commands. Returns `true`
/// when a command matched and ran.
fn run_slash(id: &str, args: &str, daemon: &Daemon, registry: &CommandRegistry) -> bool {
    match id {
        "model" => {
            if args.is_empty() {
                daemon
                    .status
                    .set("use /model <id> or the selector below".into());
            } else {
                daemon.set_model_override(Some(args.into()));
                daemon.status.set(format!("model \u{2192} {args}"));
            }
            true
        }
        "thinking" => match args {
            "" | "default" => {
                daemon.set_thinking_level(None);
                daemon.status.set("thinking \u{2192} default".into());
                true
            }
            "off" | "minimal" | "low" | "medium" | "high" | "xhigh" => {
                daemon.set_thinking_level(Some(args.into()));
                daemon.status.set(format!("thinking \u{2192} {args}"));
                true
            }
            _ => {
                daemon.status.set(format!(
                    "unknown level: {args} (off|minimal|low|medium|high|xhigh|default)"
                ));
                true
            }
        },
        // `/clear`, `/help`, new-session, toggle-*, workspace-toggle,
        // open-council — all route through the registry callback so there is
        // exactly one execution path (the slash popover pick and the ⌘K palette
        // behave identically). Disabled soon-commands are refused here.
        _ => registry.run(id),
    }
}

#[component]
pub fn App() -> impl IntoView {
    let daemon = Daemon::new(daemon_url_from_env());
    // Voice phases 2/3: hand the realtime voice-chat module its daemon handle
    // once — the orb's menu entry starts sessions without prop-threading.
    crate::voice::realtime::install(daemon.clone());
    // Zero-config boot: fetch /api/config from the same-origin proxy to learn
    // the daemon URL + confirm auth is preconfigured, THEN connect AND fetch the
    // model catalogue — in that order, inside bootstrap. Falls back to
    // daemon_url_from_env() if no proxy answers.
    //
    // Do NOT add an eager daemon.fetch_models() (or any url-dependent call)
    // here: it would run before bootstrap learns the real origin, succeed by
    // luck on localhost, and silently fail from ocean.risingtidesviral.com
    // (wrong URL → empty model picker). Any startup fetch that needs the daemon
    // URL belongs INSIDE bootstrap_then_connect, after url.set().
    daemon.bootstrap_then_connect();

    // Daemon supervision (Tauri shell only). The shell supervises the
    // ocean-daemon process and reports liveness via `daemon-status` events;
    // we mirror it into a signal so a quiet, conditional indicator can surface
    // "daemon offline" (process down) without new permanent chrome — the
    // existing connection chip already covers the reachable case. Off-Tauri
    // this is a no-op: `daemon_status()` is None and the listener never fires,
    // so the signal stays None and the indicator never mounts.
    let daemon_shell_status = RwSignal::new(None::<DaemonStatus>);
    {
        let sig = daemon_shell_status;
        crate::host::on_daemon_status(move |s| sig.set(Some(s)));
        // Seed from the current status so the indicator is correct before the
        // first on-change event (best-effort; None off-Tauri).
        let sig = daemon_shell_status;
        wasm_bindgen_futures::spawn_local(async move {
            if let Some(s) = crate::host::daemon_status().await {
                sig.set(Some(s));
            }
        });
    }

    let input = RwSignal::new(String::new());
    let textarea_ref: NodeRef<leptos::html::Textarea> = NodeRef::new();
    let daemon_council = daemon.clone();

    // Daemon holds only Copy signal handles, so cloning per-closure is cheap
    // and avoids fighting the borrow checker over a single moved value.
    let status = daemon.status;
    let status_detail = daemon.status_detail;
    let turns = daemon.turns;
    let streaming = daemon.streaming;
    let voice_ready = daemon.voice_ready;
    let last_turn_tokens = daemon.last_turn_tokens;
    let session_tokens = daemon.session_tokens;
    // `daemon.model` (the live global model signal) is no longer bound here —
    // its only consumer, the header model picker, was removed in OCEAN-202. The
    // composer's per-turn `model_override` is the surface's model control now.
    let models = daemon.models;
    // Browser-control indicator (OCEAN-92): lit while the agent is driving the
    // browser (set from the daemon's `browser_activity` SSE event), with the
    // most recent `browser_*` action shown alongside.
    let browser_active = daemon.browser_active;
    let livekit_token_path = daemon.livekit_token_path;
    let browser_last_action = daemon.browser_last_action;
    let pending_permissions = daemon.pending_permissions;
    // Canvas patch stream (OCEAN-178): patches the agent applied this session,
    // streamed over the daemon's `surface_patch` SSE event. The GPUI native
    // shell renders these on a full canvas; the web surface renders a basic
    // representation so the data is no longer dropped at the transport layer.
    let canvas_patches = daemon.canvas_patches;
    // Per-turn overrides (OCEAN-79): reasoning effort + model. Both ride on the
    // next turn's request; `None` leaves the daemon defaults untouched.
    let thinking_level = daemon.thinking_level;
    let model_override = daemon.model_override;
    // Predicates pulled out of the view! macro: a bare `>` inside an attribute
    // expression would be parsed as the element's closing bracket.
    let has_tokens = move || session_tokens.get().total() > 0;
    let has_rate = move || {
        last_turn_tokens
            .get()
            .map(|t| t.tokens_per_second > 0.0)
            .unwrap_or(false)
    };

    // Sessions panel overlay.
    let show_sessions = RwSignal::new(false);
    let island_mode: RwSignal<IslandMode> = RwSignal::new(IslandMode::Closed);
    let island_focus_request = RwSignal::new(0u64);
    // Council/quorum observability deck overlay (OCEAN-96). A native Leptos
    // stage (crate::council::CouncilStage) inside a full-screen modal — no
    // iframe, no proxied static page. It reads the daemon's folded Longhouse
    // topics snapshot (GET /v1/longhouse/topics), polled while the deck is
    // open.
    let show_council = RwSignal::new(false);
    // Call controls row — created early so Rooms::new can share the signal.
    let show_livekit_controls = RwSignal::new(false);
    let show_rooms = RwSignal::new(false);
    // Sessions and Rooms are sibling browse overlays (same right-hand modal
    // pattern) — opening one closes the other so they never stack. Every
    // entry point (palette command, header buttons) routes through these two
    // closures; a second toggle convention beside them is a bug.
    let toggle_sessions = move || {
        let opening = !show_sessions.get_untracked();
        if opening {
            show_rooms.set(false);
        }
        show_sessions.set(opening);
    };
    let toggle_rooms = move || {
        let opening = !show_rooms.get_untracked();
        if opening {
            show_sessions.set(false);
        }
        show_rooms.set(opening);
    };
    // Persistent Rooms panel (OCEAN-108). Shares the Daemon's `url` signal so it
    // targets the same origin; opens a right-hand overlay like Sessions.
    let rooms = Rooms::new(
        &daemon,
        daemon.livekit_room_id,
        daemon.livekit_token_path,
        show_livekit_controls,
        show_rooms,
    );
    // Room mode: a room is a mode of operation you ENTER — opening one from
    // the rooms browser swaps the chat surface for the room's own stage
    // (roster + transcript + composer). Purely daemon-native text; the LiveKit
    // call is an optional in-mode upgrade, so no credential or call state
    // gates the mode.
    let in_room_mode = Signal::derive(move || rooms.open_key.get().is_some());

    // Context deck (north star): the WEB/EXTENSION reveal rail. At most ONE
    // panel revealed at a time, reveal-on-intent via ⌘K commands, never
    // permanent chrome. On Tauri the deck never mounts (the Show gate below
    // hard-gates it on !in_tauri) — the desktop shell's persistent surfaces
    // live in the workspace pane instead, so the toggle-* commands route
    // there on Tauri and here on every other host.
    let deck_panel: RwSignal<Option<DeckPanel>> = RwSignal::new(None);
    let toggle_deck = move |p: DeckPanel| {
        deck_panel.update(|cur| *cur = if *cur == Some(p) { None } else { Some(p) })
    };
    // Panels mount inside a reactive match arm that re-runs on panel switch,
    // so the Daemon clone lives in a StoredValue (same pattern as
    // daemon_for_perms below).
    let daemon_for_deck = StoredValue::new(daemon.clone());

    // Workspace pane (north star desktop shell): THE right-side surface on
    // Tauri — permanent, tabbed (Files · previews · Browser · Repo). One
    // command layer routes per-host: toggle-* commands open+focus a tab here
    // on Tauri and reveal the deck on web/extension. `workspace_open` is the
    // shared collapse state for the header toggle, the ⌘K `workspace-toggle`
    // command, and the pane itself; persisted to localStorage so a relaunch
    // restores it, defaulting open when the key is absent. Off-Tauri the pane
    // never mounts, so this is inert there.
    let in_tauri = crate::host::running_in_tauri();
    let daemon_for_island = StoredValue::new(daemon.clone());
    let palette_open = RwSignal::new(false);
    // Every Island entry point shares one overlay policy. Agent interaction,
    // session switching, and history Recall are mutually exclusive modes of one
    // titlebar object; they never stack into one dashboard or hide behind peers.
    let open_island = Callback::new(move |next: IslandMode| {
        palette_open.set(false);
        show_council.set(false);
        show_rooms.set(false);
        show_sessions.set(false);
        if next != IslandMode::Closed {
            island_focus_request.update(|request| *request = request.wrapping_add(1));
        }
        island_mode.set(next);
    });
    // Preserve mutual exclusion when another entry point (native menu, room
    // navigation, or a future deep link) opens a sibling overlay directly.
    // `open_island` clears those signals before setting the Island, so the
    // latest explicit opener wins without surfaces stacking invisibly.
    Effect::new(move |_| {
        if island_mode.get() != IslandMode::Closed
            && (show_council.get() || show_rooms.get() || show_sessions.get())
        {
            island_mode.set(IslandMode::Closed);
        }
    });
    let workspace_open: RwSignal<bool> = RwSignal::new(
        // Default OPEN when the key is absent; only an explicit "0" starts
        // collapsed (the pane is the shell's primary surface).
        ls_get(WORKSPACE_OPEN_KEY).map(|s| s != "0").unwrap_or(true),
    );
    // One-shot focus intent from the toggle-* commands (Tauri path): the pane
    // watches this and opens/focuses the matching tab, then resets to None.
    let workspace_focus: RwSignal<Option<WorkspaceFocus>> = RwSignal::new(None);
    let daemon_for_workspace = StoredValue::new(daemon.clone());
    // Pinned rail: StoredValue (Copy) so the root view! can hand the daemon
    // to <PinnedRail> without moving the plain clone out of the closure.
    let daemon_for_pinned = StoredValue::new(daemon.clone());

    // Persist the pane open/collapse state so a relaunch restores it. Runs
    // once at setup (writing the init value back — idempotent) and on every
    // change thereafter.
    Effect::new(move |_| {
        ls_set(
            WORKSPACE_OPEN_KEY,
            if workspace_open.get() { "1" } else { "0" },
        );
    });

    // Deep-menu registry (north star command layer): ONE registry drives the
    // ⌘K palette today and the native menubar + header overflow when they
    // land. Commands registered here are the integration wiring — modules
    // never self-register.
    let registry = CommandRegistry::new();
    let always = Signal::derive(|| true);
    let never = Signal::derive(|| false);
    {
        let daemon_new_session = daemon.clone();
        registry.register(Command {
            id: "new-session",
            title: "New Session".into(),
            hint: Some("reset transcript; created on first prompt".into()),
            scope: CommandScope::Session,
            slash: Some("/new"),
            enabled: always,
            run: Callback::new(move |_| daemon_new_session.new_session()),
        });
        registry.register(Command {
            id: "toggle-files",
            title: "Toggle Files Explorer".into(),
            hint: None,
            scope: CommandScope::Files,
            slash: Some("/files"),
            enabled: always,
            run: Callback::new(move |_| {
                if in_tauri {
                    workspace_open.set(true);
                    workspace_focus.set(Some(WorkspaceFocus::Files));
                } else {
                    toggle_deck(DeckPanel::Files);
                }
            }),
        });
        registry.register(Command {
            id: "toggle-repo",
            title: "Toggle Repo Panel".into(),
            hint: None,
            scope: CommandScope::Repo,
            slash: Some("/repo"),
            enabled: always,
            run: Callback::new(move |_| {
                if in_tauri {
                    workspace_open.set(true);
                    workspace_focus.set(Some(WorkspaceFocus::Repo));
                } else {
                    toggle_deck(DeckPanel::Repo);
                }
            }),
        });
        registry.register(Command {
            id: "toggle-browser",
            title: "Toggle Browser Cockpit".into(),
            hint: None,
            scope: CommandScope::Browser,
            slash: Some("/browser"),
            enabled: always,
            run: Callback::new(move |_| {
                if in_tauri {
                    workspace_open.set(true);
                    workspace_focus.set(Some(WorkspaceFocus::Browser));
                } else {
                    toggle_deck(DeckPanel::Browser);
                }
            }),
        });
        registry.register(Command {
            id: "toggle-sessions",
            title: "Toggle Sessions".into(),
            hint: None,
            scope: CommandScope::App,
            slash: Some("/sessions"),
            enabled: always,
            run: Callback::new(move |_| toggle_sessions()),
        });
        registry.register(Command {
            id: "focus-search",
            title: "Switch Session…".into(),
            hint: Some("⌘P".into()),
            scope: CommandScope::App,
            slash: None,
            enabled: Signal::derive(move || in_tauri),
            run: Callback::new(move |_| open_island.run(IslandMode::Sessions)),
        });
        registry.register(Command {
            id: "recall-history",
            title: "Recall History…".into(),
            hint: Some("⌘⇧F".into()),
            scope: CommandScope::App,
            slash: None,
            enabled: Signal::derive(move || in_tauri),
            run: Callback::new(move |_| open_island.run(IslandMode::Recall)),
        });
        registry.register(Command {
            id: "toggle-rooms",
            title: "Toggle Rooms".into(),
            hint: None,
            scope: CommandScope::App,
            slash: Some("/rooms"),
            enabled: always,
            run: Callback::new(move |_| toggle_rooms()),
        });
        registry.register(Command {
            id: "open-council",
            title: "Open Council Stage".into(),
            hint: None,
            scope: CommandScope::App,
            slash: Some("/council"),
            enabled: always,
            run: Callback::new(move |_| show_council.set(true)),
        });
        // Daemon supervision (Tauri shell only): start/restart the supervised
        // ocean-daemon. Hidden off-Tauri via the enabled predicate (the palette
        // drops disabled rows) — the browser PWA/extension talk to an
        // already-running daemon and never supervise one.
        let daemon_tauri = crate::host::running_in_tauri();
        registry.register(Command {
            id: "daemon-start",
            title: "Start Daemon".into(),
            hint: Some("native shell only".into()),
            scope: CommandScope::App,
            slash: None,
            enabled: Signal::derive(move || daemon_tauri),
            run: Callback::new(move |_| {
                wasm_bindgen_futures::spawn_local(async move {
                    crate::host::daemon_start(None).await;
                });
            }),
        });
        registry.register(Command {
            id: "daemon-restart",
            title: "Restart Daemon".into(),
            hint: Some("native shell only".into()),
            scope: CommandScope::App,
            slash: None,
            enabled: Signal::derive(move || daemon_tauri),
            run: Callback::new(move |_| {
                wasm_bindgen_futures::spawn_local(async move {
                    crate::host::daemon_restart(None).await;
                });
            }),
        });
        // Workspace pane toggle (Tauri shell only). The id matches the native
        // app-menu "Toggle Workspace" MenuItem — the orchestrator wires that
        // item afterward (no lib.rs edit here); on_menu_command above routes
        // the id back to this registry entry.
        registry.register(Command {
            id: "workspace-toggle",
            title: "Toggle Workspace".into(),
            hint: Some("native shell only".into()),
            scope: CommandScope::App,
            slash: Some("/workspace"),
            enabled: Signal::derive(move || in_tauri),
            run: Callback::new(move |_| workspace_open.update(|v| *v = !*v)),
        });
        // Composer `/` commands — Session-scoped, wired. The `run` callbacks
        // here are fallback status hints; the real dispatch for arg-taking
        // commands (`/model`, `/thinking`) lives in `run_slash`, and `/clear`
        // + `/help` delegate back to these callbacks via `registry.run`, so
        // the slash popover and the ⌘K palette run identical code.
        let daemon_clear = daemon.clone();
        registry.register(Command {
            id: "clear",
            title: "Clear transcript".into(),
            hint: None,
            scope: CommandScope::Session,
            slash: Some("/clear"),
            enabled: always,
            run: Callback::new(move |_| {
                daemon_clear.turns.set(Vec::new());
                daemon_clear.status.set("transcript cleared".into());
            }),
        });
        let daemon_model = daemon.clone();
        registry.register(Command {
            id: "model",
            title: "Set model".into(),
            hint: Some("/model <id>".into()),
            scope: CommandScope::Session,
            slash: Some("/model"),
            enabled: always,
            run: Callback::new(move |_| {
                daemon_model
                    .status
                    .set("use /model <id> or the selector below".into());
            }),
        });
        let daemon_thinking = daemon.clone();
        registry.register(Command {
            id: "thinking",
            title: "Set reasoning effort".into(),
            hint: Some("/thinking <level>".into()),
            scope: CommandScope::Session,
            slash: Some("/thinking"),
            enabled: always,
            run: Callback::new(move |_| {
                daemon_thinking
                    .status
                    .set("use /thinking off|minimal|low|medium|high|xhigh|default".into());
            }),
        });
        let daemon_help = daemon.clone();
        registry.register(Command {
            id: "help",
            title: "Show commands".into(),
            hint: None,
            scope: CommandScope::Session,
            slash: Some("/help"),
            enabled: always,
            run: Callback::new(move |_| daemon_help.status.set("type / to browse commands".into())),
        });
        // `/resume` stays registered as a disabled signpost: the row renders
        // greyed with a hint pointing at the sessions panel (the real resume
        // surface), and `registry.run` refuses it via the `enabled` predicate.
        registry.register(Command {
            id: "resume",
            title: "Resume a session".into(),
            hint: Some("use the sessions panel".into()),
            scope: CommandScope::Session,
            slash: Some("/resume"),
            enabled: never,
            run: Callback::new(|_| ()),
        });
    }

    // Composer `/` popover state. One reactive source of truth: `slash_query`
    // is the command-name token (text after the leading `/`, up to the first
    // space) so the menu keeps filtering while the user types args — e.g.
    // `/model gpt-5` keeps `/model` selected and passes `gpt-5` as the arg on
    // pick. `slash_items` mirrors registry order so the flat `slash_selected`
    // index lines up with `<SlashMenu>`'s row order. The menu is open while the
    // input is a leading-slash line with at least one matching command.
    let slash_selected: RwSignal<usize> = RwSignal::new(0);
    let slash_query = Signal::derive(move || {
        input
            .get()
            .strip_prefix('/')
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or("")
            .to_string()
    });
    let slash_items = Signal::derive({
        let registry = registry.clone();
        move || {
            let t = input.get();
            if !t.starts_with('/') {
                return Vec::new();
            }
            let q = slash_query.get();
            registry
                .slash_filter(&q)
                .into_iter()
                .map(|c| SlashRow {
                    id: c.id.to_string(),
                    title: c.title.clone(),
                    alias: c.slash.unwrap_or("").to_string(),
                    hint: c.hint.clone(),
                    group: scope_label(c.scope).to_string(),
                    enabled: c.enabled.get(),
                })
                .collect::<Vec<_>>()
        }
    });
    let slash_open =
        Signal::derive(move || input.get().starts_with('/') && !slash_items.get().is_empty());
    // One stable pick callback shared by the popover click + Send-button path.
    // Args are the text after the first whitespace (empty for bare commands).
    let on_slash_pick = Callback::new({
        let daemon = daemon.clone();
        let registry = registry.clone();
        move |id: String| {
            let args = input
                .get_untracked()
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .to_string();
            run_slash(&id, &args, &daemon, &registry);
            input.set(String::new());
        }
    });

    // Native app-menu bridge: the Tauri shell emits `menu-command` with a
    // command id when the user picks a "Commands" submenu item. Route every
    // selection through the registry — unknown ids no-op and disabled commands
    // are refused (see CommandRegistry::run). No-op off the Tauri shell, so
    // the browser PWA and extension simply never register a listener. The
    // effect body reads nothing reactive, so it runs once at mount.
    let menu_registry = registry.clone();
    Effect::new(move |_| {
        let reg = menu_registry.clone();
        crate::host::on_menu_command(move |id| {
            reg.run(&id);
        });
        // The subscriber is now registered — dispatched before the await
        // below on the same FIFO IPC channel, so it lands before the shell
        // drains `pending`. Tell the host it may replay any boot-time menu
        // clicks that fired pre-attach. No-op off the Tauri shell.
        wasm_bindgen_futures::spawn_local(crate::host::notify_ui_ready());
    });

    // TTS: speak the assistant's final text each time a turn finishes
    // (streaming flips true→false). Gated by `muted`. We track the previous
    // streaming value so we only fire on the falling edge, and remember the
    // last spoken turn so re-renders don't double-speak.
    let muted = RwSignal::new(false);
    let prev_streaming = RwSignal::new(false);
    let last_spoken: RwSignal<Option<String>> = RwSignal::new(None);
    Effect::new(move |_| {
        let now = streaming.get();
        let was = prev_streaming.get_untracked();
        prev_streaming.set(now);
        // Falling edge = a turn just completed.
        if was && !now {
            // Native OS notification when a turn finishes off-focus, so the
            // user is pulled back to the answer (OS-presence slice). Same
            // falling edge the TTS speak guards; independent of `muted`.
            if !window_focused() {
                let title = "Ocean".to_string();
                wasm_bindgen_futures::spawn_local(async move {
                    crate::host::notify(&title, "Turn complete").await;
                });
            }
            if let Some((id, text)) = latest_assistant_text(&turns.get_untracked()) {
                if last_spoken.get_untracked().as_deref() != Some(id.as_str()) {
                    last_spoken.set(Some(id));
                    crate::tts::speak(text, muted);
                }
            }
        }
    });

    // Dock/taskbar badge mirrors the pending permission-prompt count: a
    // non-zero count sets the macOS dock badge so a blocked tool decision is
    // noticed; zero clears it. No-op off the Tauri shell (host::set_badge
    // returns early on non-Tauri hosts).
    Effect::new(move |_| {
        let n = pending_permissions.with(|p| p.len() as i64);
        wasm_bindgen_futures::spawn_local(async move {
            crate::host::set_badge(if n > 0 { Some(n) } else { None }).await;
        });
    });

    // Deep links (ocean://...): the Tauri shell brings the window forward and
    // emits `deep-link` with the raw URL when the OS asks Ocean to open one.
    // Parse it into an action — v1 understands only `ocean://session/<id>`,
    // which reuses the exact path a SessionsPanel row click takes
    // (`Daemon::switch_session`): clear state, set the id, hydrate the
    // persisted transcript, reconnect the SSE tail. Title is fetched from the
    // GET /v1/sessions/<id> snapshot inside switch_session, so an empty title
    // here is overwritten once that returns. Unknown/unparseable URLs are
    // logged and dropped. No-op off the Tauri shell; the effect reads nothing
    // reactive, so it registers the listener once at mount (mirrors
    // on_menu_command above).
    let daemon_for_deeplink = daemon.clone();
    Effect::new(move |_| {
        let daemon = daemon_for_deeplink.clone();
        crate::host::on_deep_link(move |raw| match parse_deep_link(&raw) {
            Some(DeepLinkAction::SelectSession(id)) => {
                daemon.switch_session(id, String::new());
            }
            None => log::info!("ignoring unparseable ocean:// deep link: {raw}"),
        });
    });

    // Pointer light: ONE window mousemove listener feeds cursor position to
    // :root as viewport percentages. Opted-in surfaces (.ocean-lit, defined
    // in styles/base.css) paint a faint radial specular there so they read
    // as catching one overhead light source. Cheap direct set per event —
    // two custom properties, no rAF. Bound + on_cleanup so the listener
    // lives with the App scope and is torn down on unmount.
    let _pointer_light = window_event_listener(ev::mousemove, move |e: web_sys::MouseEvent| {
        let Some(win) = web_sys::window() else { return };
        let Some(w) = win.inner_width().ok().and_then(|v| v.as_f64()) else {
            return;
        };
        let Some(h) = win.inner_height().ok().and_then(|v| v.as_f64()) else {
            return;
        };
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let x = e.client_x() as f64 / w * 100.0;
        let y = e.client_y() as f64 / h * 100.0;
        let Some(doc) = win.document() else { return };
        let Some(root) = doc.document_element() else {
            return;
        };
        let Ok(root) = root.dyn_into::<web_sys::HtmlElement>() else {
            return;
        };
        let style = root.style();
        let _ = style.set_property("--pointer-x", &format!("{x:.2}%"));
        let _ = style.set_property("--pointer-y", &format!("{y:.2}%"));
    });
    on_cleanup(move || _pointer_light.remove());

    // Desktop Island shortcuts. Cmd/Ctrl+P opens the dedicated session
    // switcher; Cmd/Ctrl+Shift+F opens transcript Recall. Both are Tauri-only,
    // preserving browser/PWA Print and Find. Cmd/Ctrl+K closes the Island before
    // PaletteView handles the same event.
    let _island_shortcut = window_event_listener(ev::keydown, move |e: ev::KeyboardEvent| {
        if e.is_composing() {
            return;
        }
        let command = e.meta_key() || e.ctrl_key();
        if command
            && !e.shift_key()
            && !e.alt_key()
            && e.key().eq_ignore_ascii_case("k")
            && island_mode.get_untracked() != IslandMode::Closed
        {
            island_mode.set(IslandMode::Closed);
        }
        if in_tauri
            && command
            && !e.shift_key()
            && !e.alt_key()
            && e.key().eq_ignore_ascii_case("p")
        {
            e.prevent_default();
            e.stop_propagation();
            open_island.run(IslandMode::Sessions);
        } else if in_tauri
            && command
            && e.shift_key()
            && !e.alt_key()
            && e.key().eq_ignore_ascii_case("f")
        {
            e.prevent_default();
            e.stop_propagation();
            open_island.run(IslandMode::Recall);
        }
    });
    on_cleanup(move || _island_shortcut.remove());

    // Window-level Escape closes the topmost open reveal — priority follows
    // the z-order: council stage (full-screen modal) over the Island and rooms/sessions
    // browse overlays over the deck rail (web/extension only — the Show gate
    // keeps the deck off Tauri). The palette's and slash-menu's own Escape
    // arms call stop_propagation(), so an Escape they handle never bubbles
    // here: one Escape closes exactly one surface, never a cascade.
    let _overlay_escape = window_event_listener(ev::keydown, move |e: ev::KeyboardEvent| {
        if e.key() != "Escape" || e.is_composing() {
            return;
        }
        if palette_open.get() {
            palette_open.set(false);
        } else if show_council.get() {
            show_council.set(false);
        } else if island_mode.get() != IslandMode::Closed {
            island_mode.set(IslandMode::Closed);
        } else if show_rooms.get() {
            show_rooms.set(false);
        } else if show_sessions.get() {
            show_sessions.set(false);
        } else if deck_panel.get().is_some() {
            deck_panel.set(None);
        }
    });
    on_cleanup(move || _overlay_escape.remove());

    let submit = {
        let daemon = daemon.clone();
        let registry = registry.clone();
        move |ev: SubmitEvent| {
            ev.prevent_default();
            let text = input.get_untracked();
            if text.trim().is_empty() {
                return;
            }
            input.set(String::new());
            // A `/`-prefixed input is a slash command, never a prompt. Route
            // it through the same dispatcher the popover uses (best subseq
            // match on the command-name token) so clicking Send on `/model
            // gpt-5` behaves like pressing Enter in the menu; an unknown or
            // disabled token clears with a hint. This closes the path the
            // textarea keydown guard can't reach (the submit button).
            if text.starts_with('/') {
                let rest = text.strip_prefix('/').unwrap_or("");
                let name = rest.split_whitespace().next().unwrap_or("");
                let args = rest.split_whitespace().nth(1).unwrap_or("");
                match registry.slash_filter(name).into_iter().next() {
                    Some(cmd) if cmd.enabled.get_untracked() => {
                        run_slash(cmd.id, args, &daemon, &registry);
                    }
                    _ => daemon
                        .status
                        .set("unknown command \u{2014} type / to see them".into()),
                }
            } else {
                daemon.send_prompt(text);
            }
            // Refocus + collapse the textarea so a long prior prompt doesn't
            // leave the next turn trapped in a tall empty scrollbox.
            if let Some(el) = textarea_ref.get_untracked() {
                reset_composer_textarea(&el);
                let _ = el.focus();
            }
        }
    };

    // Wrap submit in a StoredValue so it can be shared across closures
    // without being consumed (the composer's submit handler needs it).
    let submit = StoredValue::new(submit);

    // Clone reserved for the SessionsPanel.
    let daemon_for_panel = daemon.clone();

    // Permission-approval overlay (OCEAN-64). Stored (Copy) so it can be handed
    // a fresh clone wherever the component is mounted without moving the main
    // `daemon` out of scope.
    let daemon_for_perms = StoredValue::new(daemon.clone());

    // Voice → text: drop the transcript into the composer and submit it via the
    // voice send path, which tags the turn `client_type="leo-voice"` so the
    // daemon applies its concise, speakable voice system prompt (OCEAN-181).
    // Otherwise the transcript would be tagged like a typed message.
    let on_transcript = {
        let daemon = daemon.clone();
        Callback::new(move |text: String| {
            let text = text.trim().to_string();
            if text.is_empty() {
                return;
            }
            input.set(text.clone());
            daemon.send_voice_prompt(text);
            input.set(String::new());
        })
    };
    let on_voice_status = Callback::new(move |msg: String| status.set(msg));

    // Dictate mode: transcript lands in the composer for review rather than
    // auto-sending. VoiceOrb routes to this when Dictate is active.
    let on_dictate = Callback::new(move |text: String| {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        input.update(|s| {
            if !s.is_empty() && !s.ends_with(' ') {
                s.push(' ');
            }
            s.push_str(&text);
        });
    });

    // Clones for the composer's per-turn override controls (OCEAN-79). These
    // controls live INSIDE the chat-branch <Show> fallback, which must be `Fn`,
    // so they go through StoredValue (Copy) — a plain clone would be moved out of
    // the fallback environment and make it `FnOnce`.
    let daemon_thinking = StoredValue::new(daemon.clone());
    let daemon_model_override = StoredValue::new(daemon.clone());
    // StoredValue is Copy, so the halt button's closure (inside the chat-branch
    // <Show> fallback, which must be Fn) can grab the daemon without the
    // fallback moving a plain clone out of its environment.
    let daemon_halt = StoredValue::new(daemon.clone());
    // Screenshot capture button (OCEAN-138): StoredValue (Copy) so the on:click
    // closure can grab the daemon to stage the captured image for the next turn.
    let daemon_capture = StoredValue::new(daemon.clone());
    // Header overflow menu (<details>): council/rooms/mute/capture live behind
    // one "⋯" affordance instead of a row of buttons. Item clicks close it.
    let more_ref: NodeRef<leptos::html::Details> = NodeRef::new();
    // Call/collaboration controls are explicit reveals, not permanent top
    // chrome. The overflow menu opens them; the row below the header exists only
    // while one is intentionally active.
    let show_phone_dialer = RwSignal::new(false);
    let daemon_livekit = StoredValue::new(daemon.clone());
    let daemon_phone_call = StoredValue::new(daemon.clone());

    // Realtime voice-chat layout (voice phases 2/3): components rendered
    // BEFORE voice started must not pre-dock a new session. Capture the
    // baseline count when we first transition out of Off; dock only when
    // the live component count exceeds that baseline.
    let rt_stage = crate::voice::realtime::stage();
    let rt_stage_for_baseline = rt_stage.clone();
    let rt_current_component_count = {
        let rt_turns = daemon.turns;
        Memo::new(move |_| {
            rt_turns.with(|t| {
                t.iter()
                    .flat_map(|turn| turn.blocks.iter())
                    .filter(|b| matches!(b, crate::model::Block::Component { .. }))
                    .count()
            })
        })
    };
    let rt_baseline_component_count = RwSignal::new(None::<usize>);

    // Snapshot the component count when voice transitions out of Off.
    let _ = {
        let rt_stage = rt_stage_for_baseline;
        let rt_count = rt_current_component_count;
        let rt_baseline = rt_baseline_component_count;
        Effect::new(move |_| {
            let stage = rt_stage.get();
            if stage != crate::voice::realtime::RealtimeStage::Off {
                let current = rt_count.get();
                // Only snap once per fresh start (None means not yet captured).
                if rt_baseline.get_untracked().is_none() {
                    rt_baseline.set(Some(current));
                }
            } else {
                rt_baseline.set(None);
            }
        })
    };

    let rt_stage_for_active = rt_stage.clone();
    let rt_stage_for_docked = rt_stage;
    let voice_chat_active = move || {
        let layout = surface_voice_layout(
            rt_stage_for_active.get(),
            rt_baseline_component_count.get(),
            rt_current_component_count.get(),
        );
        layout.center_stage
    };
    let voice_chat_docked = move || {
        let layout = surface_voice_layout(
            rt_stage_for_docked.get(),
            rt_baseline_component_count.get(),
            rt_current_component_count.get(),
        );
        layout.docked
    };
    let voice_level_style = move || format!("{:.3}", crate::voice::realtime::level().get());

    // In the Chrome side panel the cockpit lives in a ~360px-wide column. Tag
    // the root so the shared stylesheet's compact `.ocean-surface--extension`
    // rules apply, without forking the layout for the full-width web app.
    let root_class = if crate::daemon::running_as_extension() {
        "ocean-surface ocean-surface--extension"
    } else {
        "ocean-surface"
    };

    // Clone before the root view! so nested closures / children can each
    // take their own handle without moving the outer registry.
    let registry_for_view = registry.clone();
    view! {
        <main
            class=root_class
            class:has-workspace-open=move || in_tauri && workspace_open.get()
            // Desktop-only: the header doubles as the window titlebar (Tauri
            // overlay traffic lights float over it) — pads the brand clear.
            class:is-titlebar=in_tauri
            class:voice-chat-active=voice_chat_active
            class:voice-chat-docked=voice_chat_docked
            style=("--voice-level", voice_level_style)
        >
            // `data-tauri-drag-region` applies only to the element it is ON
            // (not descendants): set it on both the header and the brand so
            // the whole left zone drags the window on desktop; inert on web.
            <header class="ocean-header" data-tauri-drag-region="">
                <div class="ocean-header__left" data-tauri-drag-region="">
                    <div class="ocean-brand" aria-label="Ocean" data-tauri-drag-region="">
                        <crate::icons::WaveBadge spinning=false compact=true />
                        <span class="ocean-brand__word" aria-hidden="true">
                            <crate::icons::OceanWordmark />
                        </span>
                    </div>
                </div>
                <Show when=move || in_tauri>
                    <DynamicIsland
                        daemon=daemon_for_island.get_value()
                        mode=island_mode
                        focus_request=island_focus_request
                        on_open=open_island
                    />
                </Show>
                <div class="ocean-header__right">
                    // Web and extension keep the existing Sessions modal entry;
                    // Tauri uses the centered Island while the registry command
                    // remains the deep-browse fallback on every host.
                    <Show when=move || !in_tauri>
                        <button
                            class="ocean-sessions-trigger"
                            type="button"
                            aria-label="sessions"
                            title="Sessions"
                            on:click=move |_| toggle_sessions()
                        >
                            "Sessions"
                        </button>
                    </Show>
                    // Ambient runtime readouts — token usage, the browser-driving
                    // cue, and connection status — grouped into one demoted cluster
                    // so they read as secondary telemetry, not equal-weight peers
                    // to the primary header controls.
                    <div class="ocean-runtime">
                    // Token usage: session total, with a per-turn + cache
                    // breakdown on hover. Hidden until the first turn finishes.
                    <Show when=has_tokens>
                        <div
                            class="ocean-tokens"
                            title=move || {
                                let s = session_tokens.get();
                                let last = last_turn_tokens.get().unwrap_or_default();
                                format!(
                                    "Session — in {} · out {} · cache {} · total {}\nLast turn — in {} · out {} · {:.1} tok/s",
                                    s.input, s.output, s.cache_read, s.total(),
                                    last.input, last.output, last.tokens_per_second,
                                )
                            }
                        >
                            <span class="ocean-tokens__io">
                                {move || {
                                    let s = session_tokens.get();
                                    format!("↑{} ↓{}", fmt_tokens(s.input), fmt_tokens(s.output))
                                }}
                            </span>
                            <Show when=has_rate>
                                <span class="ocean-tokens__rate">
                                    {move || format!("{:.0} t/s", last_turn_tokens.get().unwrap_or_default().tokens_per_second)}
                                </span>
                            </Show>
                        </div>
                    </Show>
                    // Browser-control indicator (OCEAN-92). Visible only while
                    // Ocean is driving the browser; shows the last browser action
                    // (e.g. "navigate", "click") so the user sees what's happening
                    // in their tab. Driven by the daemon's browser_activity stream.
                    <Show when=move || browser_active.get()>
                        <div
                            class="ocean-browser-control"
                            title=move || match browser_last_action.get() {
                                Some(a) => format!("Ocean is driving the browser — last action: {a}"),
                                None => "Ocean is driving the browser".to_string(),
                            }
                        >
                            <span class="ocean-browser-control__dot"></span>
                            <span class="ocean-browser-control__label">
                                {move || match browser_last_action.get() {
                                    Some(a) => format!(
                                        "driving · {}",
                                        a.strip_prefix("browser_").unwrap_or(&a),
                                    ),
                                    None => "driving browser".to_string(),
                                }}
                            </span>
                        </div>
                    </Show>
                    // Tooltip carries the full raw payload, but only while the
                    // displayed status is the exact string stored alongside it —
                    // any later status.set (error or benign) drops the tooltip
                    // instead of leaking a stale payload.
                    <div
                        class="ocean-status"
                        class:is-quiet=move || matches!(
                            status.get().as_str(),
                            "connected" | "new session" | "session loaded"
                        )
                        aria-label=move || format!("status: {}", status.get())
                        title=move || {
                            status_detail
                                .get()
                                .filter(|(s, _)| *s == status.get())
                                .map(|(_, detail)| detail)
                                .unwrap_or_else(|| status.get())
                        }
                    >
                        <span class="ocean-status__dot"></span>
                        <span class="ocean-status__text">{move || status.get()}</span>
                    </div>
                    // Daemon-supervision indicator (Tauri shell only). Shown
                    // ONLY when the shell reports the daemon process is down
                    // (stopped/unreachable) — explaining why the connection chip
                    // above is also failing, and pointing at the palette/tray to
                    // start it. Hidden whenever the daemon is up or off-Tauri
                    // (signal is None), so it adds no permanent chrome.
                    <Show when=move || {
                        daemon_shell_status
                            .get()
                            .map(|s| matches!(s.state.as_str(), "stopped" | "unreachable"))
                            .unwrap_or(false)
                    }>
                        <div
                            class="ocean-status"
                            title="The ocean-daemon process isn't running. Start it via ⌘K → Start Daemon or the tray menu."
                        >
                            <span class="ocean-status__dot"></span>
                            <span class="ocean-status__text">"daemon offline"</span>
                        </div>
                    </Show>
                    </div>
                    // Secondary actions live behind one overflow control:
                    // council deck, rooms, voice mute, extension tab capture.
                    // Death-by-buttons is a design defect; the header keeps
                    // exactly one icon button (sessions) plus this "⋯".
                    <details class="ocean-more" node_ref=more_ref>
                        <summary class="ocean-more__btn" aria-label="more actions" title="More">
                            "⋯"
                        </summary>
                        <div class="ocean-more__menu" role="menu">
                            <Show when=move || !livekit_token_path.get().trim().is_empty()>
                                <button
                                    class="ocean-more__item"
                                    type="button"
                                    role="menuitem"
                                    on:click=move |_| {
                                        if let Some(d) = more_ref.get() { let _ = d.remove_attribute("open"); }
                                        show_phone_dialer.set(false);
                                        show_livekit_controls.set(true);
                                    }
                                >
                                    "Join room call"
                                </button>
                            </Show>
                            <button
                                class="ocean-more__item"
                                type="button"
                                role="menuitem"
                                on:click=move |_| {
                                    if let Some(d) = more_ref.get() { let _ = d.remove_attribute("open"); }
                                    show_livekit_controls.set(false);
                                    show_phone_dialer.set(true);
                                }
                            >
                                "Dial phone"
                            </button>
                            <button
                                class="ocean-more__item"
                                type="button"
                                role="menuitem"
                                on:click=move |_| {
                                    if let Some(d) = more_ref.get() { let _ = d.remove_attribute("open"); }
                                    show_council.set(true);
                                }
                            >
                                "Council deck"
                            </button>
                            <button
                                class="ocean-more__item"
                                type="button"
                                role="menuitem"
                                on:click=move |_| {
                                    if let Some(d) = more_ref.get() { let _ = d.remove_attribute("open"); }
                                    toggle_rooms();
                                }
                            >
                                "Rooms"
                            </button>
                            <Show when=crate::daemon::running_as_extension>
                                <button
                                    class="ocean-more__item"
                                    type="button"
                                    role="menuitem"
                                    on:click=move |_| {
                                        if let Some(d) = more_ref.get() { let _ = d.remove_attribute("open"); }
                                        daemon_capture.get_value().capture_and_attach_visible_tab();
                                    }
                                >
                                    "Capture tab"
                                </button>
                            </Show>
                        </div>
                    </details>
                    // Workspace pane collapse toggle (Tauri shell only). Slim
                    // chevron at the header's right edge — the pane docks right,
                    // so the toggle sits at the boundary. Chevrons point toward
                    // the edge the pane slides to: open shows "›" (collapse to
                    // the right), collapsed shows "‹" (reveal from the right).
                    <Show when=move || in_tauri>
                        <button
                            class="ocean-workspace-toggle"
                            type="button"
                            aria-label="toggle workspace"
                            title=move || if workspace_open.get() { "Hide workspace" } else { "Show workspace" }
                            on:click=move |_| workspace_open.update(|v| *v = !*v)
                        >
                            {move || if workspace_open.get() { "›" } else { "‹" }}
                        </button>
                    </Show>
                </div>
            </header>

            // Chat surface. (The Leptos component "gauntlet" toggle was removed
            // in OCEAN-202 — it was a dev-only component harness, not shipping UI.)
            // Call/collaboration utility line. Hidden while idle so the app has
            // one top chrome bar, not a second row of quiet-but-visible buttons.
            <Show when=move || show_phone_dialer.get()>
                <div class="ocean-utility-row">
                    // Place-call control (OCEAN-261): revealed from overflow.
                    // On PSTN success, CallPanel below takes over from the
                    // daemon's call_started event.
                    <crate::place_call::PlaceCallControl
                        daemon=daemon_phone_call.get_value()
                        open=show_phone_dialer
                    />
                </div>
            </Show>

            // LiveKit collaboration presence (OCEAN-83): the compact call
            // strip (join/leave, mic, camera, roster). In room mode it reads
            // as the room's call upgrade — it lights up when a room join
            // routes the shared LiveKit signals and credentials exist. Never
            // double-mount the singleton bridge.
            <crate::livekit::LiveKitPanel
                daemon=daemon_livekit.get_value()
                open=show_livekit_controls
            />

            // Room mode: the entered room takes the surface over in place of
            // the chat transcript + composer below.
            <Show when=move || in_room_mode.get()>
                <RoomStage rooms=rooms />
            </Show>

            <Show when=move || !in_room_mode.get()>

                        // Live call-mode view (OCEAN-CALL). Self-contained: it
                        // subscribes to the daemon's `/v1/events` control stream
                        // for the `call_*` frames and stays hidden until a
                        // `call_started` arrives, then shows the live transcript,
                        // rolling summary, detected action items, and wake orb;
                        // it collapses again on `call_ended`. Purely additive.
                        <crate::call::CallPanel daemon=daemon.clone() />

                        <Transcript daemon=daemon.clone() show_sessions=show_sessions />

                        // Agent canvas (OCEAN-178 → OCEAN-248). Folds the
                        // daemon's `surface_patch` stream into a client-side
                        // ledger and renders it spatially — positioned cards +
                        // SVG edges — instead of a text changelog. The GPUI
                        // native shell renders the full interactive canvas.
                        <crate::canvas::CanvasRender canvas_patches=canvas_patches />

                        // Blocking permission prompts sit just above the composer
                        // so a gated mutating turn can't be missed or scrolled past.
                        <PermissionPrompts daemon=daemon_for_perms.get_value() />

                        <form class="ocean-composer ocean-lit" style:position="relative" on:submit=move |ev| submit.with_value(|s| s(ev))>
                            // Push-to-talk only when the proxy has a usable xAI key;
                            // otherwise a dim, disabled placeholder explains why.
                            <Show
                                when=move || voice_ready.get()
                                fallback=|| view! {
                                    <div class="voice-wrap">
                                        <button class="voice-orb is-disabled" type="button" disabled=true
                                                title="voice off — set xAI key in ~/.config/ocean-surface/xai.key">
                                            <span class="voice-orb__glyph"><crate::icons::Mic /></span>
                                        </button>
                                        <span class="voice-hint">"voice off"</span>
                                    </div>
                                }
                            >
                                <VoiceOrb on_transcript=on_transcript on_status=on_voice_status muted=muted on_dictate=on_dictate />
                            </Show>
                            // Per-turn overrides (OCEAN-79): reasoning effort +
                            // model. Compact pills next to the composer. Both
                            // default to "daemon default" so an untouched control
                            // sends no override and preserves prior behavior.
                            <div class="ocean-turn-controls">
                                <select
                                    class="ocean-thinking"
                                    aria-label="reasoning effort"
                                    title="Reasoning effort (this turn onward)"
                                    prop:value=move || thinking_level.get().unwrap_or_default()
                                    on:change=move |ev| {
                                        let v = event_target_value(&ev);
                                        daemon_thinking.with_value(|d| {
                                            d.set_thinking_level((!v.is_empty()).then_some(v))
                                        });
                                    }
                                >
                                    // Values map 1:1 to ocean_protocol::ThinkingLevel
                                    // (serde lowercase): off | minimal | low | medium
                                    // | high | xhigh. Empty = no override (daemon
                                    // default). These are the exact levels the daemon
                                    // accepts — anything else round-trips to a serde
                                    // error. (OCEAN-202)
                                    <option value="" prop:selected=move || thinking_level.get().is_none()>
                                        "think: default"
                                    </option>
                                    <option value="off" prop:selected=move || thinking_level.get().as_deref() == Some("off")>
                                        "think: off"
                                    </option>
                                    <option value="minimal" prop:selected=move || thinking_level.get().as_deref() == Some("minimal")>
                                        "think: minimal"
                                    </option>
                                    <option value="low" prop:selected=move || thinking_level.get().as_deref() == Some("low")>
                                        "think: low"
                                    </option>
                                    <option value="medium" prop:selected=move || thinking_level.get().as_deref() == Some("medium")>
                                        "think: medium"
                                    </option>
                                    <option value="high" prop:selected=move || thinking_level.get().as_deref() == Some("high")>
                                        "think: high"
                                    </option>
                                    <option value="xhigh" prop:selected=move || thinking_level.get().as_deref() == Some("xhigh")>
                                        "think: xhigh"
                                    </option>
                                    // Unknown persisted value (stale pref, daemon
                                    // drift): still render it selected — the same
                                    // guard the model select has. Without this the
                                    // controlled select desyncs and renders BLANK.
                                    <Show when=move || {
                                        matches!(
                                            thinking_level.get().as_deref(),
                                            Some(v) if !matches!(v, "off" | "minimal" | "low" | "medium" | "high" | "xhigh")
                                        )
                                    }>
                                        <option prop:value=move || thinking_level.get().unwrap_or_default() prop:selected=true>
                                            {move || format!("think: {}", thinking_level.get().unwrap_or_default())}
                                        </option>
                                    </Show>
                                </select>
                                // Per-turn model override (distinct from the
                                // header picker's global swap). Drawn from the
                                // same /v1/models catalogue.
                                <select
                                    class="ocean-model-override"
                                    aria-label="model override"
                                    title="Model for this turn (overrides daemon default)"
                                    prop:value=move || model_override.get().unwrap_or_default()
                                    on:change=move |ev| {
                                        let id = event_target_value(&ev);
                                        daemon_model_override.with_value(|d| {
                                            d.set_model_override((!id.is_empty()).then_some(id))
                                        });
                                    }
                                >
                                    <option prop:value="" prop:selected=move || model_override.get().is_none()>
                                        "model: default"
                                    </option>
                                    // If a persisted override isn't in the
                                    // catalogue yet, still show it selected.
                                    <Show when=move || {
                                        let cur = model_override.get();
                                        cur.is_some()
                                            && !models.get().iter().any(|m| Some(&m.id) == cur.as_ref())
                                    }>
                                        <option prop:value=move || model_override.get().unwrap_or_default() prop:selected=true>
                                            {move || model_override.get().unwrap_or_default()}
                                        </option>
                                    </Show>
                                    <For
                                        each=move || models.get()
                                        key=|m| m.id.clone()
                                        children=move |m| {
                                            let id = m.id.clone();
                                            let id_sel = m.id.clone();
                                            let label = if m.label.is_empty() { m.id.clone() } else { m.label.clone() };
                                            view! {
                                                <option
                                                    prop:value=id.clone()
                                                    prop:selected=move || model_override.get().as_deref() == Some(id_sel.as_str())
                                                >
                                                    {label}
                                                </option>
                                            }
                                        }
                                    />
                                </select>
                            </div>
                            {move || {
                                // Reactive (not `<Show>`) so the plain Vec<usize
                                // props re-evaluate every keystroke: the list
                                // refines as the query narrows and the highlight
                                // tracks arrow-key selection.
                                if !slash_open.get() {
                                    return None;
                                }
                                let items = slash_items.get();
                                if items.is_empty() {
                                    return None;
                                }
                                let selected = slash_selected
                                    .get()
                                    .min(items.len().saturating_sub(1));
                                Some(view! {
                                    <SlashMenu items selected on_pick=on_slash_pick />
                                })
                            }}
                            <textarea
                                class="ocean-composer__input"
                                rows="1"
                                placeholder="message Ocean…"
                                node_ref=textarea_ref
                                prop:value=move || input.get()
                                on:input=move |ev| {
                                    input.set(event_target_value(&ev));
                                    if let Some(target) = ev.target() {
                                        if let Ok(el) = target.dyn_into::<web_sys::HtmlTextAreaElement>() {
                                            fit_composer_textarea(&el);
                                        }
                                    }
                                }
                                on:keydown={
                                    let daemon = daemon.clone();
                                    let registry = registry.clone();
                                    move |ev| {
                                        let key = ev.key();
                                        // IME candidate navigation owns every key
                                        // while composition is active, including
                                        // slash-menu arrows, Enter, and Tab.
                                        if ev.is_composing() {
                                            return;
                                        }
                                        let text = input.get_untracked();
                                        let items = slash_items.get_untracked();
                                        // While the input is a leading-slash line
                                        // with matching commands the popover drives:
                                        // arrows move selection, Enter/Tab pick,
                                        // Escape dismisses \u{2014} none fall
                                        // through to submit.
                                        if text.starts_with('/') && !items.is_empty() {
                                            let len = items.len();
                                            match key.as_str() {
                                                "ArrowDown" => {
                                                    ev.prevent_default();
                                                    slash_selected
                                                        .update(|i| *i = (*i + 1) % len);
                                                    return;
                                                }
                                                "ArrowUp" => {
                                                    ev.prevent_default();
                                                    slash_selected.update(|i| {
                                                        *i = if *i == 0 {
                                                            len.saturating_sub(1)
                                                        } else {
                                                            *i - 1
                                                        }
                                                    });
                                                    return;
                                                }
                                                "Enter" | "Tab" => {
                                                    ev.prevent_default();
                                                    let idx = slash_selected
                                                        .get_untracked()
                                                        .min(len.saturating_sub(1));
                                                    let row = &items[idx];
                                                    if row.enabled {
                                                        let args = text
                                                            .split_whitespace()
                                                            .nth(1)
                                                            .unwrap_or("")
                                                            .to_string();
                                                        run_slash(
                                                            &row.id,
                                                            &args,
                                                            &daemon,
                                                            &registry,
                                                        );
                                                    } else {
                                                        daemon.status.set(
                                                            "unknown command \u{2014} type / to see them"
                                                                .into(),
                                                        );
                                                    }
                                                    input.set(String::new());
                                                    return;
                                                }
                                                "Escape" => {
                                                    ev.prevent_default();
                                                    // Stop propagation so the
                                                    // window-level Escape
                                                    // (which closes the deck)
                                                    // doesn't also fire — one
                                                    // Escape clears the slash
                                                    // menu only, no cascade.
                                                    ev.stop_propagation();
                                                    input.set(String::new());
                                                    return;
                                                }
                                                _ => {}
                                            }
                                        }
                                        // Enter submits and Shift+Enter inserts a
                                        // newline, but composition Enter belongs to
                                        // the IME candidate picker and must never
                                        // leak through as a form submission.
                                        if should_submit_composer_key(
                                            &key,
                                            ev.shift_key(),
                                            ev.is_composing(),
                                        ) {
                                            ev.prevent_default();
                                            if let Some(target) = ev.target() {
                                                if let Ok(el) = target.dyn_into::<web_sys::HtmlElement>() {
                                                    if let Ok(Some(form)) = el.closest("form") {
                                                        if let Ok(form) = form.dyn_into::<web_sys::HtmlFormElement>()
                                                        {
                                                            let _ = form.request_submit();
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            />
                            // Trailing action — one circular slot. Streaming shows Stop
                            // (halt the in-flight turn); otherwise Send (submit,
                            // disabled while empty).
                            <Show
                                when=move || streaming.get()
                                fallback=move || view! {
                                    <button
                                        class="ocean-composer__send"
                                        type="submit"
                                        aria-label="send"
                                        title="Send"
                                        disabled=move || input.get().trim().is_empty()
                                    >
                                        <crate::icons::Send />
                                    </button>
                                }
                            >
                                <button
                                    class="ocean-composer__halt"
                                    type="button"
                                    aria-label="stop"
                                    title="Stop the running turn"
                                    on:click=move |_| daemon_halt.with_value(|d| d.halt())
                                >
                                    <crate::icons::Stop />
                                </button>
                            </Show>
                        </form>

            </Show>

            <SessionsPanel daemon=daemon_for_panel open=show_sessions />

            // Persistent Rooms panel (OCEAN-108). Lightweight browse/create
            // overlay only — a successful join closes this panel and promotes
            // the main surface into room mode.
            <RoomsPanel rooms=rooms open=show_rooms />

            // Context deck (north star): the web/extension reveal rail. On
            // Tauri it can never mount (hard-gated on !in_tauri) — the desktop
            // shell's surfaces live in the workspace pane below.
            <Show when=move || deck_panel.get().is_some() && !in_tauri>
                <aside class="deck ocean-lit" role="complementary" aria-label="Context deck">
                    <div class="deck__bar">
                        <span class="deck__title">
                            {move || deck_panel.get().map(|p| p.title()).unwrap_or("")}
                        </span>
                        <button
                            class="deck__close"
                            type="button"
                            aria-label="close deck"
                            title="Close"
                            on:click=move |_| deck_panel.set(None)
                        >
                            "✕"
                        </button>
                    </div>
                    <div class="deck__body">
                        {move || match deck_panel.get() {
                            Some(DeckPanel::Files) => {
                                view! { <FilesPanel daemon=daemon_for_deck.get_value() /> }.into_any()
                            }
                            Some(DeckPanel::Repo) => {
                                view! { <RepoPanel daemon=daemon_for_deck.get_value() /> }.into_any()
                            }
                            Some(DeckPanel::Browser) => {
                                view! { <BrowserCockpit daemon=daemon_for_deck.get_value() /> }.into_any()
                            }
                            None => ().into_any(),
                        }}
                    </div>
                </aside>
            </Show>

            // Workspace pane (north star desktop shell): THE right-side
            // surface on Tauri, permanent + tabbed (Files · previews · Browser
            // · Repo). position:fixed (see styles/workspace.css) so DOM order
            // is flexible; it docks right of the transcript via the shell's
            // `has-workspace-open` gutter on wide viewports. The web/extension
            // layout is untouched — the pane never mounts there.
            // `focus_intent` carries one-shot tab-focus intents from the
            // toggle-* commands.
            <Show when=move || in_tauri>
                <crate::workspace::WorkspacePane
                    daemon=daemon_for_workspace.get_value()
                    open=workspace_open
                    focus_intent=workspace_focus
                />
            </Show>

            // Pinned rail (north star): widgets the agent docked with
            // `props.placement == "pinned"` — a left-side persistent dock that
            // renders nothing when empty (see `PinnedRail`). position:fixed
            // (styles/panels.css) so it rides the free viewport margin beside
            // the centered shell, mirroring the right-side workspace pane.
            <PinnedRail daemon=daemon_for_pinned.get_value() />

            // ⌘K command palette — the deep-menu engine over the registry.
            <PaletteView registry=registry_for_view open=palette_open />

            // Council/quorum observability deck (OCEAN-96). Native workflow
            // stage now lives inside the surface instead of an iframe.
            <Show when=move || show_council.get()>
                <div class="ocean-council-modal" role="dialog" aria-label="Council stage">
                    <div class="ocean-council-modal__bar">
                        <span class="ocean-council-modal__title">"Council — workflow stage"</span>
                        <button
                            class="ocean-council-modal__close"
                            type="button"
                            aria-label="close council stage"
                            title="Close"
                            on:click=move |_| show_council.set(false)
                        >
                            "✕"
                        </button>
                    </div>
                    <crate::council::CouncilStage daemon=daemon_council.clone() />
                </div>
            </Show>
        </main>
    }
}

/// Pull the most recent assistant turn's concatenated text blocks, paired
/// with its turn id (used to dedupe TTS). Skips thinking + tool output.
fn latest_assistant_text(turns: &[Turn]) -> Option<(String, String)> {
    let turn = turns.iter().rev().find(|t| t.role == Role::Assistant)?;
    let id = turn.turn_id.clone()?;
    let mut text = String::new();
    for block in &turn.blocks {
        if let Block::Text(buf) = block {
            text.push_str(buf);
        }
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some((id, text))
    }
}

/// Humanize a token count for the header chip: 942 → "942", 12_345 → "12.3k",
/// 1_580_000 → "1.6M". Keeps the readout compact.
fn fmt_tokens(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

// ── deep links (ocean://) ───────────────────────────────────────────────

/// What a parsed `ocean://` deep link asks the surface to do.
///
/// The Tauri shell forwards each opened URL (host.rs `on_deep_link`); the
/// surface decides what it means. v1 supports only session selection.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DeepLinkAction {
    /// `ocean://session/<id>` — switch to the session with this id.
    SelectSession(String),
}

/// Parse an `ocean://` deep-link URL into a [`DeepLinkAction`].
///
/// The single supported shape is `ocean://session/<id>`, mapping to
/// [`DeepLinkAction::SelectSession`]. The host must be exactly `session` and
/// the path exactly one non-empty segment (the session id); a trailing query
/// (`?…`) or fragment ("#…") is allowed and ignored. Anything else returns
/// `None` so the caller logs and drops it — an unknown scheme/host/shape is
/// not an error, just not something v1 acts on.
///
/// Pure on purpose: no WASM, fully unit-testable on the native target.
pub(crate) fn parse_deep_link(raw: &str) -> Option<DeepLinkAction> {
    // Drop any query ("?…") / fragment ("#…") so `ocean://session/abc?ref=x`
    // resolves to the same action as the bare URL.
    let path = raw.split(['?', '#']).next().unwrap_or("");
    let rest = path.strip_prefix("ocean://session/")?;
    // The id is everything after the prefix. Reject an empty id and a
    // multi-segment path (`ocean://session/a/b`) — a session id is atomic.
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    Some(DeepLinkAction::SelectSession(rest.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        composer_height_px, composer_overflow_y, parse_deep_link, should_submit_composer_key,
        DeepLinkAction, COMPOSER_MAX_HEIGHT_PX, COMPOSER_MIN_HEIGHT_PX,
    };

    #[test]
    fn composer_height_clamps_to_min_and_max() {
        assert_eq!(composer_height_px(0), COMPOSER_MIN_HEIGHT_PX);
        assert_eq!(composer_height_px(72), 72);
        assert_eq!(composer_height_px(999), COMPOSER_MAX_HEIGHT_PX);
    }

    #[test]
    fn composer_overflow_switches_only_past_max_height() {
        assert_eq!(composer_overflow_y(COMPOSER_MAX_HEIGHT_PX), "hidden");
        assert_eq!(composer_overflow_y(COMPOSER_MAX_HEIGHT_PX + 1), "auto");
    }

    #[test]
    fn composer_enter_respects_newlines_and_ime_composition() {
        assert!(should_submit_composer_key("Enter", false, false));
        assert!(!should_submit_composer_key("Enter", true, false));
        assert!(!should_submit_composer_key("Enter", false, true));
        assert!(!should_submit_composer_key("a", false, false));
    }

    #[test]
    fn realtime_layout_docks_only_for_components_rendered_after_voice_start() {
        // Wished-for production seam: app.rs should compute the voice-chat root
        // classes from stage + the component count captured when voice started,
        // not from "any component exists in the transcript". A pre-existing card
        // must not pre-dock a new realtime session before the voice agent renders
        // anything in that session.
        let no_new_components = super::surface_voice_layout(
            crate::voice::realtime::RealtimeStage::Connecting,
            Some(2),
            2,
        );
        assert!(no_new_components.center_stage);
        assert!(!no_new_components.docked);

        let component_rendered_during_voice =
            super::surface_voice_layout(crate::voice::realtime::RealtimeStage::Live, Some(2), 3);
        assert!(!component_rendered_during_voice.center_stage);
        assert!(component_rendered_during_voice.docked);

        let off =
            super::surface_voice_layout(crate::voice::realtime::RealtimeStage::Off, Some(2), 3);
        assert!(!off.center_stage);
        assert!(!off.docked);
    }

    #[test]
    fn realtime_layout_distinguishes_captured_zero_baseline() {
        // Regression contract: a voice session may start before any component
        // cards exist. `None` is "baseline not captured yet"; `Some(0)` is a
        // captured baseline of zero cards. When the first card appears after
        // voice is live, the layout must dock instead of treating zero as an
        // uncaptured sentinel and recapturing the baseline as one.
        let no_components_at_voice_start = super::surface_voice_layout(
            crate::voice::realtime::RealtimeStage::Connecting,
            Some(0),
            0,
        );
        assert!(no_components_at_voice_start.center_stage);
        assert!(!no_components_at_voice_start.docked);

        let first_component_after_voice_start =
            super::surface_voice_layout(crate::voice::realtime::RealtimeStage::Live, Some(0), 1);
        assert!(!first_component_after_voice_start.center_stage);
        assert!(first_component_after_voice_start.docked);
    }
    #[test]
    fn deep_link_selects_session() {
        assert_eq!(
            parse_deep_link("ocean://session/abc-123"),
            Some(DeepLinkAction::SelectSession("abc-123".into()))
        );
        // UUID-shaped ids (no '/') pass through unchanged.
        assert_eq!(
            parse_deep_link("ocean://session/11111111-2222-4333-8444-555555555555"),
            Some(DeepLinkAction::SelectSession(
                "11111111-2222-4333-8444-555555555555".into()
            ))
        );
    }

    #[test]
    fn deep_link_strips_query_and_fragment() {
        assert_eq!(
            parse_deep_link("ocean://session/abc?ref=tray"),
            Some(DeepLinkAction::SelectSession("abc".into()))
        );
        assert_eq!(
            parse_deep_link("ocean://session/abc#frag"),
            Some(DeepLinkAction::SelectSession("abc".into()))
        );
    }

    #[test]
    fn deep_link_rejects_unknown_or_malformed() {
        // Wrong scheme / shape.
        assert_eq!(parse_deep_link("https://session/abc"), None);
        assert_eq!(parse_deep_link("ocean:session/abc"), None);
        // Wrong host.
        assert_eq!(parse_deep_link("ocean://sessions/abc"), None);
        // Missing or empty id.
        assert_eq!(parse_deep_link("ocean://session"), None);
        assert_eq!(parse_deep_link("ocean://session/"), None);
        // Multi-segment path — a session id is atomic.
        assert_eq!(parse_deep_link("ocean://session/a/b"), None);
        // Empty input.
        assert_eq!(parse_deep_link(""), None);
    }
}
