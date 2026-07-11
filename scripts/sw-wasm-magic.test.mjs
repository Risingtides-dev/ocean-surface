// Unit test for the service-worker wasm-integrity helper (OCEAN-122).
//
// The SW serves hashed wasm cache-first for speed, but validates the cached
// bytes against the WebAssembly magic word ("\0asm") before serving so a
// once-pinned corrupt (e.g. all-zero) wasm can't brick the page across reloads.
// This test exercises the pure `hasWasmMagic` helper exported from public/sw.js.
//
// Run:  node scripts/sw-wasm-magic.test.mjs
//
// We require() the SW source under a minimal SW-runtime shim so its
// `self.addEventListener(...)` registrations don't throw under node; only the
// pure helper is exercised.

import assert from 'node:assert';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const here = dirname(fileURLToPath(import.meta.url));
const swPath = join(here, '..', 'public', 'sw.js');

// Minimal globals so requiring sw.js is a no-op beyond defining the helpers.
globalThis.self = {
  addEventListener() {},
  location: { origin: 'https://example.test' },
  skipWaiting() {},
  clients: { claim() {}, matchAll: async () => [] },
};
globalThis.caches = {
  open: async () => ({}),
  keys: async () => [],
  match: async () => undefined,
  delete: async () => false,
};

const { hasWasmMagic, isLoopbackHostname } = require(swPath);

const good = new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]).buffer;
assert.strictEqual(hasWasmMagic(good), true, 'valid wasm magic should pass');

// All-zero bytes — the OCEAN-121/122 failure mode (a failed wasm-opt ships zeros).
assert.strictEqual(hasWasmMagic(new Uint8Array(64).buffer), false, 'all-zero buffer should fail');

assert.strictEqual(hasWasmMagic(new Uint8Array([0x00, 0x61, 0x73]).buffer), false, 'short buffer should fail');
assert.strictEqual(hasWasmMagic(new Uint8Array(0).buffer), false, 'empty buffer should fail');
assert.strictEqual(hasWasmMagic(null), false, 'null should fail');
assert.strictEqual(hasWasmMagic(undefined), false, 'undefined should fail');

// e.g. an HTML error page mistakenly cached under a .wasm URL ("<!doc").
assert.strictEqual(hasWasmMagic(new Uint8Array([0x3c, 0x21, 0x64, 0x6f, 0x63]).buffer), false, 'non-wasm bytes should fail');

// Accepts a TypedArray view directly, not only an ArrayBuffer.
assert.strictEqual(hasWasmMagic(new Uint8Array([0x00, 0x61, 0x73, 0x6d])), true, 'Uint8Array input should pass');

// Loopback is a development surface: its worker must retire instead of
// intercepting navigations and pinning the emergency reconnect shell.
assert.strictEqual(isLoopbackHostname('localhost'), true, 'localhost should retire the worker');
assert.strictEqual(isLoopbackHostname('127.0.0.1'), true, 'IPv4 loopback should retire the worker');
assert.strictEqual(isLoopbackHostname('::1'), true, 'IPv6 loopback should retire the worker');
assert.strictEqual(isLoopbackHostname('[::1]'), true, 'bracketed IPv6 loopback should retire the worker');
assert.strictEqual(isLoopbackHostname('ocean.agentsworld.org'), false, 'deployed PWA should keep the worker');

console.log('ALL PASS: service-worker integrity + loopback policy (13 assertions)');
