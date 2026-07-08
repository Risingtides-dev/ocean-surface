# Ocean Desktop — North Star Design

> The Tauri desktop app's product design: information architecture, deep-menu
> system, facet functionality, and build orchestration. Grounded in the
> 2026-07 competitive research pass (Claude Desktop, Codex/ChatGPT desktop,
> Cursor 3.x) and the existing Ocean specs
> (`../ocean-os/docs/OCEAN_BROWSER_CONTROL_SURFACE.md`,
> `../ocean-os/docs/AGENT_RENDER_PROTOCOL.md`).
>
> Contract recap: ONE canonical Leptos WASM bundle, two hosts (browser PWA,
> Tauri shell). The daemon is the authority; every surface steers. Native
> affordances are host-gated behind `running_as_tauri()` and degrade cleanly
> in the browser. Design language: OCEAN depth ramp, conditional rendering
> over permanent chrome, reveal-on-intent (`docs/OCEAN_WEB_SURFACE_DESIGN.md`).

## Strategic position (from the comp research)

- **Codex Remote proved our thesis**: files/creds/permissions stay on the
  host; satellites steer over a paired relay. Codex is one-phone-one-host;
  Ocean is many-surfaces-one-daemon. We extend a validated pattern.
- **Claude Desktop's weakness is fragmentation**: Claude Code desktop sessions
  are isolated from web/mobile; Cowork projects don't sync. Ocean's
  daemon-owned session contract fixes this *by construction*. Never break it.
- **Cursor sets the GitHub bar**: agents-as-sessions listed in one window
  regardless of origin, branch-per-agent, PR results, Bugbot auto-review,
  event automations. We match the *orchestration UX*, not the cloud-VM
  execution model.
- **Nobody has an agent-native browser.** Codex fled to cloud VMs, Claude is a
  screenshot loop, Cursor only drives localhost. Layer 6 of the browser spec
  (tab-as-task, omnibox-as-intent, record/replay, audit) is the long-game
  differentiator. The cockpit (below) is the first step that requires zero
  daemon changes.

## Information architecture

Three vertical zones plus a command layer. Idle state stays a single bar +
transcript — every zone below is reveal-on-intent.

```text
┌──────────────────────────────────────────────────────────────┐
│ header: project/session context · daemon dot · ⋯ overflow    │
├──────────┬──────────────────────────────────┬────────────────┤
│ RAIL     │ TRANSCRIPT                       │ CONTEXT DECK   │
│ projects │ turns · streaming · render-      │ Files          │
│ sessions │ protocol components · permission │ Repo           │
│ agents   │ prompts · composer               │ Browser        │
│          │                                  │ (Council)      │
└──────────┴──────────────────────────────────┴────────────────┘
                 ⌘K command palette (the deep menu)
```

### The Rail (left, collapsible, project-first)

Extends the existing sessions panel into the Cursor "Agents Window" pattern
mapped onto Ocean's real hierarchy:

- **Projects** (bound to repos when a binding exists) → sessions grouped
  under them; `Other` bucket per the existing session contract.
- Session rows: status glyph (running / idle / needs-approval), origin badge
  (`surface-web` / `surface-tauri` / `surface-extension` / TUI), branch chip
  when the project has a repo binding.
- No new chrome when idle: the rail is the existing sessions panel, richer.

### The Context Deck (right, one panel visible at a time)

Deck tabs are ghost triggers; nothing renders until opened. Each panel is a
self-contained Leptos module with a single mount point (see Orchestration).

1. **Files** — persistent explorer for the session cwd. Native root pick
   (Tauri `pick_folder`), live updates (`watch_paths` → `path-changed`),
   collapsible tree reusing `file_tree` styling, click → preview (read via
   daemon tools, permission-gated). Browser host: read-only tree from the
   agent-emitted `file_tree` components; picker and watcher hidden.
2. **Repo** — local-first repo state: current branch, dirty/staged counts,
   ahead/behind, recent commits. Sourced natively (git2 in the Tauri backend,
   watching `.git/HEAD` + refs via the existing watcher) — this is *local
   file reading*, not a provider call, so it belongs in the shell. GH-API
   depth (PRs, CI, reviews) lands later behind daemon tools; the panel's
   schema reserves space for it. Actions (commit/push/PR) stay behind
   reveal-on-intent and route through the daemon as agent turns.
3. **Browser** — the agent-browser cockpit, v1 built *entirely from the
   existing event stream*: a reducer over `browser_*` tool calls in the
   session's turns (navigate/click/type actions → action timeline;
   `browser_read_page` results → current PageRead card with url/title/elements;
   `browser_screenshot` results → latest screenshot). Zero daemon changes.
   v2 (later, ocean-os work): `ClientContext.browser` + dedicated events per
   the control-plane doc; v3 is the embedded-Chromium decision (Phase B:
   WRY vs CEF spike — deliberately deferred; the cockpit teaches us what the
   embed must do).
4. **Council** — longhouse surface; owned by the concurrent session's work.
   The deck reserves the slot; we do not touch `council_events` code.

### The Command Layer (the "deep menus")

Deep functionality without chrome sprawl — three entry points, one registry:

- **⌘K command palette**: fuzzy command list, the primary deep menu. Commands
  are a typed registry (id, title, scope, availability predicate, action).
  Scopes: session (new/switch/approve), files (pick root, reveal), repo
  (copy branch, view diff), browser (open cockpit, screenshot), app (toggle
  deck, daemon status). Availability predicates gate on host + panel state —
  browser-host shows only what works there.
- **Native menubar** (Tauri Menu API, macOS-first): File / Session / View /
  Help mirroring palette commands. Menubar is Tauri-only by definition.
- **Header ⋯ overflow**: unchanged — secondary actions per the design system.

One registry drives all three; a command added once appears everywhere.

## Host bridge (the enabling slice)

A single `src/host.rs` module in `ocean-surface-ui` owns ALL Tauri interop:

- `invoke(cmd, args) -> Future<Result<JsValue>>` via `__TAURI_INTERNALS__`,
  compiled for wasm, runtime-gated on `running_as_tauri()`.
- `listen(event, cb)` for `path-changed` (and future shell events).
- Typed wrappers: `pick_folder()`, `watch_paths(paths)`, `unwatch_paths`,
  `repo_state(root)`.
- Every wrapper returns `None`/no-op on non-Tauri hosts — panels never call
  `__TAURI_INTERNALS__` directly. This is the only file that knows Tauri
  exists.

Backend additions in `crates/ocean-tauri`: `repo_state` command (git2:
branch, ahead/behind vs upstream, dirty/staged counts, last N commits) and a
`menu` setup. `pick_folder`/`watch_paths` already exist.

## Surface Capability Matrix

The information architecture says *what* each piece is; this matrix says
*where it runs*. The split is structural, not a porting decision: the daemon
owns all state, so anything daemon-backed crosses over to every surface for
free, and the desktop bundle contributes only native hands. One Leptos bundle,
one gate — `host::running_in_tauri()` — and native affordances degrade to
`None`/`false`/no-op on the browser and extension. That gate *is* the
capability axis; there is no second codebase to keep aligned.

The comps leak at exactly this seam. Claude Desktop's Cowork sessions are
stranded off web/mobile; Codex keeps IDE and web history unsynced; Cursor had
to bolt on an Agents Window to reconcile cloud sessions with local ones. Each
failure is a capability that didn't cross surfaces. Codex Remote inverts
Ocean's topology yet confirms it — files, creds, and permissions stay on the
host while the phone steers over a paired relay, i.e. daemon-authority with a
thinner satellite (fuller argument in "Strategic position" above).

### Crosses over (daemon-backed → every surface)

These port for free because they read state any client can already see:

- Transcript, sessions, projects — the session contract itself.
- ⌘K command palette — registry + scoring, host-agnostic.
- Permission prompts and voice — daemon-mediated, surfaced identically
  everywhere.
- Render-protocol components and the Council deck — turn and `council_events`
  data.
- Browser cockpit — a reducer over `browser_*` tool events; the browser lives
  in the daemon via CDP, so any surface watches the same action /
  `browser_read_page` / screenshot stream.
- Files panel listing — daemon `/v1/fs/dirs`, readable from any host.

### Desktop-only (native hands)

These exist only because they need the OS; each is one gated wrapper, never a
feature fork:

- **Daemon supervision** — the desktop app is the daemon's home: sidecar
  spawn, health, restart. Web connects; desktop runs.
- **OS presence** — menubar/tray, a global hotkey that summons the app, a dock
  badge for pending permissions, native notifications on turn-complete and
  permission-request, launch-at-login, keep-awake.
- **Native menus** projected from the single `CommandRegistry` — palette and
  menubar share one command source (see Command Layer).
- **Local FS hands** — `pick_folder`, `watch_paths`, and the git2 `repo_state`.
  Shipped; local file reading that belongs in the shell.
- **Deep links, Keychain creds, QR pairing host** — `ocean://session/<id>`
  entry, OS credential storage, and the desktop acting as the relay host
  web/phone pairs against.
- **Multi-window** — session pop-outs and an always-on-top companion.

### Web/PWA distinctive edge

The web surface is not a degraded desktop — reachability is its native
advantage: zero-install, open from anywhere, PWA install, web push. It is the
remote control that sees the *same* session state the desktop does, which beats
every comp's remote: they sync a copy, Ocean reads the original.

### Flagged decision — repo panel

Repo state is native-git2 today, so the browser shows a native-shell-only empty
state. Wave 2 adds a daemon-side repo-state tool as the web fallback and the
shared depth source (PRs / CI / reviews) for both surfaces; the panel schema
already reserves the space. This is the one panel deliberately not yet
crossover-complete.

## Orchestration & worktree protocol

The main checkout is shared with concurrent council/longhouse work
(uncommitted edits to `daemon.rs`, `sessions.rs`, `proxy/main.rs`,
`panels.css`). Build isolation rules:

1. **Every workstream builds in its own git worktree** off `main`, branch
   `feat/desktop-<stream>`. Nobody edits the shared checkout directly.
2. **Collision files are integrator-owned.** `app.rs` (mounts), `daemon.rs`,
   `index.html` / `extension/sidepanel.html` / `scripts/build-extension.sh`
   (the stylesheet 3-place rule) are touched only by the integrator (Main) at
   merge time. Subagents deliver self-contained modules + a documented mount
   hook, never the mount edit itself.
3. **One new stylesheet** `styles/deck.css` covers all deck panels + palette
   (one 3-place wiring edit, done once by the integrator). Colors only from
   `styles/tokens.css`.
4. **Merges are serialized** through the integrator: review in worktree →
   rebase on main → gates (`cargo check -p ocean-surface-ui --target
   wasm32-unknown-unknown`, `cargo test -p ocean-surface-ui`, tauri crate
   check from its own dir) → land → next stream rebases.
5. **Never touch** the concurrent session's files beyond mechanical rebase;
   their work lands on their schedule.

### Workstreams

| Stream | Branch | Scope (files) | Depends on |
|---|---|---|---|
| HostBridge | `feat/desktop-host-bridge` | NEW `ocean-surface-ui/src/host.rs`; `ocean-tauri/src/lib.rs` (+git2 `repo_state`, menu scaffold) | — |
| FilesPanel | `feat/desktop-files-panel` | NEW `src/deck/files.rs` (+`deck/mod.rs`) | HostBridge contract |
| RepoPanel | `feat/desktop-repo-panel` | NEW `src/deck/repo.rs` | HostBridge contract |
| Cockpit | `feat/desktop-cockpit` | NEW `src/deck/browser.rs` (event reducer) | — (reads existing turn stream) |
| Palette | `feat/desktop-palette` | NEW `src/palette.rs` (registry + ⌘K UI) | — (commands wired at integration) |
| Integration | (main, integrator only) | `app.rs` mounts, `deck.css` wiring, menubar wiring, rail grouping polish | all above |

Panels code against the HostBridge *contract* (function signatures above),
not its implementation — streams run in parallel from the start.

## Sequencing

1. **Wave 1 (parallel)**: HostBridge, Cockpit, Palette, FilesPanel, RepoPanel.
2. **Integration pass** (integrator): mounts, deck.css 3-place wiring, palette
   command registry filled with real actions, menubar → palette registry.
3. **Wave 2** (after user look): rail regrouping by repo/branch/origin
   (touches `sessions.rs` — deliberately deferred until the concurrent
   session's `sessions.rs` work lands), GH-API depth via daemon tools,
   streaming-diff review affordance, checkpoint/rollback.
4. **Wave 3**: embedded-browser spike (WRY vs CEF), `ClientContext.browser`
   daemon events, Layer-6 primitives.

### Native-feel priorities

The OS-presence slice, sequenced by feel-impact: **P1** tray + menubar +
global hotkey · **P2** native notifications + dock badge · **P3** native menus
from the registry · **P4** daemon-supervision sidecar · **P5** deep links /
Keychain / QR pairing · **P6** multi-window.
