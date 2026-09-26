#!/usr/bin/env node
// Vendors ocean-os's session, voice, component and observatory wire contracts
// into this repo, or checks that the vendored copies still match them.
//
// WHAT THEY ARE. ocean-os publishes one JSON artifact per wire under
// `docs/contracts/` and holds each equal to the code that serves it
// (`session_wire_contract_matches_the_daemon`, `voice_wire_contract_matches_the_daemon`,
// `component_wire_contract_matches_the_runtime`,
// `observatory_wire_contract_matches_the_daemon`). This repo keeps a
// byte-for-byte copy of each at
// crates/ocean-surface-ui/tests/fixtures/ocean-os-<name>/<name>.json, and the
// `*_wire_contract_tests.rs` modules include_str! them and prove the Surface
// decoders, encoders and proxy routes stay inside every value in them.
//
// This is the same scheme scripts/vendor-ocean-os-room-wire.mjs (PR #225) uses
// for room-wire.json: same fixture layout, same `vendored-from.json`
// provenance (repo, path, commit, sha256), same flags and exit codes. It takes
// a contract name because there are four of them.
//
// WHY A COPY AND NOT A PATH. The two repos build and ship independently, and a
// test that read a sibling checkout would pass or fail on whatever branch that
// checkout happened to be on. The copy is what this surface was built against;
// `vendored-from.json` beside it names the ocean-os commit it came from and the
// sha256 of the bytes, so a hand edit of the copy is caught too.
//
// THE CHECK. `--check` compares each vendored copy with its source and exits 1
// when one differs. Both repos are public, so CI can hand it a copy fetched
// from ocean-os `main` over HTTPS (`--source`) with no cross-repo credential.
//
// No dependencies: node: builtins only, like every other script in this dir.
import path from 'node:path';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

const scriptPath = fileURLToPath(import.meta.url);
const repoRoot = path.resolve(path.dirname(scriptPath), '..');

// room-wire is #225's (scripts/vendor-ocean-os-room-wire.mjs), not this one's.
const CONTRACTS = ['session-wire', 'voice-wire', 'component-wire', 'observatory-wire'];
const REFRESH = 'node scripts/vendor-ocean-os-wire-contracts.mjs';

const contractPath = (name) => `docs/contracts/${name}.json`;
const vendorDir = (name) =>
  path.join(repoRoot, `crates/ocean-surface-ui/tests/fixtures/ocean-os-${name}`);
const vendoredFile = (name) => path.join(vendorDir(name), `${name}.json`);
const provenanceFile = (name) => path.join(vendorDir(name), 'vendored-from.json');

const HELP = `Usage:
  node scripts/vendor-ocean-os-wire-contracts.mjs            refresh all four copies
  node scripts/vendor-ocean-os-wire-contracts.mjs --check    exit 1 if any differs

Options:
  --contract <name> only this contract (repeatable): ${CONTRACTS.join(', ')}
  --ref <ref>       read the contracts at a git ref of the ocean-os checkout
                    (e.g. origin/main) instead of its working tree
  --source <file>   read the contract from this file instead of a checkout
                    (CI hands it the copy fetched from ocean-os main); needs
                    exactly one --contract, and a refresh records no commit
  -h, --help        this text

The ocean-os checkout is $OCEAN_OS_DIR, default ../ocean-os beside this repo.
Exit 0 when refreshed or in sync, 1 when --check finds a difference, 2 when
the check could not run (no checkout, unreadable file, invalid JSON).`;

class UsageError extends Error {}

function parseArgs(argv) {
  const opts = { check: false, ref: null, source: null, contracts: [] };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--check') opts.check = true;
    else if (arg === '--ref' || arg === '--source' || arg === '--contract') {
      const value = argv[i + 1];
      if (!value) throw new UsageError(`${arg} needs a value`);
      if (arg === '--contract') {
        if (!CONTRACTS.includes(value)) {
          throw new UsageError(`unknown contract: ${value} (one of ${CONTRACTS.join(', ')})`);
        }
        opts.contracts.push(value);
      } else opts[arg.slice(2)] = value;
      i += 1;
    } else if (arg === '-h' || arg === '--help') opts.help = true;
    else throw new UsageError(`unknown argument: ${arg}`);
  }
  if (opts.contracts.length === 0) opts.contracts = [...CONTRACTS];
  if (opts.source && opts.contracts.length !== 1) {
    throw new UsageError('--source reads one file, so it needs exactly one --contract');
  }
  return opts;
}

function git(dir, args) {
  return execFileSync('git', ['-C', dir, ...args], {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  });
}

// Returns { text, commit, from } — commit is null when the source is a bare file.
async function readSource(opts, name) {
  if (opts.source) {
    return { text: await readFile(opts.source, 'utf8'), commit: null, from: opts.source };
  }
  const osDir = path.resolve(repoRoot, process.env.OCEAN_OS_DIR || '../ocean-os');
  if (opts.ref) {
    const commit = git(osDir, ['rev-parse', `${opts.ref}^{commit}`]).trim();
    const text = git(osDir, ['show', `${commit}:${contractPath(name)}`]);
    return { text, commit, from: `${osDir} @ ${opts.ref}` };
  }
  const text = await readFile(path.join(osDir, contractPath(name)), 'utf8');
  const commit = git(osDir, ['rev-parse', 'HEAD']).trim();
  const dirty = git(osDir, ['status', '--porcelain', '--', contractPath(name)]).trim() !== '';
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

async function checkOne(opts, name) {
  const source = await readSource(opts, name);
  JSON.parse(source.text); // an unparseable contract is exit 2, not a verdict
  const vendored = await readFile(vendoredFile(name), 'utf8');
  const provenance = JSON.parse(await readFile(provenanceFile(name), 'utf8'));
  if (provenance.sha256 !== sha256(vendored)) {
    console.error(
      `vendored ${name}.json was edited by hand: its sha256 no longer matches ` +
        `vendored-from.json. Run \`${REFRESH} --contract ${name}\` to re-vendor it from ocean-os.`,
    );
    return 1;
  }
  if (vendored !== source.text) {
    console.error(
      `The vendored ocean-os ${name} contract differs from ${source.from}:\n` +
        `${describeDrift(vendored, source.text).join('\n')}\n` +
        `Run \`${REFRESH} --contract ${name}\` (OCEAN_OS_DIR=<ocean-os checkout>, --ref origin/main) ` +
        `and make \`cargo test -p ocean-surface-ui wire_contract\` pass against the new copy.`,
    );
    return 1;
  }
  console.log(`${name}.json is in sync with ${source.from}`);
  return 0;
}

async function vendorOne(opts, name) {
  const source = await readSource(opts, name);
  JSON.parse(source.text);
  await mkdir(vendorDir(name), { recursive: true });
  await writeFile(vendoredFile(name), source.text);
  const provenance = {
    repo: 'Risingtides-dev/ocean-os',
    path: contractPath(name),
    commit: source.commit,
    sha256: sha256(source.text),
  };
  await writeFile(provenanceFile(name), `${JSON.stringify(provenance, null, 2)}\n`);
  console.log(
    `vendored ${contractPath(name)} from ${source.from}` +
      (source.commit ? ` (${source.commit})` : ''),
  );
  return 0;
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.help) {
    console.log(HELP);
    return 0;
  }
  let code = 0;
  for (const name of opts.contracts) {
    const result = opts.check ? await checkOne(opts, name) : await vendorOne(opts, name);
    code = Math.max(code, result);
  }
  return code;
}

main().then(
  (code) => process.exit(code),
  (err) => {
    if (err instanceof UsageError) console.error(`${err.message}\n\n${HELP}`);
    else console.error(`vendor-ocean-os-wire-contracts: ${err.message}`);
    process.exit(2);
  },
);
