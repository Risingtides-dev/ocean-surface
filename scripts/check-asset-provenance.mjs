#!/usr/bin/env node

import { createHash } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const repoRoot = fileURLToPath(new URL('..', import.meta.url));
process.chdir(repoRoot);
const manifestPath = new URL('../docs/asset-provenance.json', import.meta.url);
const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
const assetExtensions = new Set(manifest.asset_extensions);
const allowedStatuses = new Set(['cleared', 'protected_brand', 'project_dual_licensed']);

function fail(message) {
  console.error(`asset-provenance: ${message}`);
  process.exitCode = 1;
}

const result = spawnSync('git', ['ls-files', '-z'], { encoding: 'utf8' });
if (result.status !== 0) {
  console.error(result.stderr);
  process.exit(result.status ?? 1);
}

const trackedAssets = result.stdout
  .split('\0')
  .filter(Boolean)
  .filter((path) => {
    const index = path.lastIndexOf('.');
    return index >= 0 && assetExtensions.has(path.slice(index).toLowerCase());
  })
  .filter((path) => existsSync(path))
  .sort();

const records = manifest.assets;
const manifestPaths = records.map((record) => record.path);
const sortedPaths = [...manifestPaths].sort();
if (JSON.stringify(manifestPaths) !== JSON.stringify(sortedPaths)) {
  fail('records must be sorted by path');
}

const duplicatePaths = manifestPaths.filter((path, index) => manifestPaths.indexOf(path) !== index);
if (duplicatePaths.length > 0) {
  fail(`duplicate records: ${[...new Set(duplicatePaths)].join(', ')}`);
}

const trackedSet = new Set(trackedAssets);
const manifestSet = new Set(manifestPaths);
const missing = trackedAssets.filter((path) => !manifestSet.has(path));
const stale = manifestPaths.filter((path) => !trackedSet.has(path));
if (missing.length > 0) fail(`tracked assets missing from manifest: ${missing.join(', ')}`);
if (stale.length > 0) fail(`manifest records not present in tracked tree: ${stale.join(', ')}`);

for (const record of records) {
  if (!allowedStatuses.has(record.status)) {
    fail(`${record.path}: unsupported status ${record.status}`);
    continue;
  }
  if (!record.category || !record.provenance || !record.license) {
    fail(`${record.path}: category, provenance, and license are required`);
    continue;
  }
  if (!existsSync(record.path)) continue;
  const bytes = readFileSync(record.path);
  const sha256 = createHash('sha256').update(bytes).digest('hex');
  if (record.bytes !== bytes.length) {
    fail(`${record.path}: byte count changed (${record.bytes} -> ${bytes.length})`);
  }
  if (record.sha256 !== sha256) {
    fail(`${record.path}: SHA-256 changed (${record.sha256} -> ${sha256})`);
  }
}

const expectedStatusCounts = {
  cleared: 22,
  protected_brand: 61,
  project_dual_licensed: 30,
};
const actualStatusCounts = records.reduce((counts, record) => {
  counts[record.status] = (counts[record.status] || 0) + 1;
  return counts;
}, {});
for (const [status, expected] of Object.entries(expectedStatusCounts)) {
  if (actualStatusCounts[status] !== expected) {
    fail(`${status}: expected ${expected} records, found ${actualStatusCounts[status] || 0}`);
  }
}
if (Object.keys(actualStatusCounts).length !== Object.keys(expectedStatusCounts).length) {
  fail(`unexpected launch status set: ${Object.keys(actualStatusCounts).sort().join(', ')}`);
}

for (const record of records) {
  if (record.status === 'protected_brand' && !record.license.includes('TRADEMARKS.md')) {
    fail(`${record.path}: protected brand records must point to TRADEMARKS.md`);
  }
  if (record.status === 'project_dual_licensed') {
    if (!record.path.toLowerCase().endsWith('.html')) {
      fail(`${record.path}: only repository-authored HTML artifacts use project_dual_licensed`);
    }
    if (!record.license.includes('MIT OR Apache-2.0') || !record.license.includes('TRADEMARKS.md')) {
      fail(`${record.path}: dual-licensed HTML must record the project license and brand exclusion`);
    }
  }
}

for (const required of [
  'public/fonts/OFL.txt',
  'crates/ocean-gui/assets/icons/ocean-gui/LICENSE-LUCIDE',
]) {
  if (!existsSync(required)) fail(`required third-party license is missing: ${required}`);
}

if (!process.exitCode) {
  console.log(JSON.stringify({
    ok: true,
    trackedAssets: trackedAssets.length,
    statuses: actualStatusCounts,
  }, null, 2));
}
