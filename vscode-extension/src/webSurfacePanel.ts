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

    // A webview can't necessarily reach a raw loopback URL: under Remote SSH,
    // WSL, Codespaces or a devcontainer the extension host runs somewhere the
    // webview's browser cannot route to. asExternalUri maps the local port to
    // whatever the client can actually reach (a no-op locally).
    const externalUrl = (
      await vscode.env.asExternalUri(
        vscode.Uri.parse(WebSurfacePanel.proxy.url),
      )
    )
      .toString()
      .replace(/\/$/, "");
    log(`Web surface daemon endpoint (external): ${externalUrl}`);

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
    WebSurfacePanel.current = new WebSurfacePanel(panel, extensionUri, externalUrl);
  }

  static disposeShared(): void {
    WebSurfacePanel.proxy?.dispose();
    WebSurfacePanel.proxy = undefined;
  }

  private constructor(
    private readonly panel: vscode.WebviewPanel,
    extensionUri: vscode.Uri,
    daemonUrl: string,
  ) {
    panel.webview.html = renderWasmHtml(panel.webview, extensionUri, daemonUrl);

    // The webview reports what it actually sees, so diagnosing a failed connect
    // needs only the Output -> Ocean channel (no devtools, no frame switching:
    // VS Code runs webview content in a nested iframe, so a console typed
    // against the default `top` context reads the wrong window).
    panel.webview.onDidReceiveMessage((msg) => {
      switch (msg?.type) {
        case "diag":
          log(
            `Web surface diag: injected __ocean_daemon_url=${
              msg.daemonUrl ?? "<undefined>"
            } origin=${msg.origin}`,
          );
          break;
        case "wasm-ok":
          log("Web surface: WASM initialised");
          break;
        case "wasm-fail":
          logError(`Web surface: WASM init failed: ${msg.error}`);
          break;
        case "csp":
          logError(
            `Web surface CSP violation: directive=${msg.directive} blocked=${msg.blocked}`,
          );
          break;
        default:
          break;
      }
    });

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
  <script nonce="${nonce}">
    // Must run before the (deferred) module script so the WASM bootstrap sees it.
    window.__ocean_daemon_url = ${JSON.stringify(daemonUrl)};
    const __vscode = acquireVsCodeApi();
    window.__ocean_vscode = __vscode;
    __vscode.postMessage({
      type: "diag",
      daemonUrl: window.__ocean_daemon_url ?? null,
      origin: location.origin,
    });
    window.addEventListener("securitypolicyviolation", (e) => {
      __vscode.postMessage({
        type: "csp",
        directive: e.violatedDirective,
        blocked: e.blockedURI,
      });
    });
  </script>
  <script type="module" nonce="${nonce}">
    import init from "${jsUri}";
    init({ module_or_path: "${wasmUri}" })
      .then(() => window.__ocean_vscode.postMessage({ type: "wasm-ok" }))
      .catch((err) => {
        window.__ocean_vscode.postMessage({ type: "wasm-fail", error: String(err) });
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
