// Ocean Surface service worker — self-healing, fail-safe shell delivery.
//
// History (OCEAN — "blank pane / 11-minute load"): a wedged sw.js pinned a
// stale cached HTML shell, so the app showed a blank pane and "loaded" for
// minutes even though the server bytes were current and valid. The cause was
// twofold: (a) a slow-but-succeeding network fetch could be pre-empted by a
// stale cached copy, and (b) an OLD already-installed worker kept controlling
// the page and never handed off to a fresh deploy.
//
// This rewrite makes the worker SELF-HEALING and FAIL-SAFE:
//   • The HTML shell + sw.js are ALWAYS served network-fresh — a new deploy is
//     picked up immediately, never pinned behind an old worker.
//   • Navigations are network-FIRST with a short timeout: a slow tunnel fails
//     fast to a cached shell (seconds, not minutes) instead of hanging.
//   • A successful (even if slightly slow) network response is NEVER pre-empted
//     by stale cache — cache is only a last resort when the network is dead.
//   • Hashed static assets are immutable (content-addressed), so cache-first is
//     safe and fast for THOSE — they never change for a given hash.
//   • On activation the new worker claims all clients and tells them to reload,
//     so a deploy auto-updates the user with no manual cache-clear.
//
// NEVER touch /v1/* or /api/* — those are live daemon/proxy calls (turns, SSE,
// STT, TTS, config, permissions) and must always go straight to the network.

// Bump on every deploy to evict the prior shell. `activate` deletes every cache
// that isn't this one, so a version bump guarantees the wedged v3 cache (and
// any older shell) is dropped the instant this worker activates.
const CACHE = 'ocean-shell-v4';

// Precache the install-time PWA bits. The start_url ('/') is fetched on install
// and stored under a STABLE key (OFFLINE_KEY) — separate from live navigation
// responses — so Chrome's installable-PWA check sees a working offline
// start_url, while navigations stay network-first and never serve a stale shell.
const SHELL = ['/manifest.webmanifest', '/icon-192.png', '/icon-512.png'];
const OFFLINE_KEY = '/__offline_shell__';

// How long to wait for the network on a navigation before failing over to the
// cached shell. Short on purpose: a slow/flaky tunnel must fail fast (seconds)
// rather than hang the page for minutes. Once we time out we still serve a
// shell so the app boots; the in-page bundle then talks to the live daemon.
const NAV_TIMEOUT_MS = 5000;

// Fetch with a hard timeout. Resolves with the response if the network answers
// in time; rejects (AbortError) otherwise so the caller can fall back to cache.
function fetchWithTimeout(request, timeoutMs) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  // Force a fresh fetch for the shell so a new deploy is always picked up; the
  // browser HTTP cache must never short-circuit the navigation document.
  return fetch(request, { signal: controller.signal, cache: 'no-store' })
    .finally(() => clearTimeout(timer));
}

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches.open(CACHE).then(async (c) => {
      await c.addAll(SHELL).catch(() => {});
      // Seed the offline navigation fallback so the app is installable +
      // launchable with no network.
      try {
        const resp = await fetch('/', { cache: 'no-store' });
        if (resp.ok) await c.put(OFFLINE_KEY, resp.clone());
      } catch (_) {}
    })
  );
  // Take over immediately — don't wait for old tabs to close. Combined with the
  // activate-time claim + reload message below, a new deploy heals on the spot.
  self.skipWaiting();
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    (async () => {
      // Drop every cache that isn't the current one — evicts the wedged v3
      // shell and any older bundle the instant this worker activates.
      const keys = await caches.keys();
      await Promise.all(
        keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))
      );
      await self.clients.claim();
      // Tell every controlled client a fresh worker is live so it can reload
      // onto the new shell without a manual cache-clear. Pairs with the
      // index.html controllerchange handler (belt-and-suspenders reload).
      const clientList = await self.clients.matchAll({ type: 'window' });
      for (const client of clientList) {
        client.postMessage({ type: 'SW_ACTIVATED', cache: CACHE });
      }
    })()
  );
});

// Hashed, content-addressed assets are immutable: the same URL always maps to
// the same bytes (Trunk emits e.g. `index-<hash>.js`,
// `ocean-surface-ui-<hash>_bg.wasm`, `<name>-<hash>.css`). For THOSE,
// cache-first is correct and fast. The HTML shell and sw.js carry no content
// hash and MUST stay network-fresh, so they are deliberately excluded here.
function isImmutableAsset(url) {
  // Hash is a hex run of 8+ chars right before the extension. wasm-bindgen
  // emits `<name>-<hash>_bg.wasm`, so allow an optional `_bg` between the hash
  // and `.wasm`. JS / CSS are `<name>-<hash>.{js,css}`.
  return /-[0-9a-f]{8,}(?:_bg)?\.(?:js|wasm|css)$/i.test(url.pathname);
}

function isWasm(url) {
  return /\.wasm$/i.test(url.pathname);
}

// A valid WebAssembly module begins with the 4-byte magic "\0asm"
// (0x00 0x61 0x73 0x6D), immediately followed by a 4-byte version. This is the
// SAME guard run-surface.sh applies to dist/*_bg.wasm at build time (OCEAN-121);
// here we apply it at RUNTIME to CACHED bytes so a once-corrupt wasm that got
// pinned in the cache (OCEAN-122 — all-zero bytes from a failed wasm-opt) can
// never be served again.
//
// Pure, side-effect-free, and exported below so it can be unit-tested in node.
// `buffer` is an ArrayBuffer (or any TypedArray-able). Returns true only when
// the first 4 bytes are exactly the wasm magic word; false for short or
// all-zero / otherwise-corrupt buffers.
const WASM_MAGIC = [0x00, 0x61, 0x73, 0x6d];
function hasWasmMagic(buffer) {
  if (!buffer) return false;
  const bytes = new Uint8Array(buffer);
  if (bytes.length < WASM_MAGIC.length) return false;
  for (let i = 0; i < WASM_MAGIC.length; i++) {
    if (bytes[i] !== WASM_MAGIC[i]) return false;
  }
  return true;
}

// Read a Response's first bytes and confirm the wasm magic WITHOUT consuming the
// body we hand back to the page: we always validate against a clone. Returns the
// boolean result; never throws (a body that can't be read is treated as corrupt
// so we fail safe to a network refetch).
async function responseHasWasmMagic(resp) {
  if (!resp) return false;
  try {
    const buf = await resp.clone().arrayBuffer();
    return hasWasmMagic(buf);
  } catch (_) {
    return false;
  }
}

// Tell every controlled client that a cached asset failed its integrity check
// and was busted + refetched. The page can surface a minimal "reload" affordance
// so the user recovers without opening devtools to hand-clear caches.
async function notifyIntegrityFailure(detail) {
  try {
    const clientList = await self.clients.matchAll({ type: 'window' });
    for (const client of clientList) {
      client.postMessage({ type: 'SW_INTEGRITY_FAILURE', ...detail });
    }
  } catch (_) {}
}

// Serve a hashed wasm asset with a runtime integrity gate. Cache-first for
// SPEED, but the cached bytes are validated against the wasm magic word before
// they're served; a corrupt cached entry is busted and refetched from the
// network (and the fresh good copy is re-cached) instead of being served. This
// is the OCEAN-122 fix: a once-pinned all-zero wasm can no longer brick the page
// across reloads/rebuilds.
async function serveWasm(request) {
  const cache = await caches.open(CACHE);
  const hit = await cache.match(request);

  if (hit) {
    if (await responseHasWasmMagic(hit)) {
      // Good cached bytes — serve fast, exactly like any immutable asset.
      return hit;
    }
    // Corrupt cached wasm (e.g. all-zero from a failed build that got cached).
    // Evict it and fall through to a fresh network fetch so we never serve
    // known-dead bytes. Tell clients so they can offer a hard refresh.
    await cache.delete(request).catch(() => {});
    notifyIntegrityFailure({ url: request.url, reason: 'corrupt-cached-wasm' });
  }

  // Cache miss, or we just busted a corrupt entry: fetch fresh — but BYPASS the
  // browser's HTTP disk cache with `cache: 'reload'`. The proxy serves wasm with
  // `Cache-Control: public, max-age=31536000, immutable`
  // (crates/ocean-surface-proxy/src/main.rs), so a plain `fetch(request)` would
  // be free to return the SAME corrupt all-zero bytes straight from the HTTP
  // cache — a layer SEPARATE from the Cache API we just evicted — and we'd hand
  // the page dead bytes on every reload. `reload` forces a network round-trip
  // (revalidating/replacing the HTTP-cache entry) so we actually get fresh bytes,
  // while still populating the HTTP cache with the good copy for next time
  // ('no-store' would skip the network cache entirely; we want it warmed).
  const resp = await fetch(request, { cache: 'reload' });
  // Validate the network bytes too — only cache a copy that actually carries the
  // wasm magic, so we never re-pin a corrupt response served by a broken
  // proxy/build into the Cache API.
  if (resp && resp.ok && (await responseHasWasmMagic(resp))) {
    cache.put(request, resp.clone()).catch(() => {});
  }
  return resp;
}

self.addEventListener('fetch', (event) => {
  const url = new URL(event.request.url);

  // Bypass the SW entirely for live endpoints — let them hit the network
  // directly so SSE streaming and POSTs are never intercepted or cached.
  if (url.pathname.startsWith('/v1/') || url.pathname.startsWith('/api/')) {
    return;
  }

  // Only handle same-origin GETs; everything else passes through.
  if (event.request.method !== 'GET' || url.origin !== self.location.origin) {
    return;
  }

  // sw.js itself must ALWAYS be fetched fresh so a new worker can replace a
  // wedged one. Never serve it from cache (this is the escape hatch — an old
  // worker can't pin its own successor).
  if (url.pathname === '/sw.js') {
    event.respondWith(fetch(event.request, { cache: 'no-store' }));
    return;
  }

  // Navigations (the HTML doc): network-FIRST with a short timeout. A
  // successful response — even a slow one within the timeout — always wins and
  // refreshes the offline fallback. We fall back to the cached shell ONLY when
  // the network is dead or too slow (offline / timeout), so a wedged stale
  // shell can never pre-empt a live, current one. This is what stops the
  // "blank pane + 11-minute load": slow network fails fast to a shell that then
  // boots and talks to the live daemon, instead of hanging indefinitely.
  if (event.request.mode === 'navigate') {
    event.respondWith(
      fetchWithTimeout(event.request, NAV_TIMEOUT_MS)
        .then((resp) => {
          if (resp && resp.ok) {
            const copy = resp.clone();
            caches.open(CACHE).then((c) => c.put(OFFLINE_KEY, copy)).catch(() => {});
          }
          return resp;
        })
        .catch(async () =>
          // Offline / timed out: serve the freshest shell we have, else a
          // minimal inline boot page so the user never stares at a blank pane.
          (await caches.match(OFFLINE_KEY)) ||
          (await caches.match('/')) ||
          new Response(
            '<!doctype html><meta charset=utf-8>' +
              '<title>Ocean</title>' +
              '<body style="background:#060606;color:#B8C0CC;font:16px system-ui;' +
              'display:grid;place-items:center;height:100vh;margin:0">' +
              'Reconnecting…<script>setTimeout(()=>location.reload(),2500)</script>',
            { headers: { 'Content-Type': 'text/html; charset=utf-8' } }
          )
        )
    );
    return;
  }

  // Hashed wasm: cache-first for speed, but VALIDATE the cached bytes against
  // the wasm magic word before serving (OCEAN-122). A corrupt cached wasm is
  // busted + refetched instead of served, so a once-pinned all-zero module can't
  // brick the page across reloads. Handled before the generic immutable branch.
  if (isImmutableAsset(url) && isWasm(url)) {
    event.respondWith(serveWasm(event.request));
    return;
  }

  // Other hashed, immutable assets (js/css): cache-first for speed, fall through
  // to the network (and populate the cache) on a miss. Safe because the URL is
  // content-addressed — a new build ships a NEW filename, so there is no stale
  // version to serve.
  if (isImmutableAsset(url)) {
    event.respondWith(
      caches.match(event.request).then(
        (hit) =>
          hit ||
          fetch(event.request).then((resp) => {
            if (resp && resp.ok) {
              const copy = resp.clone();
              caches.open(CACHE).then((c) => c.put(event.request, copy)).catch(() => {});
            }
            return resp;
          })
      )
    );
    return;
  }

  // Everything else (un-hashed assets, icons, manifest): network-first so a
  // deploy is reflected, with cache as an offline fallback.
  event.respondWith(
    fetch(event.request)
      .then((resp) => {
        if (resp && resp.ok) {
          const copy = resp.clone();
          caches.open(CACHE).then((c) => c.put(event.request, copy)).catch(() => {});
        }
        return resp;
      })
      .catch(() => caches.match(event.request))
  );
});

// Export the pure integrity helper for unit testing under node. Guarded so it's
// a no-op in the service-worker runtime (no CommonJS `module` there).
if (typeof module !== 'undefined' && module.exports) {
  module.exports = { hasWasmMagic };
}
