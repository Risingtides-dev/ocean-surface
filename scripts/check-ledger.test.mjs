// Covers scripts/check-ledger.mjs. ocean-bedrock exercises the same logic from
// the other end — it merges branches with the real union driver and asserts the
// checker's verdict on the result — and this repo does that too, in
// scripts/events-merge-driver.test.mjs, but only against a scratch repo it
// builds itself. These are pure over the text instead, plus the exit contract,
// which is the part CI actually depends on. Ported from ocean-os's copy of this
// file, which is where the port of the script itself came through.
//
// The identity separator is the reason half of these exist. A rule now carries
// its entry's own `HH:MM` and the `worktree:` it was written on, so no two
// entries end with the same line and union has nothing to fold; the fixtures
// below cover an entry with a worktree, one without, one with neither, and two
// sharing a minute, because each of those four takes a different branch through
// `entryIdentity` and the first port to skip one shipped a `--fix` that ignored
// the worktree with every test still green.
//
// Run: node --test scripts/check-ledger.test.mjs
import assert from 'node:assert/strict';
import test from 'node:test';
import os from 'node:os';
import path from 'node:path';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';

import { closeEntries, entryIdentity, main, openEntries, readEntries } from './check-ledger.mjs';

const RULE = '_'.repeat(81);

// Both rule forms, the way the checker itself reads them: bare for every entry
// written before the convention, identity-bearing after it.
const rules = (text) => text.split('\n').filter((line) => /^_{5,}(?:[ \t].*)?$/.test(line));

// `worktree` is a parameter because it is the half of the identity the clock
// cannot supply: pass a branch for an entry written in a worktree, null for one
// written on the main checkout, which has no branch to name.
function entry(time, prose, worktree = 'main') {
  const head = [`time:      [${time}] [01-01-26]`, 'agent:     [test]'];
  if (worktree !== null) head.push(`worktree:  ${worktree}`);
  return [...head, '', prose];
}

// A healthy ledger in the OLD form: every entry closed by a bare rule, one blank
// line between the rule and the next header. Every entry this ledger already
// holds looks like this and always will, so it is the fixture that pins the bare
// rule as valid forever rather than the one that pins the fix.
const CLEAN = [...entry('10:00', 'First.'), RULE, '', ...entry('11:00', 'Second.'), RULE, ''].join('\n');

// What the union driver actually left behind while every entry closed the same
// way: the rule the two entries shared is gone, so the second header sits
// directly under the first entry's prose.
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
  // 11 are why subtracting rules from entries answers "eleven to spare" for a
  // ledger with nothing open at all. The check is "a rule before the next
  // header", never "a rule on the last line". The first entry's rule is
  // followed by more prose, so a last-line reading would call it open — which
  // is what makes this fixture discriminate.
  const quoted = [...entry('10:00', 'First.'), RULE, 'Quoting the rule above, not closing on it.', ''];
  const ledger = [...quoted, ...entry('11:00', 'Second.'), RULE].join('\n');
  assert.equal(openEntries(ledger).length, 0);
  assert.notEqual(quoted.filter((line) => line.trim()).pop(), RULE, 'the fixture must not end on a rule');
});

test('a rule closes its entry whether it is bare or carries an identity', () => {
  // The reason the pattern admits a suffix at all, and the reason it must go on
  // admitting a bare rule: this ledger's 287 rules are bare, it is append-only,
  // and a checker that demanded the new form would red every one of them.
  const identityForm = [
    ...entry('10:00', 'First.', 'loop/slice-a'),
    `${RULE} 10:00 loop/slice-a`,
    '',
    ...entry('11:00', 'Second.', 'loop/slice-b'),
    `${RULE} 11:00 loop/slice-b`,
    '',
  ].join('\n');
  assert.equal(openEntries(identityForm).length, 0, 'an identity-bearing rule closes an entry');
  assert.equal(openEntries(CLEAN).length, 0, 'and the bare rule keeps closing one, forever');
  // The suffix is separated by a space or a tab, never run on. Widening this to
  // `_{5,}.*` instead would make ordinary emphasised prose read as a rule and
  // silently close the entry it appears in.
  const runOn = [...entry('10:00', 'First.'), `${RULE}not-a-rule`, ''].join('\n');
  assert.equal(openEntries(runOn).length, 1, 'underscores run on into text are prose, not a separator');
});

test('arbitrary prose after the bar is not an identity separator', () => {
  const malformed = [...entry('10:00', 'First.'), `${RULE} not-an-identity`, ''].join('\n');
  assert.equal(openEntries(malformed).length, 1);
});

test("entryIdentity is the entry's own minute and the branch it was written on", () => {
  assert.equal(entryIdentity(entry('9:04', 'On a slice branch.', 'loop/short-clock')), '9:04 loop/short-clock');
  assert.equal(entryIdentity(entry('23:52', 'On a slice branch.', 'loop/my-slice')), '23:52 loop/my-slice');
  assert.equal(entryIdentity(entry('23:52', 'On the main checkout.', null)), '23:52');
  // Read off the entry's own lines and nothing else, so a repair is
  // reproducible from the ledger rather than from the checkout it runs in.
  assert.equal(
    entryIdentity(['time:      [01-01-26]', 'agent:     [test]', '', 'A header with no clock in it.']),
    '',
    'an entry naming neither a minute nor a branch has no identity to write',
  );
  assert.equal(
    entryIdentity(['time:      [09:15] [01-01-26]', 'worktree:  loop/first', 'worktree:  loop/second']),
    '09:15 loop/first',
    'the first worktree field wins, so a quoted one later in the prose cannot displace it',
  );
});

test('a repair written from a one-digit ledger hour parses as closed on the next run', () => {
  const folded = entry('9:04', 'One-digit hours remain valid ledger identity.', 'loop/short-clock').join('\n');
  const { text, closed } = closeEntries(folded);
  assert.equal(closed.length, 1);
  assert.match(text, new RegExp(`${RULE} 9:04 loop/short-clock`));
  assert.equal(openEntries(text).length, 0);
});

test('a malformed clock repairs with a bare rule that the next run accepts', () => {
  for (const clock of ['24:00', '99:99', '09:60']) {
    const folded = entry(clock, 'Historical malformed clock.', 'loop/legacy-clock').join('\n');
    const { text, closed } = closeEntries(folded);
    assert.equal(closed.length, 1, clock);
    assert.equal(entryIdentity(entry(clock, 'Historical malformed clock.', 'loop/legacy-clock')), '');
    assert.equal(openEntries(text).length, 0, clock);
    assert.equal(rules(text).at(-1), RULE, clock);
  }
});

test('closeEntries repairs the fold without deleting a line, and the rerun is clean', () => {
  const { text, closed } = closeEntries(FOLDED);
  assert.equal(closed.length, 1);
  assert.equal(openEntries(text).length, 0);

  const before = FOLDED.split('\n');
  const after = text.split('\n');
  assert.ok(after.length > before.length, 'the repair inserts');
  assert.ok(isSubsequence(before, after), 'the repair deletes nothing');
  assert.deepEqual(
    rules(text),
    [`${RULE} 10:00 main`, RULE],
    'the repair closes the folded entry with its identity and leaves the bare rule it found untouched',
  );
});

test('closeEntries writes one identity per entry, and no two entries a shared line', () => {
  // The whole point, stated as the two ways an entry can differ from the one
  // above it. The first was written in a worktree and the second on the main
  // checkout, so one run has to emit two different shapes — assert them as
  // exact lines: delete the worktree half of entryIdentity and the first
  // degrades to `___ 11:04`, which every looser assertion still accepts.
  const unclosed = [
    ...entry('11:04', 'Appended on a slice branch, and never closed.', 'loop/slice-a'),
    '',
    ...entry('11:07', 'Appended on the main checkout, where one writer means nothing to race.', null),
    '',
  ].join('\n');
  const { text } = closeEntries(unclosed);
  assert.equal(openEntries(text).length, 0);
  assert.deepEqual(rules(text), [`${RULE} 11:04 loop/slice-a`, `${RULE} 11:07`]);
});

test('two entries written in the same minute still close with two different rules', () => {
  // Minute resolution is not identity: two slices in one wave land in the same
  // minute often, and here the worktree is the whole of what tells them apart.
  const sameMinute = [
    ...entry('10:01', 'Slice A.', 'loop/slice-a'),
    '',
    ...entry('10:01', 'Slice B.', 'loop/slice-b'),
    '',
  ].join('\n');
  const [first, second] = rules(closeEntries(sameMinute).text);
  assert.notEqual(first, second, 'two entries that end with the same line are exactly what union cannot keep apart');
  assert.deepEqual([first, second], [`${RULE} 10:01 loop/slice-a`, `${RULE} 10:01 loop/slice-b`]);
});

test('an entry with no identity to write still gets closed, by a bare rule', () => {
  // Closing the entry is the job; the identity is what keeps the next merge off
  // it. An entry that can name neither still has to come back closed rather
  // than get a rule with a trailing space where its identity would go.
  const undated = [
    'time:      [01-01-26]',
    'agent:     [test]',
    '',
    'No clock in the header and no branch in the body.',
    '',
  ].join('\n');
  const { text } = closeEntries(undated);
  assert.equal(openEntries(text).length, 0);
  assert.deepEqual(rules(text), [RULE]);
});

test('the repair copies the rule width the file already uses', () => {
  const narrow = '_'.repeat(73);
  const folded = [...entry('10:00', 'First.'), ...entry('11:00', 'Second.'), narrow, ''].join('\n');
  const repaired = closeEntries(folded).text;
  assert.deepEqual(rules(repaired), [`${narrow} 10:00 main`, narrow]);
  assert.ok(!rules(repaired).some((rule) => rule.startsWith(RULE)), 'never the default width when the file has one of its own');
});

test('the width is the underscore run, not the length of the line carrying it', () => {
  // An identity-bearing rule is longer than its bar, so a repair that measured
  // the whole line would widen every rule it wrote after the first one — and
  // then widen again off that, one repair at a time.
  const narrow = '_'.repeat(73);
  const ledger = [
    ...entry('09:00', 'Closed, and closed in the new form.', 'loop/base'),
    `${narrow} 09:00 loop/base`,
    '',
    ...entry('10:00', 'Open.', 'loop/slice-a'),
    '',
  ].join('\n');
  const repaired = rules(closeEntries(ledger).text);
  assert.equal(repaired.at(-1), `${narrow} 10:00 loop/slice-a`);
  assert.equal(repaired.at(-1).match(/^_+/)[0].length, narrow.length, 'the bar stays the width the ledger already uses');
});

// The two shapes the 22 entries this ledger had open when the checker landed
// actually came in. 18 of them abut — the fold FOLDED already covers — but 4
// keep a blank line where the rule should be, which is an entry written without
// one rather than a merge artefact. The repair has to close both, and it must
// not double the blank.
test('an entry left open with its blank line intact is closed without doubling it', () => {
  const unruled = [...entry('10:00', 'First.'), '', ...entry('11:00', 'Second.'), RULE, ''].join('\n');
  const repaired = closeEntries(unruled).text.split('\n');
  assert.equal(openEntries(repaired.join('\n')).length, 0);
  assert.equal(repaired[5], `${RULE} 10:00 main`, 'the rule lands on the blank line the entry already had after its prose');
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

test('main --fix closes the ledger in place, in the identity form, and exits 0', async () => {
  const file = await tempLedger(FOLDED);
  assert.equal(await main([file, '--fix']), 0);
  const repaired = await readFile(file, 'utf8');
  assert.equal(openEntries(repaired).length, 0);
  assert.ok(
    rules(repaired).includes(`${RULE} 10:00 main`),
    'a repair that wrote a bare rule would hand the next merge the same shared line to fold on',
  );
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
