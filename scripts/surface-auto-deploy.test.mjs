import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, readlinkSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const repo = resolve(import.meta.dirname, '..');
const script = join(repo, 'deploy', 'ocean-surface-auto-deploy.sh');
const proxyPlist = readFileSync(join(repo, 'deploy', 'dev.risingtides.ocean-surface-proxy.plist'), 'utf8');
const deployPlist = readFileSync(join(repo, 'deploy', 'dev.risingtides.ocean-surface-auto-deploy.plist'), 'utf8');
const root = mkdtempSync(join(tmpdir(), 'ocean-surface-promote-'));

function validBundle(path) {
  mkdirSync(path, { recursive: true });
  writeFileSync(join(path, 'index.html'), '<script type="module" src="/ocean.js"></script>');
  writeFileSync(join(path, 'ocean-surface-ui_bg.wasm'), Buffer.from([0x00, 0x61, 0x73, 0x6d, 0x01]));
  writeFileSync(join(path, 'ocean-surface-ui.js'), '// wasm-bindgen glue');
}

try {
  const state = join(root, 'state');
  const staged = join(root, 'staged');
  validBundle(staged);

  execFileSync(script, ['--promote', staged, 'abc123'], {
    env: { ...process.env, OCEAN_SURFACE_STATE_DIR: state, OCEAN_SURFACE_NO_RESTART: '1' },
    stdio: 'pipe',
  });

  assert.equal(readlinkSync(join(state, 'current')), 'releases/abc123');
  assert.equal(readFileSync(join(state, 'deployed-rev'), 'utf8').trim(), 'abc123');
  assert.equal(readFileSync(join(state, 'releases', 'abc123', 'ocean-surface-ui_bg.wasm')).subarray(0, 4).toString('hex'), '0061736d');

  const staged2 = join(root, 'staged2');
  validBundle(staged2);
  execFileSync(script, ['--promote', staged2, 'def456'], {
    env: { ...process.env, OCEAN_SURFACE_STATE_DIR: state, OCEAN_SURFACE_NO_RESTART: '1' },
    stdio: 'pipe',
  });
  assert.equal(readlinkSync(join(state, 'current')), 'releases/def456', 'a second promotion must replace the current symlink');
  assert.equal(readFileSync(join(state, 'deployed-rev'), 'utf8').trim(), 'def456');
  assert.deepEqual(readdirSync(join(state, 'releases', 'abc123')).sort(), ['.deploy-sha', 'index.html', 'ocean-surface-ui.js', 'ocean-surface-ui_bg.wasm']);

  const bad = join(root, 'bad');
  mkdirSync(bad, { recursive: true });
  writeFileSync(join(bad, 'index.html'), '<h1>broken</h1>');
  const failed = spawnSync(script, ['--promote', bad, 'broken'], {
    env: { ...process.env, OCEAN_SURFACE_STATE_DIR: state, OCEAN_SURFACE_NO_RESTART: '1' },
    encoding: 'utf8',
  });

  assert.notEqual(failed.status, 0, 'invalid bundle promotion must fail');
  assert.equal(readlinkSync(join(state, 'current')), 'releases/def456', 'failed promotion must preserve current release');
  assert.equal(readFileSync(join(state, 'deployed-rev'), 'utf8').trim(), 'def456', 'failed promotion must preserve marker');

  const noOpState = join(root, 'noop-state');
  const mainRevision = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: repo, encoding: 'utf8' }).trim();
  execFileSync(script, ['--promote', staged, mainRevision], {
    env: { ...process.env, OCEAN_SURFACE_STATE_DIR: noOpState, OCEAN_SURFACE_NO_RESTART: '1' },
    stdio: 'pipe',
  });
  mkdirSync(join(noOpState, 'auto-deploy.lock'));
  writeFileSync(join(noOpState, 'auto-deploy.lock', 'pid'), '99999999\n');
  const noOp = spawnSync(script, [], {
    env: {
      ...process.env,
      OCEAN_SURFACE_REPO: repo,
      OCEAN_SURFACE_STATE_DIR: noOpState,
      OCEAN_SURFACE_NO_RESTART: '1',
      OCEAN_SURFACE_TARGET_REV: mainRevision,
    },
    encoding: 'utf8',
  });
  assert.equal(noOp.status, 0, noOp.stderr);
  assert.match(noOp.stdout, new RegExp(`CURRENT: ${mainRevision}`));
  assert.equal(existsSync(join(noOpState, 'auto-deploy.lock')), false, 'a stale deployment lock must be reclaimed');

  assert.ok(proxyPlist.includes('/Users/risingtidesdev/.config/ocean-surface/bin/ocean-surface-proxy.sh'));
  assert.ok(deployPlist.includes('/Users/risingtidesdev/.config/ocean-surface/bin/ocean-surface-auto-deploy.sh'));
  assert.ok(!`${proxyPlist}\n${deployPlist}`.includes('/dev/ocean-surface/deploy/ocean-surface-'));

  console.log('ALL PASS: surface atomic deployment promotion (16 assertions)');
} finally {
  rmSync(root, { recursive: true, force: true });
}
