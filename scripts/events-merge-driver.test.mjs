// Guards the one line that lets two ocean-surface slices share a wave:
// `events.md merge=union` in the root .gitattributes. Every slice appends its
// entry at the END of events.md — the single hunk git cannot resolve unaided —
// and the last wave to land two surface slices paid for it by hand (#174): the
// rebase raised a real conflict whose two sides were two complete entries
// sharing one trailing rule. The attribute is one line in a file a cleanup
// could plausibly delete, and its failure mode reads as loop breakage rather
// than as a missing attribute, so it gets diagnosed slowly.
//
// Run:  node scripts/events-merge-driver.test.mjs
//
// Also a CI step: the `guards` job in .github/workflows/ci.yml runs it on
// every PR and on every push to main. It was hand-run at first, which for an
// attribute nobody thinks about until a rebase raises a conflict is the same
// as unrun — the delete and the run are never the same sitting.
//
// Four claims, because the cheap ones alone prove nothing. The file check reads
// the rule out of .gitattributes itself; `git check-attr` is asked second and
// cannot replace it, because git falls back to the copy of .gitattributes in
// the INDEX when the working-tree file is gone — measured here on git 2.50:
// delete the file and check-attr still answers `union`, and only once the
// deletion is staged does it answer `unspecified`. The behaviour check merges
// two divergent appends in a scratch repo and demands a clean result, so an
// attribute that is named but doing nothing fails it. The control runs the SAME
// merge with no .gitattributes and demands a CONFLICT, without which a green
// behaviour check would only be showing that git found the merge trivial.
//
// The last one pins a DEFECT, not a contract. Union keeps a line both sides
// added only once, and two appended entries share both a blank line and the
// trailing rule, so each extra parallel append folds one such pair away and the
// next entry's `time:` header lands directly on the previous entry's prose: two
// entries FUSE, cleanly and silently. If a per-entry-unique separator ever
// lands, this check goes red — that is the signal to delete it along with the
// warning in AGENTS.md, not to loosen it.
//
// What it does NOT cover: the fold is reproduced in a scratch repo, never read
// out of this repo's own events.md. scripts/check-ledger.mjs is the half that
// reads the real file, and the `ledger` CI job runs it on PRs and on pushes to
// main -- not on a push to a feature branch, which runs nothing at all. When it
// first ran, 22 of 269 entries here had already folded open.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const repo = resolve(import.meta.dirname, '..');
const attributesFile = join(repo, '.gitattributes');
const SEPARATOR = '_'.repeat(81);

const git = (cwd, ...args) =>
  execFileSync('git', args, { cwd, encoding: 'utf8', stdio: 'pipe', timeout: 60_000 });

// Identity and hook settings ride as `-c` flags rather than environment: a
// machine with no global user.email, or one whose core.hooksPath points at real
// hooks, would fail the scratch commit for reasons that have nothing to do with
// the merge driver — and exporting GIT_AUTHOR_* would mutate this process's own
// env, which no test here is allowed to do.
const scratchGit = (cwd, ...args) =>
  git(
    cwd,
    '-c', 'user.name=merge driver test',
    '-c', 'user.email=merge-driver-test@invalid',
    '-c', 'commit.gpgsign=false',
    '-c', 'core.hooksPath=hooks-that-do-not-exist',
    ...args,
  );

// A checkout that cannot answer the question at all (an exported tarball has no
// .git) is a loud skip; one that can answer has to answer correctly.
function missingPrecondition() {
  try {
    git(repo, 'rev-parse', '--is-inside-work-tree');
    return null;
  } catch (err) {
    return err.code === 'ENOENT'
      ? 'git is not on PATH'
      : `${repo} is not a git work tree (an exported tarball has no .git)`;
  }
}

const BASE_LEDGER = [
  'time:      [09:00] [08-31-26]',
  'agent:     [claude] [opus 5]',
  'type:      [plan]',
  'area:      [infra]',
  '',
  'The entry both branches were cut from.',
  SEPARATOR,
  '',
].join('\n');

const entry = (slice) =>
  [
    `time:      [10:0${slice === 'A' ? 1 : 2}] [08-31-26]`,
    'agent:     [claude] [opus 5]',
    `worktree:  loop/slice-${slice.toLowerCase()}`,
    'type:      [feature-request]',
    'area:      [infra]',
    '',
    `Appended at EOF by slice ${slice}, concurrently with the other slice.`,
    SEPARATOR,
    '',
  ].join('\n');

// One base commit, then two branches each appending a different entry at EOF —
// the exact shape of two slices landing in one wave.
function twoBranchAppendMerge(attributes) {
  const dir = mkdtempSync(join(tmpdir(), 'ocean-surface-union-merge-'));
  try {
    scratchGit(dir, '-c', 'init.defaultBranch=base', 'init', '.');
    if (attributes !== null) {
      // Seeded from the real file's bytes rather than a retyped rule, so a
      // .gitattributes edited to something other than union fails the behaviour
      // check too instead of quietly testing a literal that lives only here.
      writeFileSync(join(dir, '.gitattributes'), attributes);
      scratchGit(dir, 'add', '.gitattributes');
    }
    writeFileSync(join(dir, 'events.md'), BASE_LEDGER);
    scratchGit(dir, 'add', 'events.md');
    scratchGit(dir, 'commit', '-m', 'base ledger');

    for (const slice of ['A', 'B']) {
      scratchGit(dir, 'checkout', '-b', `slice-${slice}`, 'base');
      writeFileSync(join(dir, 'events.md'), `${BASE_LEDGER}\n${entry(slice)}`);
      scratchGit(dir, 'commit', '-am', `slice ${slice} appends its entry`);
    }

    scratchGit(dir, 'checkout', 'slice-A');
    let conflict = null;
    try {
      scratchGit(dir, 'merge', '--no-edit', 'slice-B');
    } catch (err) {
      conflict = `${err.stdout ?? ''}${err.stderr ?? ''}`.trim() || `merge exited ${err.status}`;
    }
    return { conflict, ledger: readFileSync(join(dir, 'events.md'), 'utf8') };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

const headerCount = (ledger) => (ledger.match(/^time:/gm) ?? []).length;
const rules = (ledger) => ledger.match(/^_{5,}$/gm) ?? [];

const missing = missingPrecondition();
if (missing) {
  console.log(`SKIP: events.md merge-driver guard NOT armed — ${missing}`);
  process.exit(0);
}

// 1. The rule is in the repo's own file, and git agrees on what it means.
assert.ok(
  existsSync(attributesFile),
  `${attributesFile} is gone, so the ledger has no merge driver and the next wave to land two surface slices hand-resolves events.md again`,
);
const attributes = readFileSync(attributesFile, 'utf8');
assert.match(
  attributes,
  /^events\.md\s+merge=union$/m,
  '.gitattributes must carry `events.md merge=union` — an entry for some other path, or some other driver, is not this guarantee',
);
assert.equal(
  git(repo, 'check-attr', 'merge', '--', 'events.md').trim(),
  'events.md: merge: union',
  'git does not resolve `merge=union` for events.md, so the rule above is being shadowed or misspelled',
);

// 2. It actually resolves two concurrent EOF appends.
const withDriver = twoBranchAppendMerge(attributes);
assert.equal(withDriver.conflict, null, `two concurrent ledger appends must merge clean:\n${withDriver.conflict}`);
assert.match(withDriver.ledger, /slice A/, "slice A's entry was dropped by the merge");
assert.match(withDriver.ledger, /slice B/, "slice B's entry was dropped by the merge");
assert.match(withDriver.ledger, /The entry both branches were cut from\./, 'the base entry was dropped by the merge');
for (const marker of ['<<<<<<<', '=======', '>>>>>>>']) {
  assert.ok(!withDriver.ledger.includes(marker), `merged ledger still carries a \`${marker}\` conflict marker`);
}

// 3. The control: the same merge without the attribute has to conflict.
const withoutDriver = twoBranchAppendMerge(null);
assert.ok(
  withoutDriver.conflict,
  'the control merge succeeded, so this file proves nothing: git resolved two divergent EOF appends on its own and the driver is not what keeps waves clean',
);
assert.ok(
  withoutDriver.ledger.includes('<<<<<<<'),
  `expected git's own conflict markers in the control merge, got:\n${withoutDriver.ledger}`,
);

// 4. The cost, pinned as the defect it is.
assert.equal(headerCount(withDriver.ledger), 3, `expected the base entry and both appends, got:\n${withDriver.ledger}`);
assert.equal(
  rules(withDriver.ledger).length,
  headerCount(withDriver.ledger) - 1,
  `union drops exactly one rule per extra append. Got ${rules(withDriver.ledger).length} rules for ${headerCount(withDriver.ledger)} entries — if that is now equal, the separator format has been fixed and this check plus the AGENTS.md warning should go`,
);
assert.match(
  withDriver.ledger,
  /Appended at EOF by slice A[^\n]*\ntime:/,
  `the fold is not cosmetic: slice B's header should sit directly on slice A's prose, fusing the two entries. Got:\n${withDriver.ledger}`,
);

console.log('ALL PASS: events.md union merge driver — the rule, git agreeing, a clean append/append merge, a conflicting control, and the fold it costs (15 assertions)');
