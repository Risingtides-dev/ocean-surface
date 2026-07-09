# Ocean Web Surface in VS Code (spike)

Hosts the **real Ocean Leptos WASM app** — the same bundle the web app and
Chrome side panel run — inside a VS Code / Cursor editor panel, via the command
**“Ocean: Open Web Surface (full app)”** (`ocean.openWebSurface`, also a `$(globe)`
button in the chat view title bar).

This is a spike. The existing ACP chat view is untouched and remains the default;
this opens the full web cockpit alongside it.

## How it works

The WASM app talks to the daemon over **HTTP + SSE**. A `vscode-webview://`
document can't reach the daemon directly — the daemon's CORS predicate only
trusts loopback and `chrome-extension://` origins, so the browser blocks a direct
`fetch`/`EventSource` from the webview.

So the extension host runs a **loopback CORS reverse-proxy**
(`src/webviewProxy.ts`): the webview talks to `http://127.0.0.1:<port>`, which
reflects the webview origin in `Access-Control-Allow-Origin` and forwards to the
daemon server-side (where CORS doesn't apply). SSE streams through untouched.

The panel (`src/webSurfacePanel.ts`) publishes the proxy URL on
`window.__ocean_daemon_url`, and the WASM's bootstrap reads it:
`crates/ocean-surface-ui/src/daemon.rs::injected_daemon_url()` — a new branch
alongside the existing `chrome-extension://` handling — so the app skips
`/api/config` and connects through the proxy.

No changes to the `ocean-os` daemon are required.

```
vscode-webview://  ──HTTP+SSE──►  127.0.0.1:<port>  ──HTTP+SSE──►  ocean-daemon
   (WASM app)        (CORS ok)     (extension host      (server-side,   :4780
                                    CORS proxy)          no CORS)
```

## Build

Requires `rustup` + `wasm32-unknown-unknown` + `trunk`.

```sh
vscode-extension/scripts/build-webapp.sh   # trunk build --release + copy bundle
```

This writes the WASM/JS/CSS into `vscode-extension/media/webapp/` (gitignored —
it's a ~7.6 MB regenerated artifact). `vsce package` bundles it into the `.vsix`.

## Test (needs a running daemon)

1. Start the daemon (`ocean-os`): it must be listening on `http://127.0.0.1:4780`.
2. Open `vscode-extension/` in VS Code / Cursor and press **F5** (Extension
   Development Host).
3. Run **Ocean: Open Web Surface (full app)** from the command palette (or the
   globe icon in the chat title bar).
4. The full Leptos cockpit should load and stream turns through the proxy.

If the WASM fails to load, check the Output → Ocean channel and the webview
devtools console (Command Palette → “Developer: Open Webview Developer Tools”).

## Known limitations

- The webview hosts the full cockpit, not the ~360 px compact extension layout
  (that layout is keyed on the `chrome-extension://` host today).
- This view uses the daemon's HTTP/SSE transport, **not** ACP — so the ACP
  chat's native unsaved-buffer reads, terminal spawns, and QuickPick permission
  prompts do not apply to it. Use the ACP chat view for those.
- CSP is loosened for the WASM (`wasm-unsafe-eval`, `style-src 'unsafe-inline'`,
  and `connect-src` to the loopback proxy).
