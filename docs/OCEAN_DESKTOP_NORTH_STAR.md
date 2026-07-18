# Ocean Desktop — North Star Design

> The Tauri desktop app's product design: information architecture, native-shell
> posture, workspace evolution, and execution sequencing. Grounded in the
> 2026-07 competitive research pass across Cursor, ChatGPT desktop + Codex,
> Claude Code / Claude Desktop, and Factory, plus the existing Ocean specs
> (`../ocean-os/docs/OCEAN_BROWSER_CONTROL_SURFACE.md`,
> `../ocean-os/docs/AGENT_RENDER_PROTOCOL.md`).
>
> Contract recap: ONE canonical Leptos WASM bundle, two hosts (browser PWA,
> Tauri shell). The daemon is the authority; every surface steers. Native
> affordances are host-gated behind `running_as_tauri()` and degrade cleanly in
> the browser. Design language: OCEAN depth ramp, conditional rendering over
> permanent chrome, reveal-on-intent (`docs/OCEAN_WEB_SURFACE_DESIGN.md`).
>
> Implementation detail for the first desktop slice lives in
> [`OCEAN_DYNAMIC_ISLAND_IMPLEMENTATION.md`](OCEAN_DYNAMIC_ISLAND_IMPLEMENTATION.md).

## North Star

**Ocean Desktop is the native cockpit for persistent agent work.**

Not a wrapped web chat app. Not merely an IDE plugin. Not a second product that
forks away from web/mobile.

On desktop, Ocean should feel like a serious native shell for:

- projects
- sessions
- agents
- files and editing
- browser/computer-use work
- repo + GitHub workflows
- approvals, artifacts, and long-running automation

The differentiator is structural:

- **shared Leptos product core**
- **Tauri-native machine hands**
- **daemon-owned runtime authority**
- **one session model across desktop, web, extension, and future mobile**

## Strategic position (from the comp research)

- **Cursor sets the orchestration bar**: one window for agents, coding,
  reviews, automation, and repo-aware work. Ocean should match the
  *project/session/workflow coherence*, not copy the cloud execution model.
- **ChatGPT desktop + Codex validate the command-center thesis**: desktop AI is
  moving beyond chat into delegated work, parallel agents, worktrees, and rich
  ambient context (files, screenshots, screen state).
- **Claude Code proves the appetite for agent management surfaces**: Agent view,
  routines, dynamic workflows, and computer use all point toward persistent,
  multi-session work management. Claude still fragments state across surfaces;
  Ocean must not.
- **Factory proves the SDLC-cockpit opportunity**: there is real demand above
  the editor layer for triage, validation, review, release, and operational
  visibility in one system.
- **Ocean's advantage is many-surfaces-one-daemon**: where competitors sync or
  duplicate work across desktop/web/mobile, Ocean can let every surface attach
  to the same live session/runtime authority.
- **Native value is context + action, not packaging**: the desktop shell wins
  when it can see more of the machine, act through OS affordances, and keep
  that capability behind one explicit host seam.

## Product thesis shift

A plain left rail is useful, but it is no longer enough to differentiate Ocean.
If desktop begins and ends with "sidebar + center chat + right tools," Ocean
risks becoming another competent agent shell that still feels formulaic.

The more distinctive desktop-native move is:

> **Ocean uses a Dynamic Island as the live surface for agent work. `⌘P`
> turns it into a dedicated session switcher; `⌘⇧F` turns it into transcript
> Recall. Those intents replace one another rather than stacking into one
> dashboard.**

That means:

- **Island Agent mode = living work object and immediate interaction**
- **Island Sessions mode = focused session switching only**
- **Island Recall mode = transcript-content retrieval only**
- **Center = focused transcript**
- **Workbench = tools and authoring**
- **Rail / palette / modal = utility browse surfaces, not the signature move**

## Information architecture

On Tauri desktop, Ocean should resolve into three primary zones plus two command
layers:

```text
┌────────────────────────────────────────────────────────────────────┐
│ header/titlebar: brand · Dynamic Island · daemon/runtime · ⋯      │
├───────────────────────────────────────┬────────────────────────────┤
│ FOCUSED TRANSCRIPT / ACTIVE WORK      │ RIGHT WORKBENCH            │
│ turns · streaming · permissions       │ Files · Editor             │
│ artifacts · composer                  │ Browser · Repo · GitHub    │
└───────────────────────────────────────┴────────────────────────────┘
   click = agent   ·   ⌘P = sessions   ·   ⌘⇧F = Recall   ·   ⌘K = actions
```

A browse surface (rail, modal, or richer palette results) still exists, but it
is not the hero. The hero is the titlebar-adjacent Island that represents work
in motion.

### The Dynamic Island (desktop-native, ambient, alive)

The Island is a compact, persistent surface in the header/titlebar zone.

Its job is to show **active work that may need attention**:

- active sessions
- sessions waiting on a user reply
- permission / approval requests
- browser-driving state
- call / room state
- CI / GitHub runs later
- longhouse / council work later

The Island is not just a status chip. It is the desktop-native **attention
router**.

#### Island behavior

- Quiet when nothing is urgent.
- Glows / pulses when a session or job needs attention.
- Expands on click into one direct agent-work object, never an Activity feed.
- Exposes the state-relevant action without an accordion disclosure.
- Replaces Agent with Sessions or Recall when those explicit tools are invoked.
- Lets the operator switch the focused session in one move from Sessions.
- Later waves can add background reply only after daemon request/token authority exists.

#### What the compact Island shows first

V1 should keep the model small and legible:

- the currently focused session
- one or more active-background sessions
- a simple state badge (`active`, `needs reply`, `approval`, `running`)
- a compact count for additional active items

### The Focused Center (transcript-native)

Ocean remains transcript-first in the middle:

- user prompts
- assistant turns
- streaming work
- permission prompts
- render-protocol artifacts
- room/call state
- composer and slash commands

This is not an IDE center pane with a chat sidebar bolted on. The conversation
and work log remain central because they are the daemon's primary record of
intent, action, and result.

The key desktop rule is:

> **one session owns the center at a time; the others orbit in the Island.**

### The Right Workbench (desktop-native work surface)

The right side is a real workbench on Tauri and a reveal-on-intent deck on the
web/extension. Same conceptual modules, different host posture.

1. **Files** — persistent explorer for the session cwd. Native root pick
   (`pick_folder`), live updates (`watch_paths` → `path-changed`), collapsible
   tree, click → preview/open.
2. **Editor** — evolves the current preview tabs into a true editing surface:
   buffers, dirty state, save flows, search, diffs, and eventually splits /
   diagnostics.
3. **Browser** — the agent-browser cockpit. v1 can continue to derive from the
   existing browser event stream and screencast path; later waves deepen this.
4. **Repo** — local-first git state from the Tauri shell: branch,
   ahead/behind, dirty/staged, recent commits.
5. **GitHub** — PRs, checks, Actions, reviews, issue context, and merge flows.
   Remote authority belongs to daemon tools; the shell contributes local repo
   facts and native affordances.
6. **Council / Canvas / Artifacts** — specialized surfaces mounted when the
   session's work requires them.

### Browse surfaces (utility, not hero)

Ocean still needs a way to browse older or inactive work. That can be supplied
by one or more utility surfaces:

- existing Sessions modal
- a future slim rail
- the bounded Sessions switcher opened by `⌘P`
- project/session browsing via the palette

The important rule is that **browsing is not the signature interaction**.
The signature interaction is the Island + focus switching.

### Command layers

Ocean should separate **agent interaction**, **session switching**, **history
Recall**, and **actions**.

#### Click — Agent

Clicking the compact object opens the focused agent or the single authoritative
work item that currently needs attention. It never appends a session catalogue.

#### `⌘P` — Sessions

`⌘P` opens the Island as a keyboard-first session switcher. It searches session
metadata and answers one question: **which session should own the center?**

#### `⌘⇧F` — Recall

`⌘⇧F` opens daemon-owned transcript Recall. It searches inside prior user and
assistant turns and returns bounded excerpts with provenance. It never mixes in
agent actions or metadata-only session rows.

#### `⌘K` — commands and actions

`⌘K` remains the action layer:

- new session
- open workbench views
- start daemon
- reveal browser tooling
- trigger workflows
- app-level actions

One host can expose both surfaces without confusing them because they serve
clearly different jobs.

## Discovery model

Discovery grows through separate tools rather than heterogeneous result groups.

### Sessions — local metadata fuzzy search

Search sessions by title, project, cwd, branch, origin, recency, and turn count.
Focused-session and authoritative work state may influence ordering, but the
result remains a session.

### Agent — authoritative work routing

Requests, approvals, browser work, calls/rooms, CI, and GitHub jobs appear as
one selected work object in Agent mode. They are not search results or drawers.

### Recall — transcript and memory retrieval

Initial Recall searches persisted user/assistant display transcript text using
truthful exact, lexical, and fuzzy ranking. Later daemon/Bedrock work may fuse
semantic retrieval over assistant summaries, files mentioned, recurring topics,
unresolved threads, and durable memory. The Surface never performs embeddings or
labels fuzzy results semantic.

This separation upgrades Ocean from "chat with history" to a memoryful native
workspace without turning the Island into a universal list.

## Host bridge (the enabling slice)

A single `src/host.rs` module in `ocean-surface-ui` owns ALL Tauri interop:

- `invoke(cmd, args) -> Future<Result<JsValue>>` via `__TAURI_INTERNALS__`
- `listen(event, cb)` for shell-emitted events
- typed wrappers: `pick_folder()`, `watch_paths(paths)`, `unwatch_paths`,
  `repo_state(root)`, daemon supervision, notifications, badge, deep links

Rules:

- Nothing outside `host.rs` references Tauri APIs directly.
- Every wrapper returns `None` / `false` / no-op on non-Tauri hosts.
- Desktop-only capability is additive; it never forks the product model.

## Surface capability matrix

The split is structural, not a porting decision: the daemon owns all state, so
anything daemon-backed crosses to every surface for free, and the desktop bundle
contributes only native hands.

### Crosses over (daemon-backed → every surface)

These port for free because they read state any client can already see:

- transcript, sessions, projects
- command palette registry + scoring
- permission prompts and voice
- render-protocol components and Council data
- browser activity/cockpit reducers over `browser_*` tool events
- files listing when sourced from daemon APIs
- GitHub/PR/checks data once it lands behind daemon tools
- Island/search results derived from daemon-visible session/activity state

### Desktop-only (native hands)

These exist only because they need the OS; each is one gated wrapper, never a
feature fork:

- daemon supervision
- native notifications, dock/taskbar badge, tray/menubar, global shortcuts
- local FS affordances (`pick_folder`, `watch_paths`)
- local git state (`repo_state`)
- deep links, keychain storage, pairing host, launch-at-login
- multi-window and pop-out utility surfaces

### Web/PWA distinctive edge

The web surface is not a degraded desktop. Its native advantage is reachability:
zero-install, remote control, mobile availability, PWA install, web push, and
shared access to the exact same daemon-owned session state.

### GitHub boundary (binding decision)

Remote repo authority does **not** belong in the Tauri shell.

- **Shell owns**: local repo discovery, branch/worktree facts, reveal/open,
  native file and OS affordances.
- **Daemon owns**: GitHub auth, PR/review/checks/Actions queries, mutations,
  automation, auditability, and permission gating.

This boundary is what lets Ocean gain GitHub depth without corrupting the
shared-core model.

## Current implementation status

The scaffold is already real:

- `src/host.rs` exists and already carries core Tauri affordances.
- `src/workspace.rs` already mounts a desktop right-side workspace pane.
- files / repo / browser workspace surfaces already exist in some form.
- palette/command-registry work exists.
- session grouping logic already exists.
- browser activity and session metadata already provide enough state to drive an
  Island v1 without daemon architecture changes.

The biggest remaining product gap is **desktop information architecture**, not
foundational plumbing.

## Implementation slices (current recommendation)

### Slice 1 — Dynamic Island shell

This is the immediate priority.

Goals:

- mount a compact Island in the Tauri header/titlebar zone
- show the focused session and authoritative agent-work state
- reflect simple live states (`ready`, `needs you`, `running`)
- expand on click into one direct Agent object
- keep the browser/web model intact

Success condition: Ocean stops reading as a web chat in a native window and
starts reading as a native workspace with a living attention surface.

### Slice 1.5 — Focus switching

Goals:

- selecting an Island item promotes that session into the center transcript
- the previously focused session slots back into the Island
- preserve room/call/browser and workbench behavior

Success condition: one session is foreground and the rest orbit naturally.

### Slice 2 — `⌘P` Sessions mode

Goals:

- open the Island as a keyboard-first session switcher
- search session metadata: title, project, path, branch, origin, recency
- bound and rank focus results
- Enter focuses, arrow keys navigate, Esc collapses

Success condition: `⌘P` answers which session should own the center without
mixing in agent work or transcript passages.

### Slice 3 — `⌘⇧F` Recall

Goals:

- search persisted user/assistant transcript content through daemon authority
- return bounded excerpts with session, role, workspace, and match provenance
- begin with truthful exact/lexical/fuzzy ranking
- add semantic fusion through daemon/Bedrock later, never in Surface code

Success condition: Ocean becomes a memoryful workspace without turning the
Island into one heterogeneous result list.

### Slice 4 — Editor-grade workbench

Goals:

- evolve preview tabs into editable buffers
- dirty state + save/save-all
- search/find-in-file
- syntax highlighting and stable tab behavior
- diff/review affordances
- command-palette integration for editor actions

Success condition: the right workbench becomes a place to *author*, not merely
preview.

### Slice 5 — Repo + GitHub cockpit

Goals:

- keep local git state strong on desktop
- add daemon-backed GitHub views: PRs, checks, Actions, reviews, issues
- connect session/worktree/branch/PR as one visible thread of work
- let actions route through daemon tools, not shell-only code
- surface CI / GitHub runs in the Island when relevant

Success condition: Ocean can manage real engineering workflow inside one shell.

### Slice 6 — OS presence and browser/multi-agent expansion

Goals:

- native menubar from the command registry
- notifications + dock badge polish
- global hotkey
- deep-link routing
- launch-at-login / keep-awake / utility pop-outs
- deepen the browser cockpit and multi-agent orchestration surfaces

Success condition: the app feels at home on the OS, not merely hosted by it,
and Ocean's long-game differentiator becomes visible without sacrificing the
current daemon/session model.

## Native-feel priorities

Ordered by operator impact:

1. **Dynamic Island Agent interaction**
2. **`⌘P` Sessions + `⌘⇧F` Recall**
3. **Editor-grade workbench**
4. **Repo + GitHub cockpit**
5. **Notifications / badge / menubar / hotkey**
6. **Deep links / keychain / pairing / multi-window**
7. **Embedded-browser / Layer-6 browser primitives**

## Execution rules

- Keep one canonical Leptos bundle.
- Keep `host.rs` as the only Tauri seam.
- Prefer mounting/refining existing modules over inventing parallel systems.
- Do the desktop shell as additive host posture, not a divergent product.
- Separate Agent interaction, `⌘P` Sessions, `⌘⇧F` Recall, and `⌘K` actions.
- Land the work in slices; **Dynamic Island first**.
