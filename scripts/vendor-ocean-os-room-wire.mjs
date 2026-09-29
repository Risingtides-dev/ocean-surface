#!/usr/bin/env node
// Vendors ocean-os's room wire contract into this repo, or checks that the
// vendored copy still matches it.
//
// WHAT IT IS. ocean-os publishes `docs/contracts/room-wire.json`: the `/events`
// SSE event names, the access-state / message-kind / participant-kind
// vocabularies, the top-level keys of `/snapshot` and `/transcript`, and the
// one "room not open" answer. A test in ocean-os holds that file equal to the
// daemon's router. This repo keeps a byte-for-byte copy at
// crates/ocean-surface-ui/tests/fixtures/ocean-os-room-wire/room-wire.json and
// `src/room_wire_contract_tests.rs` include_str!s it and proves the Rooms
// decoders cover every value in it. That is Rooms DoD 5.8, daemon half there,
// surface half here.
//
// WHY A COPY AND NOT A PATH. The two repos build and ship independently, and a
// test that read a sibling checkout would pass or fail on whatever branch that
// checkout happened to be on. The copy is what this surface was built against;
// `vendored-from.json` beside it names the ocean-os commit it came from and the
// sha256 of the bytes, so a hand edit of the copy is caught too.
//
// THE CHECK. `--check` compares the vendored copy with the source and exits 1
// when they differ. CI runs it against the file fetched from ocean-os `main`
// over HTTPS (both repos are public, so no cross-repo credential), which turns
// a daemon-side contract change into a red step here that names this script.
//
// No dependencies: node: builtins only, like every other script in this dir.
import path from 'node:path';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

const scriptPath = fileURLToPath(import.meta.url);
const repoRoot = path.resolve(path.dirname(scriptPath), '..');

const CONTRACT_PATH = 'docs/contracts/room-wire.json';
const VENDOR_DIR = path.join(
  repoRoot,
  'crates/ocean-surface-ui/tests/fixtures/ocean-os-room-wire',
);
const VENDORED = path.join(VENDOR_DIR, 'room-wire.json');
const PROVENANCE = path.join(VENDOR_DIR, 'vendored-from.json');
const REFRESH = 'node scripts/vendor-ocean-os-room-wire.mjs';

const HELP = `Usage:
  node scripts/vendor-ocean-os-room-wire.mjs            refresh the vendored copy
  node scripts/vendor-ocean-os-room-wire.mjs --check    exit 1 if it differs

Options:
  --ref <ref>       read the contract at a git ref of the ocean-os checkout
                    (e.g. origin/main) instead of its working tree
  --source <file>   read the contract from this file instead of a checkout
                    (CI hands it the copy fetched from ocean-os main); with
                    --source, a refresh records no commit
  -h, --help        this text

The ocean-os checkout is $OCEAN_OS_DIR, default ../ocean-os beside this repo.
Exit 0 when refreshed or in sync, 1 when --check finds a difference, 2 when
the check could not run (no checkout, unreadable file, invalid JSON).`;

function parseArgs(argv) {
  const opts = { check: false, ref: null, source: null };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--check') opts.check = true;
    else if (arg === '--ref' || arg === '--source') {
      const value = argv[i + 1];
      if (!value) throw new UsageError(`${arg} needs a value`);
      opts[arg.slice(2)] = value;
      i += 1;
    } else if (arg === '-h' || arg === '--help') opts.help = true;
    else throw new UsageError(`unknown argument: ${arg}`);
  }
  return opts;
}

class UsageError extends Error {}

function git(dir, args) {
  return execFileSync('git', ['-C', dir, ...args], {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  });
}

// Returns { text, commit } — commit is null when the source is a bare file.
async function readSource(opts) {
  if (opts.source) {
    return { text: await readFile(opts.source, 'utf8'), commit: null, from: opts.source };
  }
  const osDir = path.resolve(repoRoot, process.env.OCEAN_OS_DIR || '../ocean-os');
  if (opts.ref) {
    const commit = git(osDir, ['rev-parse', `${opts.ref}^{commit}`]).trim();
    const text = git(osDir, ['show', `${commit}:${CONTRACT_PATH}`]);
    return { text, commit, from: `${osDir} @ ${opts.ref}` };
  }
  const text = await readFile(path.join(osDir, CONTRACT_PATH), 'utf8');
  const commit = git(osDir, ['rev-parse', 'HEAD']).trim();
  const dirty = git(osDir, ['status', '--porcelain', '--', CONTRACT_PATH]).trim() !== '';
  return { text, commit: dirty ? `${commit}+dirty` : commit, from: osDir };
}

const sha256 = (text) => createHash('sha256').update(text).digest('hex');

// A readable summary of what moved, per top-level key. Only a hint for the
// reader — the verdict is byte equality.
function describeDrift(vendoredText, sourceText) {
  let a;
  let b;
  try {
    a = JSON.parse(vendoredText);
    b = JSON.parse(sourceText);
  } catch {
    return ['  (one side is not valid JSON)'];
  }
  const lines = [];
  for (const key of new Set([...Object.keys(a), ...Object.keys(b)])) {
    if (JSON.stringify(a[key]) === JSON.stringify(b[key])) continue;
    if (Array.isArray(a[key]) && Array.isArray(b[key])) {
      const added = b[key].filter((v) => !a[key].includes(v));
      const removed = a[key].filter((v) => !b[key].includes(v));
      const parts = [];
      if (added.length) parts.push(`added ${added.join(', ')}`);
      if (removed.length) parts.push(`removed ${removed.join(', ')}`);
      lines.push(`  ${key}: ${parts.join('; ') || 'reordered'}`);
    } else {
      lines.push(`  ${key}: ${JSON.stringify(a[key])} -> ${JSON.stringify(b[key])}`);
    }
  }
  return lines.length ? lines : ['  (whitespace or formatting only)'];
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.help) {
    console.log(HELP);
    return 0;
  }
  const source = await readSource(opts);
  JSON.parse(source.text); // an unparseable contract is exit 2, not a vendor

  if (opts.check) {
    const vendored = await readFile(VENDORED, 'utf8');
    const provenance = JSON.parse(await readFile(PROVENANCE, 'utf8'));
    if (provenance.sha256 !== sha256(vendored)) {
      console.error(
        `vendored room-wire.json was edited by hand: its sha256 no longer matches ` +
          `vendored-from.json. Run \`${REFRESH}\` to re-vendor it from ocean-os.`,
      );
      return 1;
    }
    if (vendored !== source.text) {
      console.error(
        `The vendored ocean-os room wire contract differs from ${source.from}:\n` +
          `${describeDrift(vendored, source.text).join('\n')}\n` +
          `Run \`${REFRESH}\` (OCEAN_OS_DIR=<ocean-os checkout>, --ref origin/main) ` +
          `and make \`cargo test -p ocean-surface-ui room_wire\` pass against the new copy.`,
      );
      return 1;
    }
    console.log(`room-wire.json is in sync with ${source.from}`);
    return 0;
  }

  await mkdir(VENDOR_DIR, { recursive: true });
  await writeFile(VENDORED, source.text);
  const provenance = {
    repo: 'Risingtides-dev/ocean-os',
    path: CONTRACT_PATH,
    commit: source.commit,
    sha256: sha256(source.text),
  };
  await writeFile(PROVENANCE, `${JSON.stringify(provenance, null, 2)}\n`);
  console.log(
    `vendored ${CONTRACT_PATH} from ${source.from}` +
      (source.commit ? ` (${source.commit})` : ''),
  );
  return 0;
}

main().then(
  (code) => process.exit(code),
  (err) => {
    if (err instanceof UsageError) console.error(`${err.message}\n\n${HELP}`);
    else console.error(`vendor-ocean-os-room-wire: ${err.message}`);
    process.exit(2);
  },
);
