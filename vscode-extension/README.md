# Ocean VS Code / Cursor Extension

First-party Ocean surface for VS Code and **Cursor**. Uses the same ACP client
architecture as production ACP extensions (filesystem, terminal, permissions
via VS Code APIs) and connects to your existing **`ocean-acp`** bridge.

```text
Cursor/VS Code extension (ACP client + IDE context)
        │ stdio JSON-RPC
        ▼
   ocean-acp  (ocean-os)
        │ HTTP + SSE
        ▼
   ocean-daemon
```

This gives **maximum IDE control**: unsaved buffer reads, native terminal
spawns, QuickPick permission prompts, plus automatic injection of active file,
selection, open tabs, diagnostics, and git branch on every turn.

## Prerequisites

1. **ocean-daemon** running (`cargo run -p ocean-daemon --release` in `ocean-os`)
2. **ocean-acp** built (`cargo build -p ocean-acp --release` in `ocean-os`)

## Install (development)

```bash
cd vscode-extension
npm install
npm run compile
```

Then in Cursor/VS Code:

1. **Run Extension Development Host** — open `vscode-extension/` folder, press F5
2. Or package: `npx vsce package` and **Install from VSIX**

## Settings

| Setting | Default | Description |
|---|---|---|
| `ocean.acp.path` | auto-detect | Path to `ocean-acp` binary. Auto-detect checks `~/.cargo/bin`, `~/dev/ocean-os`, `~/ocean-os`, `~/ocean-os-repo`, and sibling repo paths. |
| `ocean.daemon.url` | `http://127.0.0.1:4780` | Daemon URL passed to the bridge |
| `ocean.injectEditorContext` | `true` | Prepend IDE context to prompts |
| `ocean.autoApprovePermissions` | `ask` | `ask` or `allowAll` |
| `ocean.logTraffic` | `false` | Log ACP JSON-RPC to Output → Ocean |
| `ocean.thinkingLevel` | daemon default | Per-turn thinking-level override passed through ACP metadata |

## Usage

1. Click the **Ocean** status bar item or run **Ocean: Command Menu** for the
   native action menu. Use it to open Ocean in the sidebar, bottom panel, or
   editor tab, and to set model, thinking, context, permission, and runtime
   settings without adding controls to the chat surface.
2. Click **Connect** (or it connects on first message)
3. Chat — editor context is injected automatically. Type `@current` /
   `@active` / `@file` for the active buffer, `@selection` / `@sel` for the
   selected text, `@tabs` / `@open` for open workspace tabs, `@workspace` /
   `@codebase` / `@tree` for the bounded workspace file map, `@path/to/file`
   for bounded workspace file context, `@changes` / `@git` / `@diff` for
   current git status and diffs, or `@problems` / `@diagnostics` for current
   workspace Problems. Use `@terminal` / `@term` for recent terminal output.
   Typing `@` opens the composer context picker; continue typing to search
   workspace files and insert a bounded file-context mention.
4. Right-click selected code → **Ocean: Ask About Selection** (⌘⇧O on Mac)
5. Use **Ocean: Inline Assist** to preview a native diff before applying a
   selected-text rewrite or cursor insertion
6. Use **Ocean: Ask About Current File** for an unsaved-buffer-aware file prompt
7. Use **Ocean: Ask About Files** to pick explicit workspace files as bounded
   prompt context, including from Explorer file selections
8. Use the editor lightbulb or **Ocean: Fix Diagnostics** to send active-file
   Problems diagnostics with exact buffer content
9. Use **Ocean: Review Workspace Changes** for a git-status and bounded-diff
   review prompt
10. Use **Ocean: Show Last Edits** to inspect grouped Ocean-applied edit sets in
   VS Code's native diff editor
11. Use **Ocean: Revert Last Edit** to restore a selected captured edit after
   confirmation
12. Use **Ocean: Revert Edit Set** to roll back every safely captured file edit
   from a selected Ocean turn after confirmation
13. Use **Ocean: Open Recent Session**, **Ocean: Refresh Sessions**, or
   **Ocean: Rename Current Session** for thread history. Recent sessions refresh
   from ACP when the bridge advertises `session/list`; transcripts are cached
   locally so loaded sessions have immediate visible context while the daemon
   remains the session authority.
14. Use **Ocean: Copy Last Response** or **Ocean: Copy Transcript** from the
   command palette or Ocean status menu.
15. File references in transcript text and compact tool output, including
   inline-code refs like `src/file.ts:12`, open in the editor.
16. The composer restores its local draft after webview reloads and uses
   up/down arrow recall for recently sent prompts when the caret is on the
   first/last composer line.
17. Typing `/` at the start of a composer line opens transient workflow
   shortcuts such as `/review`, `/changes`, `/fix`, and `/workspace`.

## Why this approach (vs generic ACP Client)

The [formulahendry ACP Client](https://marketplace.visualstudio.com/items?itemName=formulahendry.acp-client)
is excellent for multi-agent setups. This extension is Ocean-specific:

- Tags daemon turns as `acp-vscode` via `OCEAN_ACP_CLIENT_TYPE`
- Injects rich `<editor_context>` on every prompt (tabs, diagnostics, git)
- Preconfigured for `ocean-acp` + daemon health checks
- Same full ACP client capabilities (fs, terminal, permissions)

## Related

- `../ocean-os/crates/ocean-acp/` — ACP agent bridge (already built)
- `../extension/` — Chrome side panel surface (`surface-extension`)
- `../crates/ocean-surface-ui/` — Leptos web/PWA surface
