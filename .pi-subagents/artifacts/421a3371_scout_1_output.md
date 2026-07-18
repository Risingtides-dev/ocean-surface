# Code Context

## Files Retrieved
1. `crates/ocean-gui/canvas-web/package.json` - React/Vite canvas package; uses tldraw, not Three.js/R3F.
2. `crates/ocean-gui/canvas-web/src/main.tsx` - React entry point mounting local or synced tldraw.
3. `crates/ocean-gui/canvas-web/src/oceanBridge.ts` - IPC bridge and ledger-to-tldraw projection.
4. `crates/ocean-gui/canvas-web/src/ledger.ts` - canvas/ledger data contracts.
5. `crates/ocean-gui/canvas-web/vite.config.ts` - Vite React build configuration.
6. `crates/ocean-gui/canvas-web/index.html` - HTML entry point.
7. `legacy-voice/package.json` - unrelated Node voice surface; no graphics dependencies.
8. `vscode-extension/package.json` - unrelated VS Code/Cursor chat extension.

## Key Code

- Framework: React 19 + Vite 8 + TypeScript.
- Existing canvas framework: tldraw 5 and `@tldraw/sync` 5.
- `main.tsx` selects:
  - `SyncedCanvas` when `syncUri` is configured.
  - `LocalCanvas` otherwise.
- `oceanBridge.ts` converts `LedgerComponent` values into tldraw rectangle (`geo`) shapes and emits IPC events.
- No `three`, `@react-three/fiber`, `@react-three/drei`, or R3F imports/dependencies were found.

Package scripts in `crates/ocean-gui/canvas-web/package.json`:

```json
{
  "dev": "vite --host 127.0.0.1",
  "build": "tsc --noEmit && vite build",
  "preview": "vite preview --host 127.0.0.1"
}
```

## Architecture

This is a legacy/optional GPUI canvas webview adapter. It is not the canonical product surface: `AGENTS.md` identifies `crates/ocean-surface-ui` as the active Leptos/WASM UI, while `crates/ocean-gui` is soft-deprecated. The canvas receives configuration via URL parameters, renders tldraw, and communicates with native Ocean code through `window.oceanSurface`/`window.ipc`.

## Start Here

For a low-risk rail-shooter prototype, start by creating a separate Three.js/R3F scene entry under `crates/ocean-gui/canvas-web/src/` rather than modifying the ledger bridge. Preserve the existing tldraw entry and add a dedicated route or feature entry in `main.tsx` only after the standalone scene builds. Begin with a minimal fixed-rail camera, procedural track, player reticle, and target meshes; avoid daemon/IPC integration until the core loop is proven.

## Review Findings

- **info:** No existing Three.js/R3F game or web app is present.
- **medium:** `crates/ocean-gui/canvas-web` is explicitly a legacy/optional tldraw adapter, so it may be the wrong long-term host for a new game.
- **low:** Existing `main.tsx` and `oceanBridge.ts` are tightly coupled to tldraw and Ocean ledger IPC; directly replacing them would risk breaking native canvas integration.

## Residual Risks

- A new game may be better placed in the canonical Leptos surface or a separate package, depending on requested deployment target.
- No Three.js dependencies, scene systems, asset pipeline, physics, input handling, or game-loop infrastructure currently exist.
- The repository-wide grep was broad but returned no actual Three.js/R3F dependency or source import.