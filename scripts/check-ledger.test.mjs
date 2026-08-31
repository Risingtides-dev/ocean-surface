// Covers scripts/check-ledger.mjs. ocean-bedrock exercises the same logic from
// the other end — it merges two branches with the real union driver and asserts
// the checker sees the fold — and this repo does that too, in
// scripts/events-merge-driver.test.mjs, but only against a scratch repo it
// builds itself. These are pure over the text instead, plus the exit contract,
// which is the part CI actually depends on. Ported from ocean-os's copy of this
// file, which is where the port of the script itself came through.
//
// Run: node --test scripts/check-ledger.test.mjs
import assert from 'node:assert/strict';
import test from 'node:test';
import os from 'node:os';
import path from 'node:path';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';

import { closeEntries, main, openEntries, readEntries } from './check-ledger.mjs';

const RULE = '_'.repeat(81);

function entry(time, prose) {
  return [`time:      [${time}] [01-01-26]`, 'agent:     [test]', 'worktree:  main', '', prose];
}

// A healthy ledger: every entry closed by its rule, one blank line between the
// rule and the next header.
const CLEAN = [...entry('10:00', 'First.'), RULE, '', ...entry('11:00', 'Second.'), RULE, ''].join('\n');

// What the union driver actually leaves behind: the rule the two entries shared
// is gone, so the second header sits directly under the first entry's prose.
const FOLDED = [...entry('10:00', 'First.'), ...entry('11:00', 'Second.'), RULE, ''].join('\n');

// Every line of `before` still present, in order, in `after`. The repair is an
// insert-only edit to an append-only file, so proving it deletes nothing
// matters more than proving what it inserted — and it is what makes a mid-file
// repair safe to land under `merge=union`.
function isSubsequence(before, after) {
  let i = 0;
  for (const line of after) if (i < before.length && line === before[i]) i++;
  return i === before.length;
}

async function tempLedger(text) {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'check-ledger-'));
  const file = path.join(dir, 'events.md');
  await writeFile(file, text);
  return file;
}

test('readEntries splits on time: headers and reports 1-based gutter lines', () => {
  const entries = readEntries(CLEAN);
  assert.equal(entries.length, 2);
  assert.equal(entries[0].line, 1);
  assert.equal(entries[0].runsInto, 8);
  assert.equal(entries[1].runsInto, null, 'the last entry runs to the end of the file');
  assert.ok(entries.every((e) => e.closed));
});

test('a fold leaves the first entry open and names where the next one starts', () => {
  const open = openEntries(FOLDED);
  assert.equal(open.length, 1);
  assert.equal(open[0].line, 1);
  assert.equal(open[0].runsInto, 6, 'the second header lands directly under the first prose');
});

test('an entry closed anywhere in its body counts as closed', () => {
  // 11 entries in this ledger carry a second rule inside their prose, and those
  // 11 are exactly why subtracting rules from entries reports half the open
  // count. The check is "a rule before the next header", never "a rule on the
  // last line". The first entry's rule is followed by more prose, so a
  // last-line reading would call it open — which is what makes this fixture
  // discriminate.
  const quoted = [...entry('10:00', 'First.'), RULE, 'Quoting the rule above, not closing on it.', ''];
  const ledger = [...quoted, ...entry('11:00', 'Second.'), RULE].join('\n');
  assert.equal(openEntries(ledger).length, 0);
  assert.notEqual(quoted.filter((line) => line.trim()).pop(), RULE, 'the fixture must not end on a rule');
});

test('closeEntries repairs the fold without deleting a line, and the rerun is clean', () => {
  const { text, closed } = closeEntries(FOLDED);
  assert.equal(closed.length, 1);
  assert.equal(openEntries(text).length, 0);

  const before = FOLDED.split('\n');
  const after = text.split('\n');
  assert.ok(after.length > before.length, 'the repair inserts');
  assert.ok(isSubsequence(before, after), 'the repair deletes nothing');
  assert.ok(after.includes(RULE));
});

test('the repair copies the rule width the file already uses', () => {
  const narrow = '_'.repeat(73);
  const folded = [...entry('10:00', 'First.'), ...entry('11:00', 'Second.'), narrow, ''].join('\n');
  const repaired = closeEntries(folded).text.split('\n');
  assert.ok(repaired.includes(narrow));
  assert.ok(!repaired.includes(RULE), 'never the default width when the file has one of its own');
});

// The two shapes this ledger's 22 open entries actually came in. 18 of them
// abut — the fold FOLDED already covers — but 4 keep a blank line where the
// rule should be, which is an entry written without one rather than a merge
// artefact. The repair has to close both, and it must not double the blank.
test('an entry left open with its blank line intact is closed without doubling it', () => {
  const unruled = [...entry('10:00', 'First.'), '', ...entry('11:00', 'Second.'), RULE, ''].join('\n');
  const repaired = closeEntries(unruled).text.split('\n');
  assert.equal(openEntries(repaired.join('\n')).length, 0);
  assert.equal(repaired[5], RULE, 'the rule lands on the blank line the entry already had after its prose');
  assert.equal(repaired[6], '', 'and exactly one blank still separates it from the next header');
  assert.match(repaired[7], /^time:/, 'and the next header follows that single blank');
});

test('a ledger whose every entry is open is fully repaired in one pass', () => {
  const open = [...entry('10:00', 'First.'), '', ...entry('11:00', 'Second.')].join('\n');
  assert.equal(openEntries(open).length, 2);
  assert.equal(openEntries(closeEntries(open).text).length, 0);
});

test('main exits 0 on a clean ledger and 1 on an open one', async () => {
  assert.equal(await main([await tempLedger(CLEAN)]), 0);
  assert.equal(await main([await tempLedger(FOLDED)]), 1);
});

test('main --fix closes the ledger in place and exits 0', async () => {
  const file = await tempLedger(FOLDED);
  assert.equal(await main([file, '--fix']), 0);
  assert.equal(openEntries(await readFile(file, 'utf8')).length, 0);
  assert.equal(await main([file]), 0, 'and the plain rerun now agrees');
});

test('main exits 2 when the check could not run at all', async () => {
  assert.equal(await main([await tempLedger('no entries here, just prose\n')]), 2, 'a ledger that lost its contents');
  assert.equal(await main([path.join(os.tmpdir(), 'check-ledger-does-not-exist', 'events.md')]), 2, 'an unreadable path');
  assert.equal(await main(['one.md', 'two.md']), 2, 'more paths than it can check');
});

test('--help exits 0 and names the exit contract', async () => {
  assert.equal(await main(['--help']), 0);
});
