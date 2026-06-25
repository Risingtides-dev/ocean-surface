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
| `ocean.acp.path` | auto-detect | Path to `ocean-acp` binary |
| `ocean.daemon.url` | `http://127.0.0.1:4780` | Daemon URL passed to the bridge |
| `ocean.injectEditorContext` | `true` | Prepend IDE context to prompts |
| `ocean.autoApprovePermissions` | `ask` | `ask` or `allowAll` |
| `ocean.logTraffic` | `false` | Log ACP JSON-RPC to Output → Ocean |

## Usage

1. Open the **Ocean** activity bar icon → **Chat**
2. Click **Connect** (or it connects on first message)
3. Chat — editor context is injected automatically
4. Right-click selected code → **Ocean: Ask About Selection** (⌘⇧O on Mac)

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
