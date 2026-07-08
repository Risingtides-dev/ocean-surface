# Ocean GUI Structure Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `crates/ocean-gui` one coherent GPUI native app by deleting the egui prototype, demoting stale tldraw paths, making Agent-first navigation explicit, and extracting focused shell slices from the 11k-line `view.rs` monolith.

**Architecture:** Keep `OceanGuiShell` as the app coordinator and session/event router. Move render-only/product-slice code into sibling modules under `crates/ocean-gui/src/shell/`, while preserving daemon/session authority in `../ocean-os` and preserving the already-committed native `CanvasLedger` + LiveKit data-channel co-editing path.

**Tech Stack:** Rust 2024, GPUI `0.2.2`, optional LiveKit feature, native `CanvasLedger`, existing `DaemonClient` protocol module, Cargo checks/tests/clippy.

---

## File structure map

### Canonical entrypoint

- Keep: `crates/ocean-gui/src/bin/ocean_gui.rs` — GPUI `Application` entrypoint for `OceanGuiShell`.
- Keep: `crates/ocean-gui/src/lib.rs` — library root exposing `shell` and `workspace`.
- Keep: `crates/ocean-gui/src/workspace.rs` — shared workspace scanning used by GPUI shell (`shell/model.rs`, `shell/vault_index.rs`).
- Modify: `crates/ocean-gui/Cargo.toml` — remove egui bin/dependency and add `default-run = "ocean-gui"`.
- Delete: `crates/ocean-gui/src/main.rs` — egui prototype bin root.
- Delete: `crates/ocean-gui/src/app.rs` — egui prototype app and mock data.

### Native canvas and tldraw demotion

- Keep untouched: `crates/ocean-gui/src/shell/canvas/*` — native canvas authority.
- Keep untouched: `crates/ocean-gui/src/shell/canvas_sync.rs` — LiveKit data-channel co-edit protocol.
- Delete: `crates/ocean-gui/src/shell/tldraw_adapter.rs` — no non-test callsites; currently `#![allow(dead_code)]`.
- Modify: `crates/ocean-gui/src/shell/surface_livekit.rs` — make room metadata source from `CanvasLedgerSet`, not `SurfaceState`/`SurfaceLedger`.
- Modify: `crates/ocean-gui/src/shell/view.rs` — update metadata callsites and remove the presence fallback to legacy `SurfaceState`.
- Keep for now: `crates/ocean-gui/src/shell/surface.rs` and `surface_host.rs` — optional webview/sketch projection remains behind the existing default-off toggle until metadata no longer depends on it and a separate deletion decision can be made safely.

### Extraction targets

- Create: `crates/ocean-gui/src/shell/chrome.rs` — navigation enum, top bar/status render, tooltip.
- Create: `crates/ocean-gui/src/shell/command_palette.rs` — palette state, entries, key text parsing, palette render.
- Create: `crates/ocean-gui/src/shell/vault_editor.rs` — vault/editor render and the custom editor element kept together for this extraction pass.
- Create: `crates/ocean-gui/src/shell/livekit_panel.rs` — LiveKit panel render-only helpers and media buttons.
- Create: `crates/ocean-gui/src/shell/agent_thread.rs` — Agent thread render-only helpers and pure label/summary helpers.
- Modify: `crates/ocean-gui/src/shell/mod.rs` — register new modules.
- Modify: `crates/ocean-gui/src/shell/view.rs` — keep coordination, event routers, task pumps, session filters, canvas sync reducers, and app composition root.

## Cross-cutting rules

- Do not move these event/session reducers in this plan:
  - `apply_agent_event`
  - `apply_control_event`
  - `apply_surface_livekit_client_event`
  - `apply_rooms_message`
  - `drain_canvas_sync_outbound`
  - `request_canvas_snapshot*`
  - `serve_canvas_snapshot`
  - `apply_canvas_snapshot`
  - `apply_canvas_sync_message`
  - `clear_canvas_sync_state`
  - `spawn_*_task` pumps
- Do not touch provider calls, permission policy, tool execution authority, or daemon-owned session semantics.
- Do not turn the daemon into a canvas relay.
- Do not rewrite render logic while extracting it; move behavior first, then test.
- Use `lsp references` before renaming exported or cross-file symbols during execution.
- Commit after each task.

---

### Task 1: Delete the egui prototype and harden the canonical GPUI binary

**Files:**
- Modify: `crates/ocean-gui/Cargo.toml`
- Delete: `crates/ocean-gui/src/main.rs`
- Delete: `crates/ocean-gui/src/app.rs`
- Keep: `crates/ocean-gui/src/bin/ocean_gui.rs`
- Keep: `crates/ocean-gui/src/lib.rs`
- Keep: `crates/ocean-gui/src/workspace.rs`
- Modify: `Cargo.lock` through Cargo only
- Modify: `.github/workflows/ci.yml` comment only

- [ ] **Step 1: Verify the current egui references**

Search for `OceanGuiApp`, `ocean-gui-egui`, and `eframe`.

Expected non-generated references before the edit:

```text
crates/ocean-gui/Cargo.toml
crates/ocean-gui/src/main.rs
crates/ocean-gui/src/app.rs
.github/workflows/ci.yml comment mentioning eframe/wgpu
```

Do not delete `src/workspace.rs`; it is used by the GPUI shell.

- [ ] **Step 2: Edit `Cargo.toml` atomically with the file deletions**

The top of `crates/ocean-gui/Cargo.toml` should become:

```toml
[package]
name = "ocean-gui"
version = "0.1.0"
edition = "2024"
default-run = "ocean-gui"

[[bin]]
name = "ocean-gui"
path = "src/bin/ocean_gui.rs"

[dependencies]
anyhow = "1"
base64 = "0.22"
gpui = "0.2.2"
indexmap = { version = "2", features = ["serde"] }
```

Keep the rest of the existing dependency list after `indexmap`. Remove only:

```toml
eframe = "0.29"
```

Do not leave a state where `[[bin]] ocean-gui-egui` is removed but `src/main.rs` remains; Cargo will auto-discover `src/main.rs` as a duplicate `ocean-gui` binary.

- [ ] **Step 3: Delete the prototype files**

Delete:

```text
crates/ocean-gui/src/main.rs
crates/ocean-gui/src/app.rs
```

Expected result: no `mod app;`, `OceanGuiApp`, `eframe::run_native`, or `eframe::egui` references remain in `crates/ocean-gui/src`.

- [ ] **Step 4: Update the stale CI comment**

In `.github/workflows/ci.yml`, change the ocean-gui dependency comment from mentioning:

```text
gpui + livekit/libwebrtc + eframe/wgpu + rfd/GTK + wry
```

to:

```text
gpui + livekit/libwebrtc + rfd/GTK + wry
```

No workflow behavior changes.

- [ ] **Step 5: Let Cargo update the lockfile**

Run:

```sh
cargo check -p ocean-gui
```

Expected: command succeeds and `Cargo.lock` is updated by Cargo if the egui dependency tree is pruned.

- [ ] **Step 6: Verify all gates for the slice**

Run:

```sh
cargo check -p ocean-gui --features livekit
cargo test -p ocean-gui
cargo clippy -p ocean-gui -- -D warnings
cargo run -p ocean-gui --bin ocean-gui
```

Expected:

- Checks/tests/clippy pass.
- GPUI app launches with the canonical binary.
- Optional proof: `cargo run -p ocean-gui` now resolves to the GPUI binary because only one bin remains and `default-run` is explicit.

- [ ] **Step 7: Commit**

```sh
git add crates/ocean-gui/Cargo.toml Cargo.lock .github/workflows/ci.yml
git rm crates/ocean-gui/src/main.rs crates/ocean-gui/src/app.rs
git commit -m "refactor(ocean-gui): remove egui prototype binary"
```

---

### Task 2: Delete the dead tldraw adapter module

**Files:**
- Delete: `crates/ocean-gui/src/shell/tldraw_adapter.rs`
- Modify: `crates/ocean-gui/src/shell/mod.rs`

- [ ] **Step 1: Re-confirm zero non-test callsites**

Search for `tldraw_adapter`, `ledger_to_tldraw_commands`, `import_shape_to_patch`, and `import_snapshot_into_ledger` outside `tldraw_adapter.rs`.

Expected before deletion:

```text
crates/ocean-gui/src/shell/mod.rs: mod tldraw_adapter;
crates/ocean-gui/src/shell/tldraw_adapter.rs: self-references and in-module tests only
```

- [ ] **Step 2: Remove the module registration**

In `crates/ocean-gui/src/shell/mod.rs`, delete this line:

```rust
mod tldraw_adapter;
```

- [ ] **Step 3: Delete the file**

Delete:

```text
crates/ocean-gui/src/shell/tldraw_adapter.rs
```

- [ ] **Step 4: Verify default and LiveKit builds**

Run:

```sh
cargo check -p ocean-gui
cargo check -p ocean-gui --features livekit
cargo test -p ocean-gui
cargo clippy -p ocean-gui -- -D warnings
```

Expected: all pass; test count drops by the removed adapter's in-module tests only.

- [ ] **Step 5: Commit**

```sh
git add crates/ocean-gui/src/shell/mod.rs
git rm crates/ocean-gui/src/shell/tldraw_adapter.rs
git commit -m "refactor(ocean-gui): remove dead tldraw adapter"
```

---

### Task 3: Source LiveKit room metadata from native `CanvasLedgerSet`

**Files:**
- Modify: `crates/ocean-gui/src/shell/surface_livekit.rs`
- Modify: `crates/ocean-gui/src/shell/view.rs`

- [ ] **Step 1: Write the failing metadata tests**

In `crates/ocean-gui/src/shell/surface_livekit.rs`, update or add tests so room metadata is built from `CanvasLedgerSet`, not `SurfaceState`.

Add this helper inside the existing `#[cfg(test)] mod tests`:

```rust
fn native_set_with_ledgers() -> CanvasLedgerSet {
    let mut set = CanvasLedgerSet::new();
    set.put(native_ledger_with_components("canvas:main", 2));
    set.put(native_ledger_with_components("canvas:storyboard", 1));
    set.set_active(&CanvasId::new("canvas:storyboard"));
    set
}
```

Add this test:

```rust
#[test]
fn room_metadata_lists_native_canvas_summaries_from_ledger_set() {
    let set = native_set_with_ledgers();

    let metadata = SurfaceLiveKitState::default().room_metadata_for(
        &set,
        Some("agent-session-1"),
    );

    assert_eq!(metadata.version, 1);
    assert_eq!(metadata.agent_session_id.as_deref(), Some("agent-session-1"));
    assert_eq!(metadata.active_canvas_id.as_deref(), Some("canvas:storyboard"));
    assert_eq!(metadata.canvas_revision, Some(1));
    assert_eq!(metadata.canvases.len(), 2);
    assert!(metadata.canvases.iter().any(|canvas| {
        canvas.canvas_id == "canvas:main"
            && canvas.revision == 2
            && canvas.component_count == 2
    }));
    assert!(metadata.canvases.iter().any(|canvas| {
        canvas.canvas_id == "canvas:storyboard"
            && canvas.revision == 1
            && canvas.component_count == 1
    }));
}
```

Run:

```sh
cargo test -p ocean-gui surface_livekit::tests::room_metadata_lists_native_canvas_summaries_from_ledger_set
```

Expected before implementation: fail to compile because `room_metadata_for` still accepts `&SurfaceState`.

- [ ] **Step 2: Update imports and constants**

At the top of `surface_livekit.rs`, remove `SurfaceCanvasContext`, `SurfaceMode`, and `SurfaceState` from the `surface` import.

Use native canvas types:

```rust
use super::canvas::{CanvasId, CanvasLedger, CanvasLedgerSet, CanvasMode};
```

Keep or move the LiveKit/default constants into this module so metadata no longer depends on `surface.rs` for identity constants:

```rust
pub const DEFAULT_SURFACE_SESSION_ID: &str = "surface:main";
pub const DEFAULT_LIVEKIT_ROOM_ID: &str = "project:surface-main";
```

If `DEFAULT_LIVEKIT_ROOM_ID` already exists in `surface.rs`, stop importing it from there and use the local constant in `surface_livekit.rs`.

- [ ] **Step 3: Change the metadata model**

Change `SurfaceCanvasRoomMetadata` from the legacy webview shape:

```rust
pub struct SurfaceCanvasRoomMetadata {
    pub canvas_id: String,
    pub tldraw_room_id: String,
    pub mode: SurfaceMode,
    pub revision: u64,
    pub component_count: usize,
    pub selection_count: usize,
}
```

to the native compact shape:

```rust
pub struct SurfaceCanvasRoomMetadata {
    pub canvas_id: String,
    pub mode: CanvasMode,
    pub revision: u64,
    pub component_count: usize,
    pub selection_count: usize,
}
```

Delete the `impl From<&SurfaceCanvasContext> for SurfaceCanvasRoomMetadata` and replace it with:

```rust
impl From<&CanvasLedger> for SurfaceCanvasRoomMetadata {
    fn from(ledger: &CanvasLedger) -> Self {
        Self {
            canvas_id: ledger.canvas_id.to_string(),
            mode: ledger.mode,
            revision: ledger.revision,
            component_count: ledger.components.len(),
            selection_count: ledger.selection.component_ids.len() + ledger.selection.edge_ids.len(),
        }
    }
}
```

- [ ] **Step 4: Change `room_metadata_for` and JSON wrapper**

Replace the signatures with:

```rust
pub fn room_metadata_for(
    &self,
    ledgers: &CanvasLedgerSet,
    agent_session_id: Option<&str>,
) -> SurfaceRoomMetadata
```

and:

```rust
pub fn room_metadata_for_json(
    &self,
    ledgers: &CanvasLedgerSet,
    agent_session_id: Option<&str>,
) -> Result<String, serde_json::Error>
```

Build metadata from the native set:

```rust
let active_canvas_id = ledgers.active_id().map(ToString::to_string);
let canvas_revision = ledgers.active().map(|ledger| ledger.revision);
let surface_session_id = ledgers
    .active()
    .map(|ledger| ledger.session_id.clone())
    .or_else(|| agent_session_id.map(str::to_string))
    .unwrap_or_else(|| DEFAULT_SURFACE_SESSION_ID.to_string());
let active_pane_id = active_canvas_id
    .clone()
    .unwrap_or_else(|| "canvas:none".to_string());
let canvases = ledgers
    .canvas_ids()
    .into_iter()
    .filter_map(|canvas_id| ledgers.get(&canvas_id))
    .map(SurfaceCanvasRoomMetadata::from)
    .collect();
```

Then construct `SurfaceRoomMetadata` with the same existing `room_id`, `surface_id`, `agent_session_id`, and `media` fields.

- [ ] **Step 5: Update existing metadata tests**

Rewrite existing tests in `surface_livekit.rs` that currently build `SurfaceState::default()` so they build `CanvasLedgerSet` instead.

Examples:

```rust
let mut set = CanvasLedgerSet::new();
set.put(native_ledger_with_components("canvas:main", 3));
let metadata = SurfaceLiveKitState::default().room_metadata_for(&set, Some("agent-session-1"));
```

For the no-canvas case:

```rust
let set = CanvasLedgerSet::new();
let metadata = SurfaceLiveKitState::default().room_metadata_for(&set, Some("agent-1"));
assert_eq!(metadata.active_canvas_id, None);
assert_eq!(metadata.canvas_revision, None);
```

Expected behavior: no `SurfaceState` is required to publish compact room metadata.

- [ ] **Step 6: Update `view.rs` callsites**

In `start_surface_livekit_join`, replace:

```rust
let native_ledger = self.canvas_ledger();
let room_metadata = match self.surface_livekit.room_metadata_for_json(
    &self.surface,
    native_ledger.as_ref(),
    self.agent.session_id.as_deref(),
) {
```

with:

```rust
let room_metadata = match self
    .surface_livekit
    .room_metadata_for_json(&self.canvas_ledgers, self.agent.session_id.as_deref())
{
```

Replace participant attributes source:

```rust
.participant_attributes(self.surface.session_id());
```

with:

```rust
.participant_attributes(self.active_surface_session_id().as_str());
```

Add this method to `impl OceanGuiShell` near other small session helpers:

```rust
fn active_surface_session_id(&self) -> String {
    self.canvas_ledgers
        .active()
        .map(|ledger| ledger.session_id.clone())
        .or_else(|| self.agent.session_id.clone())
        .unwrap_or_else(|| surface_livekit::DEFAULT_SURFACE_SESSION_ID.to_string())
}
```

Make the same metadata/participant-attribute replacement in `current_surface_livekit_update`.

- [ ] **Step 7: Remove the legacy presence fallback**

In `render_canvas_presence_overlay`, replace:

```rust
let active_canvas_id = self
    .canvas_ledgers
    .active_id()
    .map(|id| id.to_string())
    .or_else(|| self.surface.active_canvas_id().map(str::to_string));
```

with:

```rust
let active_canvas_id = self.canvas_ledgers.active_id().map(|id| id.to_string());
```

Expected behavior: presence scopes to the native active canvas only; no legacy `SurfaceState` fallback.

- [ ] **Step 8: Verify metadata and full gates**

Run:

```sh
cargo test -p ocean-gui surface_livekit::tests::room_metadata_lists_native_canvas_summaries_from_ledger_set
cargo test -p ocean-gui surface_livekit::tests
cargo check -p ocean-gui
cargo check -p ocean-gui --features livekit
cargo test -p ocean-gui
cargo clippy -p ocean-gui -- -D warnings
```

Expected: all pass. Existing co-edit tests remain green.

- [ ] **Step 9: Commit**

```sh
git add crates/ocean-gui/src/shell/surface_livekit.rs crates/ocean-gui/src/shell/view.rs
git commit -m "refactor(ocean-gui): source room metadata from native canvases"
```

---

### Task 4: Introduce `shell/chrome.rs` and make Agent-first navigation explicit

**Files:**
- Create: `crates/ocean-gui/src/shell/chrome.rs`
- Modify: `crates/ocean-gui/src/shell/mod.rs`
- Modify: `crates/ocean-gui/src/shell/view.rs`

- [ ] **Step 1: Create the chrome module with tab tests**

Create `crates/ocean-gui/src/shell/chrome.rs` with:

```rust
//! App chrome: top-level navigation, top bar, and status bar for OceanGuiShell.

use gpui::{div, IntoElement, Render};

use super::theme;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SurfaceTab {
    Surface,
    Agent,
    Vault,
}

impl Default for SurfaceTab {
    fn default() -> Self {
        Self::Agent
    }
}

pub(super) const SURFACE_TABS: [SurfaceTab; 3] = [
    SurfaceTab::Agent,
    SurfaceTab::Surface,
    SurfaceTab::Vault,
];

impl SurfaceTab {
    pub(super) fn label(self) -> &'static str {
        match self {
            SurfaceTab::Surface => "Canvas",
            SurfaceTab::Agent => "Agent",
            SurfaceTab::Vault => "Vault",
        }
    }

    pub(super) fn id(self) -> usize {
        match self {
            SurfaceTab::Agent => 0,
            SurfaceTab::Surface => 1,
            SurfaceTab::Vault => 2,
        }
    }
}

pub(super) struct ToolbarTooltip {
    pub(super) label: &'static str,
}

impl Render for ToolbarTooltip {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .bg(theme::paper())
            .border_1()
            .border_color(theme::rule())
            .rounded_md()
            .font_family(theme::MONO_FONT)
            .text_xs()
            .text_color(theme::ink())
            .child(self.label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_agent_first_navigation() {
        assert_eq!(SurfaceTab::default(), SurfaceTab::Agent);
        assert_eq!(SURFACE_TABS, [SurfaceTab::Agent, SurfaceTab::Surface, SurfaceTab::Vault]);
        assert_eq!(SurfaceTab::Surface.label(), "Canvas");
        assert_eq!(SurfaceTab::Agent.id(), 0);
    }
}
```

If imports are too broad after compilation, remove unused ones instead of adding allows.

- [ ] **Step 2: Register the module**

In `crates/ocean-gui/src/shell/mod.rs`, add near other shell modules:

```rust
mod chrome;
```

- [ ] **Step 3: Remove the old local `SurfaceTab` and `ToolbarTooltip` definitions from `view.rs`**

Delete `SurfaceTab` and its impl from `view.rs` lines around 87-110.

Delete `ToolbarTooltip` and its `impl Render` from the bottom of `view.rs`.

Add import near other `super::` imports:

```rust
use super::chrome::{SurfaceTab, ToolbarTooltip, SURFACE_TABS};
```

- [ ] **Step 4: Make the shell start Agent-first**

In `OceanGuiShell::new`, replace:

```rust
active_surface: SurfaceTab::Surface,
```

with:

```rust
active_surface: SurfaceTab::default(),
```

Expected behavior: the app initially renders the Agent workspace, matching the approved Agent-first structure.

- [ ] **Step 5: Use the chrome tab order**

In `render_surface_tabs`, replace:

```rust
[SurfaceTab::Surface, SurfaceTab::Agent, SurfaceTab::Vault]
```

with:

```rust
SURFACE_TABS
```

Expected UI order: Agent, Canvas, Vault.

- [ ] **Step 6: Run tab tests and compile gates**

Run:

```sh
cargo test -p ocean-gui chrome::tests::defaults_to_agent_first_navigation
cargo check -p ocean-gui
cargo check -p ocean-gui --features livekit
cargo test -p ocean-gui
cargo clippy -p ocean-gui -- -D warnings
```

Expected: all pass.

- [ ] **Step 7: Commit**

```sh
git add crates/ocean-gui/src/shell/chrome.rs crates/ocean-gui/src/shell/mod.rs crates/ocean-gui/src/shell/view.rs
git commit -m "refactor(ocean-gui): add agent-first shell chrome module"
```

---

### Task 5: Move top bar and status bar render methods into `chrome.rs`

**Files:**
- Modify: `crates/ocean-gui/src/shell/chrome.rs`
- Modify: `crates/ocean-gui/src/shell/view.rs`

- [ ] **Step 1: Widen the narrow field visibility needed by chrome**

In `OceanGuiShell`, make only these fields `pub(super)`:

```rust
pub(super) active_surface: SurfaceTab,
pub(super) state: ShellState,
pub(super) agent: AgentState,
pub(super) surface: SurfaceState,
pub(super) daemon: NativeDaemonState,
pub(super) gui_control: GuiControlState,
pub(super) command_palette: Option<CommandPaletteState>,
```

At this point in the sequence, command palette extraction has not run yet, so the field type is still `CommandPaletteState`.

- [ ] **Step 2: Widen the helper methods chrome calls**

Change these methods in `view.rs` from private to `pub(super)`:

```rust
pub(super) fn icon(&self, icon: ShellIcon, color: Hsla, size: f32) -> impl IntoElement
pub(super) fn copper_rule(&self) -> Div
pub(super) fn render_top_toolbar(&self, cx: &mut Context<Self>) -> Div
pub(super) fn render_agent_picker_bar(&self, cx: &mut Context<Self>) -> Option<Div>
```

Do not change their bodies.

- [ ] **Step 3: Move render methods verbatim**

Move these methods from `view.rs` into an `impl OceanGuiShell` block in `chrome.rs`:

```text
render_top_bar
render_surface_tabs
render_surface_tab
render_status_bar
```

Do not change the method bodies except for imports and `SURFACE_TABS` usage already introduced in Task 4.

- [ ] **Step 4: Fix imports with compiler guidance only**

Run:

```sh
cargo check -p ocean-gui
```

Expected first result after moving may be import/privacy errors. Fix only:

- Missing GPUI imports in `chrome.rs`.
- Missing `theme`, `ShellIcon`, `RegionId`, `REGION_CHAT_INLINE` imports.
- Missing `pub(super)` visibility on exact fields/methods reported by the compiler.

Do not refactor render logic.

- [ ] **Step 5: Run full gates**

```sh
cargo check -p ocean-gui --features livekit
cargo test -p ocean-gui
cargo clippy -p ocean-gui -- -D warnings
cargo run -p ocean-gui --bin ocean-gui
```

Manual smoke: top bar renders, tabs switch Agent/Canvas/Vault, status bar updates, command palette behavior unchanged.

- [ ] **Step 6: Commit**

```sh
git add crates/ocean-gui/src/shell/chrome.rs crates/ocean-gui/src/shell/view.rs
git commit -m "refactor(ocean-gui): extract shell chrome rendering"
```

---

### Task 6: Extract command palette state and rendering

**Files:**
- Create: `crates/ocean-gui/src/shell/command_palette.rs`
- Modify: `crates/ocean-gui/src/shell/mod.rs`
- Modify: `crates/ocean-gui/src/shell/view.rs`

- [ ] **Step 1: Write standalone palette tests**

Create `command_palette.rs` with the moved `CommandPaletteState`, `PaletteEntry`, and `command_palette_text` logic from `view.rs`.

Add tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::model::ShellState;

    #[test]
    fn empty_query_lists_commands_before_notes() {
        let state = ShellState::seed();
        let palette = CommandPaletteState::default();
        let entries = palette.entries(&state);

        assert!(matches!(entries.first(), Some(PaletteEntry::Command(_))));
        assert!(!entries.is_empty());
    }

    #[test]
    fn moving_selection_clamps_to_entries() {
        let mut palette = CommandPaletteState::default();
        palette.move_selection(99, 3);
        assert_eq!(palette.selected, 2);
        palette.move_selection(-99, 3);
        assert_eq!(palette.selected, 0);
        palette.move_selection(1, 0);
        assert_eq!(palette.selected, 0);
    }
}
```

Run:

```sh
cargo test -p ocean-gui command_palette::tests
```

Expected before wiring imports correctly: compile failures identifying missing visibility/imports.

- [ ] **Step 2: Register the module**

In `shell/mod.rs`, add:

```rust
mod command_palette;
```

- [ ] **Step 3: Move palette state and entry types**

Move from `view.rs` into `command_palette.rs`:

```text
CommandPaletteState
PaletteEntry
impl CommandPaletteState
command_palette_text
```

Make types/methods needed by `view.rs` `pub(super)`:

```rust
pub(super) struct CommandPaletteState { pub(super) query: String, pub(super) selected: usize }
pub(super) enum PaletteEntry { Command(CommandSpec), Note(NoteSearchResult) }
```

- [ ] **Step 4: Move render methods as shell impl methods or palette free functions**

Move these methods into `command_palette.rs` as an `impl OceanGuiShell` block:

```text
render_command_palette
render_palette_row
```

Keep `execute_palette_entry` and `execute_command` in `view.rs`; they mutate shell-wide state and watcher/editor scroll.

- [ ] **Step 5: Fix key handler imports**

Update `view.rs` to import:

```rust
use super::command_palette::{command_palette_text, CommandPaletteState, PaletteEntry};
```

Keep the existing `handle_command_palette_key` in `view.rs` unless moving it compiles without pulling command execution into the palette module.

- [ ] **Step 6: Verify gates**

```sh
cargo test -p ocean-gui command_palette::tests
cargo check -p ocean-gui
cargo check -p ocean-gui --features livekit
cargo test -p ocean-gui
cargo clippy -p ocean-gui -- -D warnings
```

- [ ] **Step 7: Commit**

```sh
git add crates/ocean-gui/src/shell/command_palette.rs crates/ocean-gui/src/shell/mod.rs crates/ocean-gui/src/shell/view.rs
git commit -m "refactor(ocean-gui): extract command palette"
```

---

### Task 7: Extract vault/editor render module without moving editor domain state

**Files:**
- Create: `crates/ocean-gui/src/shell/vault_editor.rs`
- Modify: `crates/ocean-gui/src/shell/mod.rs`
- Modify: `crates/ocean-gui/src/shell/view.rs`

- [ ] **Step 1: Register an empty module**

In `shell/mod.rs`:

```rust
mod vault_editor;
```

Run:

```sh
cargo check -p ocean-gui
```

Expected: pass.

- [ ] **Step 2: Move vault render methods only**

Move these methods from `view.rs` into `vault_editor.rs` as an `impl OceanGuiShell` block:

```text
render_vault_workspace
render_vault_toolbar
render_file_tree
render_file_row
render_editor
render_tabs
render_tab
render_buffer
render_inspector
render_outline_item
render_link_row
render_backlink_row
```

Do not move `ShellState`, `TextBuffer`, `EditorLayout`, `VaultIndex`, or watcher logic in this task. Those domain modules are already isolated.

- [ ] **Step 3: Widen only required helpers/fields**

When the compiler reports privacy errors, make only the needed items `pub(super)`.

Likely fields:

```rust
state
editor_focus
editor_bounds
editor_visual_scroll_row
editor_scroll_path
editor_layout_cache
editor_shape_cache
```

Likely helper methods:

```rust
panel_header
stat_row
sync_editor_scroll_path
reset_editor_scroll
visible_render_lines
line_column_from_editor_point
reveal_editor_cursor
```

Do not move watcher pumps yet.

- [ ] **Step 4: Keep custom editor element grouped**

Move the following as a grouped block into `vault_editor.rs` with `render_buffer`; these types/functions are the custom editor element implementation:

```text
EditorSurfaceElement
EditorInputElement
EditorLayoutCacheKey
EditorLayoutLineKey
EditorLayoutCache
EditorShapeKey
EditorShapeCache
paint_editor_line_number
paint_editor_continuation_marker
paint_editor_gutter_label
shape_editor_text_line
markdown_text_runs
markdown_runs
markdown_token_at
delimited_token
next_char_boundary
editor_text_run
text_run_style
base_text_run_style
pixel_cache_key
```

Keep bodies unchanged.

- [ ] **Step 5: Verify vault/editor behavior**

Run:

```sh
cargo check -p ocean-gui
cargo check -p ocean-gui --features livekit
cargo test -p ocean-gui model::tests
cargo test -p ocean-gui editor_buffer::tests
cargo test -p ocean-gui editor_layout::tests
cargo test -p ocean-gui vault_index::tests
cargo test -p ocean-gui
cargo clippy -p ocean-gui -- -D warnings
```

Manual smoke:

```sh
cargo run -p ocean-gui --bin ocean-gui
```

Check: Vault tab opens, file tree renders, editor buffer still accepts typing/navigation if a note is open.

- [ ] **Step 6: Commit**

```sh
git add crates/ocean-gui/src/shell/vault_editor.rs crates/ocean-gui/src/shell/mod.rs crates/ocean-gui/src/shell/view.rs
git commit -m "refactor(ocean-gui): extract vault editor rendering"
```


---

### Task 8: Extract LiveKit panel render-only helpers

**Files:**
- Create: `crates/ocean-gui/src/shell/livekit_panel.rs`
- Modify: `crates/ocean-gui/src/shell/mod.rs`
- Modify: `crates/ocean-gui/src/shell/view.rs`

- [ ] **Step 1: Register module**

```rust
mod livekit_panel;
```

Run:

```sh
cargo check -p ocean-gui
```

Expected: pass.

- [ ] **Step 2: Move only render/data-display types**

Move from `view.rs` to `livekit_panel.rs`:

```text
SurfaceVideoTile
render_surface_livekit_video_tiles
render_canvas_presence_overlay
presence_markers_for
presence_color
PresenceMarker
render_image_from_bgra
```

Keep reducers and co-edit handshake in `view.rs`:

```text
apply_surface_livekit_client_event
apply_surface_livekit_message
request_surface_livekit_token
start_surface_livekit_join
sync_surface_livekit_update
drain_canvas_sync_outbound
request_canvas_snapshot*
apply_canvas_sync_message
clear_canvas_sync_state
spawn_surface_livekit_task
```

- [ ] **Step 3: Extract mic/camera button block without moving surface toolbar**

Create a helper in `livekit_panel.rs`:

```rust
impl OceanGuiShell {
    pub(super) fn render_livekit_media_buttons(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            self.toolbar_icon_button(
                "toolbar-surface-mic",
                if self.surface_livekit.mic_enabled() { ShellIcon::Chat } else { ShellIcon::Diff },
                "Toggle mic intent",
                cx,
                |shell, cx| {
                    shell.toggle_surface_mic();
                    cx.notify();
                },
            ).into_any_element(),
            self.toolbar_icon_button(
                "toolbar-surface-camera",
                if self.surface_livekit.camera_enabled() { ShellIcon::Check } else { ShellIcon::Files },
                "Toggle camera intent",
                cx,
                |shell, cx| {
                    shell.toggle_surface_camera();
                    cx.notify();
                },
            ).into_any_element(),
        ]
    }
}
```

Adjust exact icon variants to match `ShellIcon` names in `icons.rs`; if `Audio`/`Video` do not exist, keep the existing icon variants used in `render_surface_toolbar`.

- [ ] **Step 4: Call the helper from `render_surface_toolbar`**

In `render_surface_toolbar`, replace the existing mic/camera button children with a loop that appends `render_livekit_media_buttons(cx)` results.

Do not move the rest of `render_surface_toolbar` in this task.

- [ ] **Step 5: Verify LiveKit feature boundary**

Run:

```sh
cargo check -p ocean-gui
cargo check -p ocean-gui --features livekit
cargo test -p ocean-gui surface_livekit::tests
cargo test -p ocean-gui surface_livekit_client::tests
cargo test -p ocean-gui
cargo clippy -p ocean-gui -- -D warnings
```

Expected: all pass. `livekit_panel.rs` must not import the `livekit` crate directly; it should use always-compiled facade/state types.

- [ ] **Step 6: Commit**

```sh
git add crates/ocean-gui/src/shell/livekit_panel.rs crates/ocean-gui/src/shell/mod.rs crates/ocean-gui/src/shell/view.rs
git commit -m "refactor(ocean-gui): extract livekit panel rendering"
```

---

### Task 9: Extract Agent thread render-only helpers

**Files:**
- Create: `crates/ocean-gui/src/shell/agent_thread.rs`
- Modify: `crates/ocean-gui/src/shell/mod.rs`
- Modify: `crates/ocean-gui/src/shell/view.rs`

- [ ] **Step 1: Register module**

```rust
mod agent_thread;
```

Run:

```sh
cargo check -p ocean-gui
```

Expected: pass.

- [ ] **Step 2: Move pure helper functions and tests first**

Move these free functions from `view.rs` into `agent_thread.rs`:

```text
compact_text_stat
tool_call_summary
permission_args_summary
render_props_text
current_model_toolbar_label
current_project_toolbar_label
current_session_toolbar_label
session_title_hint
short_session_label
compact_session_title
turns_from_session_transcript
build_submit_prompt
gui_command_for_agent_event
```

Move associated tests from `view.rs` into `agent_thread.rs` for moved functions, including tests for:

```text
render_props_text_prefers_content_bearing_keys
toolbar labels / session titles
native canvas context injection tests for build_submit_prompt
```

- [ ] **Step 3: Verify pure-helper tests**

Run:

```sh
cargo test -p ocean-gui agent_thread::tests
cargo test -p ocean-gui
```

Expected: all pass.

- [ ] **Step 4: Move render methods only**

Move these methods into `agent_thread.rs` as `impl OceanGuiShell` methods:

```text
render_agent_workspace
render_permission_banner
render_permission_card
render_agent_sidebar
agent_metric_row
roster_metric_row
render_agent_transcript
render_agent_turn
render_agent_block
collapsible_agent_block
agent_block_detail
render_agent_composer
render_agent_toolbar
render_agent_picker_bar
render_model_picker_panel
render_project_picker_panel
render_session_picker_panel
```

Keep these in `view.rs`:

```text
submit_agent_prompt
apply_agent_event
apply_agent_stream_messages
apply_agent_submit_message
apply_control_event
connect_agent_events
connect_control_events
send_component_event
pin_agent_turn_to_canvas
render_agent_component_to_canvas
apply_surface_patch_event
all spawn_agent_*_task helpers
```

Reason: those functions are the session/daemon/canvas event spine and include the cross-session bleed guard, per-turn decision token, and canvas patch routing.

- [ ] **Step 5: Fix privacy mechanically**

When compiling, make only needed fields/helpers `pub(super)`. Do not move shell task handles or event generation fields.

Likely fields needed by render methods:

```rust
agent
agent_focus
agent_scroll
model_catalog
project_catalog
current_project
session_catalog
pending_permissions
model_picker_open
project_picker_open
session_picker_open
surface_livekit
surface_video_tiles
rooms
state
daemon
gui_control
```

Likely methods needed:

```rust
toolbar_icon_button
agent_toolbar_picker_button
picker_row
picker_action_row
picker_placeholder_row
panel_header
stat_row
health_dot
agent_status_dot
pin_agent_turn_to_canvas
send_component_event
submit_agent_prompt
```

- [ ] **Step 6: Verify session/canvas invariants remained in `view.rs`**

Search `view.rs` and confirm these functions still live there:

```text
apply_agent_event
apply_control_event
connect_agent_events
submit_agent_prompt
apply_surface_patch_event
apply_canvas_sync_message
clear_canvas_sync_state
```

Expected: all still in `view.rs`.

- [ ] **Step 7: Run gates**

```sh
cargo check -p ocean-gui
cargo check -p ocean-gui --features livekit
cargo test -p ocean-gui agent_thread::tests
cargo test -p ocean-gui agent::tests
cargo test -p ocean-gui
cargo clippy -p ocean-gui -- -D warnings
cargo run -p ocean-gui --bin ocean-gui
```

Manual smoke: Agent tab opens first, composer focus works, prompt submission path still reaches daemon when daemon is running, tool blocks still expand/collapse, permission banner still renders.

- [ ] **Step 8: Commit**

```sh
git add crates/ocean-gui/src/shell/agent_thread.rs crates/ocean-gui/src/shell/mod.rs crates/ocean-gui/src/shell/view.rs
git commit -m "refactor(ocean-gui): extract agent thread rendering"
```

---

### Task 10: Final cleanup pass and proof

**Files:**
- Modify: `crates/ocean-gui/src/shell/view.rs`
- Modify: `crates/ocean-gui/src/shell/surface.rs` only if metadata no longer needs legacy surface summaries
- Modify: `crates/ocean-gui/src/shell/mod.rs` if module registrations changed
- Modify: `AGENTS.md` only if the documented launch/ownership contract needs a factual update after implementation
- Modify: `events.md` with a repo ledger entry

- [ ] **Step 1: Search for stale user-facing labels**

Search for these terms:

```text
ocean-gui-egui
TldrawCanvas
tldraw sketch projection
SurfaceLedger
tldraw_room_id
Toggle tldraw / native canvas
surface state
```

Expected after earlier tasks:

- `ocean-gui-egui` has no source/build-configuration references; only the approved spec/plan docs may mention the removed prototype historically.
- `SurfaceLedger` appears only where the optional sketch/webview projection still needs it.
- `tldraw_room_id` appears only in optional webview IPC code, not LiveKit room metadata.
- Toolbar copy says `sketch projection` or `canvas projection`, not `tldraw / native canvas`.

- [ ] **Step 2: Rename visible tldraw toggle copy if the webview projection remains**

In `render_surface_toolbar`, change toolbar copy from:

```rust
"Toggle tldraw / native canvas"
```

to:

```rust
"Toggle sketch projection / native canvas"
```

Change status text in the sketch host from:

```rust
"tldraw sketch projection"
```

to:

```rust
"sketch projection"
```

Do not rename wire fields used by `canvas-web` unless all TypeScript callers are updated in the same commit.

- [ ] **Step 3: Run the full verification gate**

```sh
cargo check -p ocean-gui
cargo test -p ocean-gui
cargo check -p ocean-gui --features livekit
cargo clippy -p ocean-gui -- -D warnings
cargo run -p ocean-gui --bin ocean-gui
```

Manual smoke:

- GPUI app launches.
- Agent is the initial tab.
- Canvas tab still renders native `CanvasLedger` path.
- Optional sketch/webview projection, if still present, is explicitly labeled optional.
- LiveKit feature build compiles.

- [ ] **Step 4: Update repo ledger**

Append one entry to root `events.md` with type `refactor`, area `frontend`, and a concise note covering the implemented structure cleanup.

Use the repo's existing entry format.

- [ ] **Step 5: Commit final cleanup**

```sh
git add crates/ocean-gui/src/shell/view.rs crates/ocean-gui/src/shell/surface.rs crates/ocean-gui/src/shell/mod.rs AGENTS.md events.md
git commit -m "refactor(ocean-gui): finalize shell structure cleanup"
```

Only add files that actually changed.

---

## Execution notes

- Use fresh subagents per task. These tasks touch shared files, especially `view.rs`, so run them sequentially or with explicit IRC coordination. Do not run Task 4/5/6/7/8/9 in parallel against `view.rs`.
- Task 1 and Task 2 can run before the extraction tasks, but Task 3 should finish before any attempt to delete or heavily rename legacy `surface.rs` concepts.
- The current approved design spec is `docs/superpowers/specs/2026-07-04-ocean-gui-structure-cleanup-design.md`.
- The committed design spec is `2350d2e docs: design ocean-gui structure cleanup`.
- The prior canvas LiveKit co-edit implementation is complete and must be preserved; do not re-open that plan except to verify its tests stay green.
