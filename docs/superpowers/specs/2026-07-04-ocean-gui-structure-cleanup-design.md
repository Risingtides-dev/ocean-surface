# ocean-gui structure cleanup design

Date: 2026-07-04
Status: ready for user review
Scope: `crates/ocean-gui` native GPUI app structure only

## Purpose

`ocean-gui` should become one coherent native client for the Ocean agent system. Today the crate ships competing app identities: the real GPUI app, a parallel egui prototype, stale tldraw-era canvas vocabulary, and an 11k-line shell object that owns nearly every surface concern. That structure makes the app hard to understand, hard to change, and easy for future work to target the wrong code path.

The cleanup goal is not visual polish or a rewrite. The goal is to make the product model and code structure say the same thing:

```text
Ocean native app = Agent-first desktop client + session canvas + supporting vault/editor
```

## Decisions

### 1. `ocean-gui` is the only canonical native app

The GPUI binary `ocean-gui` is the native desktop surface. The eframe/egui prototype must stop shipping as a peer product. It can be deleted if no active reference value remains, or moved to an explicitly named legacy/reference location that cannot be launched as the app by accident.

Acceptance criteria:

- The documented launch path remains `cargo run -p ocean-gui --bin ocean-gui`.
- No second binary presents itself as an equivalent Ocean desktop app.
- Any retained egui source is labeled reference-only and cannot be confused with the GPUI surface.

### 2. The app becomes Agent-first

The primary product spine is the Agent thread. A user opens the app to attach to or create an Ocean session, send prompts, inspect turns, and see tool/file/diff activity. The canvas and vault are workspace surfaces attached to that session/project, not separate products competing for the first-launch mental model.

Target surface roles:

- **Agent thread:** session creation/selection, transcript, composer, model/project controls, tool activity, stop/retry/error states.
- **Canvas workspace:** session canvas backed by native `CanvasLedger`; multiplayer co-editing and cards live here.
- **Vault/editor:** local notes and editor context; useful as workspace support, not the app's primary identity.
- **LiveKit presence:** collaboration state for people in the room; visible where it serves the session/canvas.

Acceptance criteria:

- The first meaningful app state orients around session/agent activity, not an empty canvas with no explanation.
- Empty canvas/vault states explain their relationship to the active session.
- Surface navigation does not imply three unrelated products.

### 3. `OceanGuiShell` becomes a coordinator

`OceanGuiShell` should coordinate app-level state and route events. It should not directly own every renderer, daemon transition, canvas operation, editor behavior, LiveKit panel, and command palette detail inline.

Target boundary:

```text
OceanGuiShell
  -> chrome/navigation/status
  -> agent thread
  -> canvas workspace
  -> vault/editor
  -> LiveKit panel
  -> command palette
```

Each slice should expose a small, boring interface:

```text
State       what the slice owns
Event       what can happen to it
Command     what it asks the shell/runtime to do
ViewModel   what render code needs
render      how it draws itself
```

The split should be incremental. Do not invent a framework. Do not migrate everything into trait objects. Prefer plain Rust modules, structs, functions, and narrow ownership.

Acceptance criteria:

- `view.rs` trends toward shell composition and event routing, not full product implementation.
- Extracted modules can be understood without reading all of `view.rs`.
- Extracted modules keep existing behavior green after each phase.

### 4. Native `CanvasLedger` is the canvas

The native canvas is now `CanvasLedger` plus GPUI rendering and LiveKit data-channel sync. Stale tldraw-era names should be retired where they imply tldraw still owns canvas state.

Target rule:

- `CanvasLedger` owns native components, edges, selection, viewport, patch log, merge state, prompt context, local persistence, and co-editing merge behavior.
- tldraw is optional source material only: import/export, embedded sketching, or reference code if explicitly retained.
- Dead adapter paths should be deleted rather than kept as confusing future hooks.

Acceptance criteria:

- New canvas work targets `shell/canvas/*` native ledger/render modules.
- `tldraw_adapter` and related names are either removed or clearly scoped to explicit import/export/reference use.
- Footer/debug copy no longer reports stale tldraw/legacy ledger concepts as product UI.

## Migration phases

### Phase 1 — Remove ambiguity

Remove the parallel-shell confusion before touching deeper architecture.

Work:

- Inspect `crates/ocean-gui/Cargo.toml` binary definitions.
- Delete or quarantine the egui prototype (`ocean-gui-egui`, `src/main.rs`, `src/app.rs`) depending on whether any unique reference code still matters.
- Update launch docs/comments that imply multiple native app targets.
- Keep local canvas co-editing work untouched except for import/name fallout.

Why first: this prevents agents and humans from debugging or polishing the wrong desktop app.

### Phase 2 — Rename reality around the native canvas

Finish the tldraw demotion.

Work:

- Replace stale public/internal labels that imply tldraw owns the active canvas.
- Remove dead adapter code that is not called by the GPUI native path.
- Keep explicit tldraw import/export/reference code only if it has a named owner and callsite.
- Update status/footer copy so user-facing debug labels match native `CanvasLedger` reality.

Why second: it removes the dual-canvas mental model before module extraction spreads names into new files.

### Phase 3 — Extract product slices from `view.rs`

Split by product seam, not by random helper type.

Recommended first extractions:

1. `shell/chrome.rs` or `shell/chrome/mod.rs` — app tabs, top bar, status bar, navigation labels.
2. `shell/agent_thread.rs` — transcript render, composer render, turn/tool rows, agent session UI state helpers.
3. `shell/canvas_workspace.rs` — canvas panel composition around existing native canvas render modules.
4. `shell/vault_editor.rs` — vault tree, editor pane, document state, backlinks/index presentation.
5. `shell/livekit_panel.rs` — rooms, participants, mic/camera/presence controls.
6. `shell/command_palette.rs` — command palette state/render/action dispatch if currently embedded in `view.rs`.

Rules:

- Extract one slice at a time.
- Keep behavior identical unless the phase explicitly changes stale naming or dead paths.
- Avoid new abstractions until two slices need the same contract.
- Prefer moving coherent blocks with small adapter structs over rewriting logic.

Why third: once identities are clear, extraction can follow product boundaries instead of preserving historical accidents.

### Phase 4 — Make Agent-first launch and empty states explicit

After structure is sane, adjust launch/default state so the app explains itself.

Work:

- Default empty state should point to session/agent start or attach flow.
- Canvas empty state should say it is a workspace for the active session.
- Vault empty state should say it is local context/editor support.
- Navigation labels should reflect roles: Agent, Canvas/Surface, Vault/Notes, Rooms/LiveKit if exposed.

Why last in this structure pass: visible UX changes should land after the code has the boundaries to support them cleanly.

## Data and event flow

The surface remains a surface. Runtime authority stays in `../ocean-os`.

Rules:

- `ocean-gui` creates/selects sessions, posts turns, subscribes to session-scoped events, and renders state.
- Provider calls, agent reasoning, permissions, tools, and session authority stay daemon-owned.
- LiveKit data channels remain a collaboration courier, not a ledger authority.
- Native canvas state remains ledger-backed and locally persistent.

Target flow:

```text
User intent
  -> OceanGuiShell command routing
  -> daemon/session API or local slice event
  -> slice state update
  -> view model
  -> GPUI render
```

For canvas co-editing:

```text
local canvas patch
  -> CanvasLedger merge/local persist
  -> eligible LiveKit data packet
  -> remote CanvasLedger merge
```

Room-received canvas patches must not re-broadcast.

## Error handling and safety

This cleanup should reduce silent failure modes.

Rules:

- Replace launch-critical `expect`/panic paths touched by the migration with visible status-returning errors.
- Preserve existing behavior for unrelated paths.
- Do not swallow daemon/session failures into empty UI.
- Add user-visible error state when a surface has no session, no canvas ledger, or no daemon connection.
- Keep compile-time feature boundaries clear: default build and `--features livekit` must both stay green.

Known risks to avoid:

- Rewriting shell state before deleting/quarantining the egui target.
- Renaming canvas symbols without migrating every callsite.
- Extracting render code into modules that still borrow all of `OceanGuiShell` mutably.
- Turning module extraction into a new UI framework.

## Verification

Run focused checks after each phase:

```sh
cargo check -p ocean-gui
cargo test -p ocean-gui
cargo check -p ocean-gui --features livekit
cargo clippy -p ocean-gui -- -D warnings
```

For phases that touch launch/default state, also launch the app:

```sh
cargo run -p ocean-gui --bin ocean-gui
```

Manual smoke target:

- App opens the GPUI shell.
- Agent/session area is understandable without knowing internal tabs.
- Canvas surface still renders native ledger state.
- LiveKit feature build still compiles.
- No egui prototype appears as a competing app target.

## Non-goals

This design does not include:

- Full brand-kit migration.
- Visual redesign beyond labels and empty states needed for structural clarity.
- New agent features.
- Daemon API changes.
- Provider calls in the surface repo.
- Turning the daemon into a canvas relay.
- A new shell framework.
- A full rewrite of `view.rs` in one step.

## Success criteria

The structure cleanup succeeds when:

1. There is one obvious native app target: GPUI `ocean-gui`.
2. The product model is obvious in code and UI: Agent-first, canvas/vault as workspace support.
3. Native `CanvasLedger` is clearly the production canvas path.
4. tldraw/egui leftovers are deleted or explicitly marked reference-only.
5. `view.rs` no longer needs to be the only place to understand every feature.
6. Default and LiveKit builds stay green throughout.
7. Future brand/UX work can target stable shell seams instead of a monolith.
