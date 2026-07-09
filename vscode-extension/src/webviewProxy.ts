import * as http from "node:http";
import type { AddressInfo } from "node:net";
import { log, logError } from "./logger";

/**
 * A loopback reverse-proxy the extension host runs in front of the Ocean daemon
 * so the WASM web surface, hosted inside a `vscode-webview://` document, can
 * reach it.
 *
 * The daemon's CORS predicate only trusts loopback and `chrome-extension://`
 * origins, so a direct `fetch`/`EventSource` from a `vscode-webview://<uuid>`
 * origin is blocked by the browser. This proxy sits between them: the webview
 * talks to `http://127.0.0.1:<port>` (this server), which reflects the webview
 * origin in `Access-Control-Allow-Origin` and forwards the request to the
 * daemon server-side, where CORS does not apply. Server-Sent Events are streamed
 * through untouched so `/v1/agent/events` and `/v1/events` keep working.
 */
export interface WebviewProxy {
  /** Base URL the webview should use as its daemon endpoint. */
  readonly url: string;
  dispose(): void;
}

export async function startWebviewProxy(
  daemonUrl: string,
): Promise<WebviewProxy> {
  const target = new URL(daemonUrl);
  const targetPort = Number(target.port || (target.protocol === "https:" ? 443 : 80));

  const server = http.createServer((req, res) => {
    const origin = req.headers.origin ?? "*";
    // CORS for the webview document. We reflect the origin (rather than "*")
    // so this stays valid even if credentialed requests are added later.
    res.setHeader("Access-Control-Allow-Origin", origin);
    res.setHeader("Vary", "Origin");
    res.setHeader(
      "Access-Control-Allow-Methods",
      "GET,POST,PATCH,DELETE,OPTIONS",
    );
    res.setHeader("Access-Control-Allow-Headers", "content-type,authorization");

    // Chromium's Private Network Access: a secure context (the webview, served
    // from vscode-webview://) reaching a private address (127.0.0.1) sends a
    // preflight carrying `Access-Control-Request-Private-Network: true` and
    // requires this header on the response, or the request is blocked before it
    // ever reaches us. Loopback-to-loopback never triggers this, which is why
    // it only shows up inside the webview.
    if (req.headers["access-control-request-private-network"]) {
      res.setHeader("Access-Control-Allow-Private-Network", "true");
    }

    if (req.method === "OPTIONS") {
      res.writeHead(204);
      res.end();
      return;
    }

    // Forward to the daemon. Strip the browser Origin/Referer so the daemon
    // sees a plain server-side request (its CORS predicate would otherwise
    // reject the webview origin, which is irrelevant here but noisy).
    const forwardHeaders: http.OutgoingHttpHeaders = { ...req.headers };
    delete forwardHeaders.origin;
    delete forwardHeaders.referer;
    forwardHeaders.host = `${target.hostname}:${targetPort}`;

    const upstream = http.request(
      {
        host: target.hostname,
        port: targetPort,
        method: req.method,
        path: req.url,
        headers: forwardHeaders,
      },
      (up) => {
        // Pass the daemon's status + headers through, but keep OUR CORS header
        // (the daemon sends none for the webview origin).
        const headers = { ...up.headers };
        delete headers["access-control-allow-origin"];
        res.writeHead(up.statusCode ?? 502, headers);
        // Streams the body incrementally, including text/event-stream SSE.
        up.pipe(res);
      },
    );

    upstream.on("error", (err) => {
      logError("webview proxy upstream error", err);
      if (!res.headersSent) {
        res.writeHead(502);
      }
      res.end("Ocean proxy: daemon unreachable");
    });

    req.pipe(upstream);
  });

  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });

  const port = (server.address() as AddressInfo).port;
  const url = `http://127.0.0.1:${port}`;
  log(`Webview daemon proxy listening on ${url} -> ${daemonUrl}`);

  return {
    url,
    dispose: () => {
      server.close();
    },
  };
}
