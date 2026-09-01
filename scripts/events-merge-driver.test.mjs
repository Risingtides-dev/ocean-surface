// Guards the one line that lets two ocean-surface slices share a wave:
// `events.md merge=union` in the root .gitattributes. Every slice appends its
// entry at the END of events.md — the single hunk git cannot resolve unaided —
// and the last wave to land two surface slices before the attribute existed
// paid for it by hand (#174): the rebase raised a real conflict whose two sides
// were two complete entries. The attribute is one line in a file a cleanup
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
// Five claims, because the cheap ones alone prove nothing. The file check reads
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
// The last two are the ones the identity separator bought, and they used to be
// a single check pinning the DEFECT instead. Union keeps a line both sides added
// only once, and while every entry closed with the same bare `___` rule, two
// appended entries ended with the same line: xdiff anchored each insertion
// before the rule they shared, one rule came out for two entries, and the next
// `time:` header landed directly on the previous entry's prose — two entries
// FUSED, cleanly and silently. #181 folded onto #180 twelve minutes after #180
// landed the checker that caught it. A rule now carries its entry's own `HH:MM`
// and the `worktree:` it was written on, so no two appends share a line and
// there is nothing left for union to fold. The three-way append below is that
// property at the width a wave actually lands, and the same-minute pair is the
// reason the identity is not the clock alone.
//
// The fold has NOT left the repo, so nothing here was deleted lightly: the 289
// rules already in events.md are bare and identical, and an append-only ledger
// keeps them forever. Detection and repair of that shape moved to
// scripts/check-ledger.test.mjs, which pins it against hand-written text rather
// than against a merge that no longer produces it.
//
// What it does NOT cover: everything here happens in a scratch repo, never read
// out of this repo's own events.md. scripts/check-ledger.mjs is the half that
// reads the real file, and the `ledger` CI job runs it on PRs and on pushes to
// main -- not on a push to a feature branch, which runs nothing at all. When it
// first ran, 22 of 269 entries here had already folded open.
//
// Nor does it cover the blank line between a rule and the next header. That one
// is settled rather than missing: a blank line cannot be given an identity, so
// it is the part of the format union will always be able to fold, and AGENTS.md
// rules it cosmetic. The three-way merge below loses it at EVERY join, which is
// why the position check accepts a header sitting flush under a rule — what it
// refuses is a header sitting on PROSE, which is the entry boundary itself.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

import { openEntries } from './check-ledger.mjs';

const repo = resolve(import.meta.dirname, '..');
const attributesFile = join(repo, '.gitattributes');
// 81 underscores, the width every rule in this ledger uses.
const BAR = '_'.repeat(81);
// A rule carrying its entry's identity, which is the whole point: two entries
// that end with the same line are what union cannot keep apart.
const rule = (time, worktree) => `${BAR} ${time} ${worktree}`;

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
  'worktree:  main',
  'type:      [plan]',
  'area:      [infra]',
  '',
  'The entry both branches were cut from.',
  rule('09:00', 'main'),
  '',
].join('\n');

// `time` is a parameter because the two things that can separate two entries
// are separated here too: the wave case gives each slice its own minute, and
// the same-minute case takes it away and leaves only the worktree.
const entry = (slice, time) =>
  [
    `time:      [${time}] [08-31-26]`,
    'agent:     [claude] [opus 5]',
    `worktree:  loop/slice-${slice.toLowerCase()}`,
    'type:      [feature-request]',
    'area:      [infra]',
    '',
    `Appended at EOF by slice ${slice}, concurrently with the other slices.`,
    rule(time, `loop/slice-${slice.toLowerCase()}`),
    '',
  ].join('\n');

// One base commit, then one branch per slice each appending a different entry
// at EOF — the exact shape of a wave landing. The first branch then merges the
// rest in one at a time, the way the land phase rebases each branch onto the
// one before it.
function parallelAppendMerge(attributes, { slices = ['A', 'B'], sameMinute = false } = {}) {
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

    for (const [n, slice] of slices.entries()) {
      const time = sameMinute ? '10:01' : `10:0${n + 1}`;
      scratchGit(dir, 'checkout', '-b', `slice-${slice}`, 'base');
      writeFileSync(join(dir, 'events.md'), `${BASE_LEDGER}\n${entry(slice, time)}`);
      scratchGit(dir, 'commit', '-am', `slice ${slice} appends its entry`);
    }

    const [onto, ...rest] = slices;
    scratchGit(dir, 'checkout', `slice-${onto}`);
    let conflict = null;
    for (const slice of rest) {
      try {
        scratchGit(dir, 'merge', '--no-edit', `slice-${slice}`);
      } catch (err) {
        conflict = `${err.stdout ?? ''}${err.stderr ?? ''}`.trim() || `merge exited ${err.status}`;
        break;
      }
    }
    return { conflict, ledger: readFileSync(join(dir, 'events.md'), 'utf8') };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

const headerCount = (ledger) => (ledger.match(/^time:/gm) ?? []).length;
// Matches both rule forms, because the ledger holds both: bare for every entry
// written before the identity convention, identity-bearing after it. Bare-only
// here and an identity rule stops being counted, which is how this file would
// go on reporting a fold it no longer sees.
const rules = (ledger) => ledger.match(/^_{5,}(?:[ \t].*)?$/gm) ?? [];

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
const withDriver = parallelAppendMerge(attributes);
assert.equal(withDriver.conflict, null, `two concurrent ledger appends must merge clean:\n${withDriver.conflict}`);
assert.match(withDriver.ledger, /slice A/, "slice A's entry was dropped by the merge");
assert.match(withDriver.ledger, /slice B/, "slice B's entry was dropped by the merge");
assert.match(withDriver.ledger, /The entry both branches were cut from\./, 'the base entry was dropped by the merge');
for (const marker of ['<<<<<<<', '=======', '>>>>>>>']) {
  assert.ok(!withDriver.ledger.includes(marker), `merged ledger still carries a \`${marker}\` conflict marker`);
}

// 3. The control: the same merge without the attribute has to conflict.
const withoutDriver = parallelAppendMerge(null);
assert.ok(
  withoutDriver.conflict,
  'the control merge succeeded, so this file proves nothing: git resolved two divergent EOF appends on its own and the driver is not what keeps waves clean',
);
assert.ok(
  withoutDriver.ledger.includes('<<<<<<<'),
  `expected git's own conflict markers in the control merge, got:\n${withoutDriver.ledger}`,
);

// 4. What the identity separator buys, at the width a wave actually lands:
// three slices, three EOF appends, one branch merging the other two in turn.
// Under the bare rule this came out with three rules for four entries.
const wave = parallelAppendMerge(attributes, { slices: ['A', 'B', 'C'] });
assert.equal(wave.conflict, null, `three concurrent ledger appends must merge clean:\n${wave.conflict}`);
assert.equal(headerCount(wave.ledger), 4, `expected the base entry and all three appends, got:\n${wave.ledger}`);
assert.equal(
  rules(wave.ledger).length,
  headerCount(wave.ledger),
  `every entry must still carry its own rule. Got ${rules(wave.ledger).length} rules for ${headerCount(wave.ledger)} entries — union folded two entries that ended with the same line, so the separator has lost its per-entry identity:\n${wave.ledger}`,
);
// Counting rules is not enough on its own, because the damage is positional: an
// arriving header may sit under a rule or under a blank line, never on prose.
const waveLines = wave.ledger.split('\n');
for (const [n, line] of waveLines.entries()) {
  if (n === 0 || !line.startsWith('time:')) continue;
  const above = waveLines[n - 1];
  assert.ok(
    above.trim() === '' || /^_{5,}/.test(above),
    `line ${n + 1} is a \`time:\` header sitting under \`${above}\` — the entry above it fused into this one:\n${wave.ledger}`,
  );
}
// And the merged text has to satisfy the checker the `ledger` job runs on the
// real file, which is the verdict the loop diffs either side of a rebase.
assert.deepEqual(
  openEntries(wave.ledger).map((open) => open.header),
  [],
  `the merged ledger must come back clean from check-ledger's own reading:\n${wave.ledger}`,
);

// 5. Why the identity is not the clock alone. Two slices in one wave land in
// the same minute often enough to have done it, and then the worktree is the
// whole of what tells the two rules apart. Counted off the merged text before
// anything is searched for in it: drop the worktree from the convention and
// both slices close with the identical `___ 10:01`, union folds them into one
// line, and the two `includes` calls below would then BOTH match it.
const minute = parallelAppendMerge(attributes, { sameMinute: true });
assert.equal(minute.conflict, null, `two same-minute ledger appends must merge clean:\n${minute.conflict}`);
const merged = rules(minute.ledger);
assert.equal(
  merged.length,
  3,
  `expected the base rule and one per slice, got ${merged.length} — with the minute shared there is nothing but the worktree left to tell these two rules apart, and union folded the two that matched:\n${minute.ledger}`,
);
const [tailA, tailB] = merged.slice(1);
assert.notEqual(
  tailA,
  tailB,
  `both slices closed with \`${tailA}\`. Two entries that end with the same line are exactly what union cannot keep apart, so a separator these two share is one the next merge eats:\n${minute.ledger}`,
);
for (const slice of ['a', 'b']) {
  assert.ok(
    minute.ledger.includes(rule('10:01', `loop/slice-${slice}`)),
    `slice ${slice.toUpperCase()}'s rule did not survive the merge. Both entries were written in the same minute, so \`HH:MM\` alone cannot tell them apart and the worktree is the whole of the identity here:\n${minute.ledger}`,
  );
}
// The other half of what this fixture merges, counted so a known limit cannot
// quietly get worse while the assertions above stay green. Identity saves an
// entry's TAIL and not its HEAD: two same-minute appends OPEN with an identical
// `time:` and `agent:` pair, union folds those the way it once folded the rule,
// and slice B arrives decapitated — its rule intact, its header gone, its
// remaining fields hanging under slice A's rule. check-ledger reads the survivor
// as one closed entry and exits 0, so nothing else in this repo sees it.
// AGENTS.md rules the fix belongs to the entry SCHEMA rather than the separator;
// this holds the cost at exactly one header. Three would mean git stopped
// folding the head — the good direction, and the line to relax, not widen.
assert.equal(
  headerCount(minute.ledger),
  2,
  `expected the same-minute head fold to cost exactly one \`time:\` header of three entries, got ${headerCount(minute.ledger)}:\n${minute.ledger}`,
);

console.log(
  'ALL PASS: events.md union merge driver — the rule, git agreeing, a clean append/append merge, a conflicting control, a three-way wave that keeps every rule and fuses nothing, and a same-minute pair whose two RULES the worktree alone keeps apart, at the cost of one folded header (25 assertions)',
);
