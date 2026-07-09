import * as vscode from "vscode";
import * as fs from "node:fs";
import { log, logError } from "./logger";
import { startWebviewProxy, type WebviewProxy } from "./webviewProxy";

/**
 * Hosts the real Ocean Leptos WASM surface (the same bundle the web app and
 * Chrome side panel run) inside a VS Code editor panel.
 *
 * The WASM talks to the daemon over HTTP + SSE. A `vscode-webview://` document
 * can't reach the daemon directly (its CORS predicate rejects that origin), so
 * we stand up a loopback CORS proxy ([`startWebviewProxy`]) and publish its URL
 * on `window.__ocean_daemon_url`; the WASM's bootstrap reads that and connects
 * through it (see `ocean-surface-ui/src/daemon.rs::injected_daemon_url`).
 */
export class WebSurfacePanel {
  public static readonly viewType = "ocean.webSurface";
  private static current: WebSurfacePanel | undefined;
  private static proxy: WebviewProxy | undefined;

  static async show(
    extensionUri: vscode.Uri,
    daemonUrl: string,
  ): Promise<void> {
    if (WebSurfacePanel.current) {
      WebSurfacePanel.current.panel.reveal(vscode.ViewColumn.Active);
      return;
    }

    // One shared proxy for the lifetime of the extension host.
    if (!WebSurfacePanel.proxy) {
      try {
        WebSurfacePanel.proxy = await startWebviewProxy(daemonUrl);
      } catch (error) {
        logError("Failed to start webview daemon proxy", error);
        await vscode.window.showErrorMessage(
          `Ocean: could not start the daemon proxy (${String(error)}).`,
        );
        return;
      }
    }

    const panel = vscode.window.createWebviewPanel(
      WebSurfacePanel.viewType,
      "Ocean",
      vscode.ViewColumn.Active,
      {
        enableScripts: true,
        retainContextWhenHidden: true,
        localResourceRoots: [
          vscode.Uri.joinPath(extensionUri, "media", "webapp"),
        ],
      },
    );
    panel.iconPath = vscode.Uri.joinPath(extensionUri, "media", "wave.svg");
    WebSurfacePanel.current = new WebSurfacePanel(panel, extensionUri);
  }

  static disposeShared(): void {
    WebSurfacePanel.proxy?.dispose();
    WebSurfacePanel.proxy = undefined;
  }

  private constructor(
    private readonly panel: vscode.WebviewPanel,
    extensionUri: vscode.Uri,
  ) {
    const proxyUrl = WebSurfacePanel.proxy!.url;
    panel.webview.html = renderWasmHtml(panel.webview, extensionUri, proxyUrl);
    panel.onDidDispose(() => {
      WebSurfacePanel.current = undefined;
    });
  }
}

function findAsset(dir: string, pattern: RegExp): string | undefined {
  try {
    return fs.readdirSync(dir).find((name) => pattern.test(name));
  } catch {
    return undefined;
  }
}

function renderWasmHtml(
  webview: vscode.Webview,
  extensionUri: vscode.Uri,
  daemonUrl: string,
): string {
  const webappDir = vscode.Uri.joinPath(extensionUri, "media", "webapp");
  const js = findAsset(webappDir.fsPath, /^ocean-surface-ui-.*\.js$/);
  const wasm = findAsset(webappDir.fsPath, /_bg\.wasm$/);
  const css = findAsset(webappDir.fsPath, /^style-.*\.css$/);

  if (!js || !wasm || !css) {
    logError(
      `Ocean web surface bundle missing in ${webappDir.fsPath} ` +
        `(js=${js}, wasm=${wasm}, css=${css})`,
    );
    return missingBundleHtml();
  }

  const jsUri = webview.asWebviewUri(vscode.Uri.joinPath(webappDir, js));
  const wasmUri = webview.asWebviewUri(vscode.Uri.joinPath(webappDir, wasm));
  const cssUri = webview.asWebviewUri(vscode.Uri.joinPath(webappDir, css));
  const nonce = String(Date.now());
  const wsUrl = daemonUrl.replace(/^http/, "ws");

  // WASM needs 'wasm-unsafe-eval'; the app fetches its own .wasm (connect-src
  // cspSource) and talks to the loopback proxy (connect-src the proxy origin).
  // Leptos sets inline style attributes, so style-src needs 'unsafe-inline'.
  const csp = [
    "default-src 'none'",
    `img-src ${webview.cspSource} data:`,
    `style-src ${webview.cspSource} 'unsafe-inline'`,
    `font-src ${webview.cspSource}`,
    `script-src ${webview.cspSource} 'wasm-unsafe-eval' 'nonce-${nonce}'`,
    `connect-src ${webview.cspSource} ${daemonUrl} ${wsUrl}`,
  ].join("; ");

  log(`Ocean web surface loading: ${js} -> daemon ${daemonUrl}`);

  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta http-equiv="Content-Security-Policy" content="${csp}" />
  <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover" />
  <meta name="theme-color" content="#06111d" />
  <link rel="stylesheet" href="${cssUri}" />
  <title>Ocean</title>
  <script nonce="${nonce}">window.__ocean_daemon_url = ${JSON.stringify(daemonUrl)};</script>
  <script type="module" nonce="${nonce}">
    import init from "${jsUri}";
    init({ module_or_path: "${wasmUri}" }).catch((err) => {
      document.body.innerHTML =
        '<pre style="color:#ff7f95;font:13px monospace;padding:16px">Ocean WASM failed to load:\\n' +
        String(err) + '</pre>';
    });
  </script>
</head>
<body></body>
</html>`;
}

function missingBundleHtml(): string {
  return `<!doctype html><html><head><meta charset="utf-8" /></head>
<body style="font:14px -apple-system,sans-serif;color:#e8eef5;background:#06111d;padding:24px">
  <h2 style="color:#7fe7c8">Ocean web surface bundle not found</h2>
  <p>The Leptos WASM bundle is missing from <code>media/webapp/</code>.</p>
  <p>Build it and copy it in:</p>
  <pre style="background:rgba(255,255,255,.06);padding:12px;border-radius:8px">trunk build --release
cp dist/ocean-surface-ui-*.js dist/ocean-surface-ui-*_bg.wasm dist/style-*.css \\
   vscode-extension/media/webapp/</pre>
</body></html>`;
}
