# Ocean Surface Shell Architecture Note

Status: active architecture note for the product surface split.

This note records the intended relationship between the shared Leptos surface,
the Tauri desktop shell, the Chrome extension, and agent-driven browser control.
It is not a phased roadmap. It is the architectural stance the repo should hold
unless a later note deliberately replaces it.

Read with:

- `docs/OCEAN_PLATFORM_CONTRACT.md`
- `docs/OCEAN_DESKTOP_NORTH_STAR.md`
- `docs/ocean-extension-context.md`
- root `AGENTS.md`

## Decision summary

Ocean Surface has **one product UI** and several shells around it.

- `crates/ocean-surface-ui` is the canonical product surface.
- `crates/ocean-tauri` is the **primary native desktop shell**.
- `extension/` is a **browser-context companion shell**, not a second product.
- Daemon-controlled Chrome is the **primary browser automation substrate**.
- Any embedded browser pane inside the desktop app is **optional and subordinate**,
  not the main browser strategy.

In short:

> One Leptos product core. Tauri is the desktop home. The extension is the real-browser companion. The daemon's browser tools are the automation engine.

## The core principle

The repo should not split back into separate product implementations for web,
desktop, and browser extension.

`crates/ocean-surface-ui` owns:

- transcript and composer behavior
- session and project UX
- model and turn presentation
- council/component rendering
- voice UI
- canvas and workspace presentation
- any cross-surface interaction model that does not require host-native APIs

Shells own only bootstrapping and host capability.

That means:

- no separate desktop feature logic just because the host is native
- no separate extension product flow just because the host is Chrome
- no provider/session/tool authority in shells
- no agent-state fork inside a shell

The daemon remains the runtime authority. Shells render state, collect intent,
and expose host-specific hands through explicit seams.

## Shell roles

### Tauri desktop shell

Tauri is the main desktop product host.

Its job is to make Ocean feel like a real local application with machine-native
capabilities, including things such as:

- workspace and folder picking
- file watching and local project awareness
- native notifications
- daemon supervision and health display
- menus, dock/tray presence, deep links, and other OS affordances
- multi-window or always-on-top helpers if those are added later

The Tauri shell should be treated as the operator's durable Ocean cockpit.
It is where local-machine presence belongs.

What it is not:

- not a separate UI product
- not a replacement for the browser extension's page awareness
- not the primary browser automation engine

### Chrome extension shell

The extension is a thin shell around the same Surface app, specialized for
real-browser context.

Its job is to attach Ocean to the user's actual browser session and page state.
That includes things such as:

- active tab title and URL
- selected text
- page-local context
- side-panel chat attached to browsing work
- browser-native workflows where being inside Chrome matters

The extension should stay strategically narrow.

It is valuable precisely because it runs in the user's real browser context.
That is something the Tauri shell does not have by default.

What it is not:

- not the main desktop shell
- not a second full product surface with divergent UX
- not the place to carry core product complexity when the shared Leptos core can own it

### Web and PWA shell

The browser/PWA host remains the zero-install, anywhere-access surface.
It proves the shared core works without native affordances and remains the base
for compact/mobile use.

Its value is reach and portability, not native control.

### Embedded desktop browser pane

An in-app browser or workspace browser pane inside the Tauri shell may still be
useful, but it should be understood correctly.

It is an **Ocean-owned browser surface**, not the user's Chrome.

That means it may be good for:

- previews
- docs
- dashboards
- login flows the operator intentionally performs inside Ocean
- controlled, in-app browsing experiences

But it does not automatically inherit:

- the user's current Chrome tab
- the user's Chrome extensions
- the user's Chrome session state
- Chrome-specific automation semantics

It is therefore optional infrastructure, not a foundation the rest of the
product should depend on.

## Browser control architecture

Ocean has three distinct browser-related roles and should keep them separate.

### 1. Real-browser context

Owned by the Chrome extension.

This is about knowing what page the operator is on and attaching turns to that
context.

Questions answered here include:

- what tab is active
- what page is open
- what text is selected
- what page context should be sent with the turn

### 2. Reliable browser automation

Owned primarily by daemon-controlled Chrome via browser tools.

This is about agent actions such as:

- navigate
- read page
- click
- type
- evaluate JavaScript
- screenshot
- inspect console and network

This should remain the main automation path because it is explicit, tool-shaped,
and already aligned with the daemon's permissioned execution model.

### 3. Embedded in-app browsing

Owned by the Tauri shell only if a browser pane is intentionally exposed.

This is about an Ocean-controlled workspace pane, not about replacing either the
extension or daemon Chrome.

If implemented, it should present itself honestly as an in-app browsing surface.
It can be agent-steerable, but only through an explicit shell bridge.

## The capability seam

`src/host.rs` remains the only capability seam through which shell-specific
behavior enters the shared UI core.

That rule matters because it prevents product logic from leaking into host code.

Examples of shell capability that belong behind the seam:

- folder pickers
- path watching
- repo state
- notifications
- daemon lifecycle controls
- desktop menu commands
- optional embedded browser commands if ever added

Examples of capability that do **not** belong as shell-owned product logic:

- transcript behavior
- turn lifecycle rendering
- session semantics
- model picker semantics
- component rendering contracts
- council or room behavior

If a capability is absent on a host, the shared UI should degrade by absence,
not by broken controls or host-specific forks.

## What this means for desktop versus extension

The desktop app and the extension should feel related because they are the same
product core, but they should not pretend to have the same powers.

Desktop is best at:

- machine and workspace presence
- native operating-system integration
- durable multi-session cockpit behavior
- acting as the local home for Ocean

Extension is best at:

- current-tab and page awareness
- browser-side workflows
- page-attached agent help
- context gathered from the real browser session

The right design stance is complementary, not competitive.

Do not force the extension to become the main app.
Do not force desktop to impersonate the real browser.
Let each shell contribute what only that shell can contribute.

## Guidance for the workspace browser

The existing workspace-browser direction should be treated as an optional deck or
panel capability inside the desktop app.

That surface may be useful, but it should follow these rules:

- it is not assumed to mirror Chrome state
- it is not the default source of web context for all turns
- it does not replace daemon browser tools
- it does not justify pushing shared product behavior into Tauri-only code

If the agent is allowed to steer it later, the shell should expose a narrow,
explicit contract such as navigation, history, read-page, evaluation, and
capture primitives. The agent should not gain magical or implicit control of a
webview.

## Non-goals

This architecture explicitly avoids the following mistakes:

- reviving a separate heavy desktop UI stack
- rebuilding product features twice for desktop and extension
- putting agent/runtime authority into the shell layer
- treating the embedded desktop webview as equivalent to the user's browser
- making the extension carry product complexity that belongs in the shared core
- replacing daemon browser tools with ad hoc webview steering

## Practical test

When a new feature is proposed, the first questions should be:

1. Is this shared product behavior or shell capability?
2. Does it belong in the Leptos core or behind `host.rs`?
3. Is this about the user's real browser context, the machine, or automation?
4. Are we accidentally creating a second product implementation?

If those answers are clear, the file placement and ownership boundary should
usually become obvious.

## Architectural stance

The recommended long-term shape is stable:

- shared Leptos core as the product
- Tauri as the main desktop shell
- Chrome extension as the real-browser companion
- daemon browser tools as the automation engine
- optional embedded browser pane as a subordinate desktop capability

That keeps Ocean coherent across surfaces without collapsing genuinely different
host capabilities into a fake uniformity.
