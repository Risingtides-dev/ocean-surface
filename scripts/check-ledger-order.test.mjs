// Covers scripts/check-ledger-order.mjs. The fixtures are the shapes this
// ledger has actually held: a newest-first prologue above a chronological
// body, merge-order descents of hours, and the mixed clocks and date orders
// history wrote. The one thing the real ledger cannot supply is a backdated
// APPEND, so that is built here.
import assert from 'node:assert/strict';
import test from 'node:test';
import os from 'node:os';
import path from 'node:path';
import { mkdtemp, writeFile } from 'node:fs/promises';

import {
  TOLERANCE_MINUTES,
  describeGap,
  main,
  misplacedEntries,
  parseStamp,
  readStamps,
} from './check-ledger-order.mjs';

const DAY = 24 * 60;

function entry(time, date, body = 'prose') {
  return `time:      [${time}] [${date}]\nagent:     [test]\ntype:      [test]\n\n${body}\n${'_'.repeat(81)}\n`;
}

function ledger(...headers) {
  return headers.map(([time, date]) => entry(time, date)).join('\n');
}

async function capture(fn) {
  const out = [];
  const err = [];
  const { log, error } = console;
  console.log = (line) => out.push(line);
  console.error = (line) => err.push(line);
  try {
    return { code: await fn(), out, err };
  } finally {
    console.log = log;
    console.error = error;
  }
}

test('parseStamp reads the 24-hour MM-DD-YY form the ledger job asks for', () => {
  assert.equal(parseStamp('time:      [20:07] [09-01-26]'), Date.UTC(2026, 8, 1, 20, 7) / 60000);
  assert.equal(parseStamp('time: [01:06] [09-01-26]'), Date.UTC(2026, 8, 1, 1, 6) / 60000);
});

test('parseStamp reads every clock history wrote: am/pm in any case, with a stray space', () => {
  assert.equal(parseStamp('time: [9:45am] [08-06-26]'), Date.UTC(2026, 7, 6, 9, 45) / 60000);
  assert.equal(parseStamp('time: [11:52pm] [07-18-26]'), Date.UTC(2026, 6, 18, 23, 52) / 60000);
  assert.equal(parseStamp('time: [09:31PM] [07-05-26]'), Date.UTC(2026, 6, 5, 21, 31) / 60000);
  assert.equal(parseStamp('time: [ 3:37AM] [07-10-26]'), Date.UTC(2026, 6, 10, 3, 37) / 60000);
  assert.equal(parseStamp('time: [12:07pm] [06-26-26]'), Date.UTC(2026, 5, 26, 12, 7) / 60000, 'noon');
  assert.equal(parseStamp('time: [12:42am] [07-07-26]'), Date.UTC(2026, 6, 7, 0, 42) / 60000, 'midnight');
});

test('a field over twelve settles the date order by itself', () => {
  assert.equal(parseStamp('time: [14:25] [14-07-26]'), Date.UTC(2026, 6, 14, 14, 25) / 60000, 'day first');
  assert.equal(parseStamp('time: [14:50] [31-08-26]'), Date.UTC(2026, 7, 31, 14, 50) / 60000, 'day first');
  assert.equal(parseStamp('time: [10:00] [08-27-26]'), Date.UTC(2026, 7, 27, 10, 0) / 60000, 'month first');
});

test('an ambiguous date is month-first unless only day-first keeps it near the entry above', () => {
  const julyEighth = Date.UTC(2026, 6, 8, 12, 0) / 60000;
  assert.equal(parseStamp('time: [03:50pm] [08-07-26]'), Date.UTC(2026, 7, 7, 15, 50) / 60000, 'alone: August 7');
  assert.equal(
    parseStamp('time: [03:50pm] [08-07-26]', julyEighth),
    Date.UTC(2026, 6, 8, 15, 50) / 60000,
    'under a July 8 entry, a DD-MM-YY slip for July 8',
  );
  assert.equal(
    parseStamp('time: [09:00] [07-09-26]', julyEighth),
    Date.UTC(2026, 6, 9, 9, 0) / 60000,
    'month-first within a week stays month-first',
  );
  assert.equal(parseStamp('time: [09:00] [06-06-26]', julyEighth), Date.UTC(2026, 5, 6, 9, 0) / 60000, 'equal fields');
});

test('a header that parses as neither is null, not a throw', () => {
  assert.equal(parseStamp('time: [soon] [09-01-26]'), null);
  assert.equal(parseStamp('time: [10:00] [2026-09-01]'), null);
  assert.equal(parseStamp('time: [25:00] [09-01-26]'), null);
  assert.equal(parseStamp('time: [13:00pm] [09-01-26]'), null);
  assert.equal(parseStamp('time: [10:00] [13-13-26]'), null);
  assert.equal(parseStamp('time:'), null);
});

test('readStamps keeps every header, stamped or not, at its 1-based line', () => {
  const text = `${entry('10:00', '09-01-26')}\n${entry('soon', '09-01-26')}\n${entry('11:00', '09-01-26')}`;
  const entries = readStamps(text);
  assert.deepEqual(
    entries.map((e) => [e.line, e.stamp === null]),
    [[1, false], [8, true], [15, false]],
  );
});

test('a newest-first prologue above a chronological body names the prologue, and only it', () => {
  const text = ledger(
    ['01:06', '09-01-26'],
    ['23:14', '08-30-26'],
    ['09:45am', '08-06-26'],
    ['01:15pm', '07-19-26'],
    ['11:52pm', '07-18-26'],
    ['11:25pm', '06-26-26'],
    ['12:07pm', '06-26-26'],
    ['9:05pm', '07-04-26'],
    ['8:20pm', '07-04-26'],
    ['15:38', '19-07-26'],
    ['03:47', '19-07-26'],
    ['20:07', '09-01-26'],
  );
  const misplaced = misplacedEntries(readStamps(text));
  assert.deepEqual(
    misplaced.map((e) => e.header.replace(/\s+/g, ' ')),
    [
      'time: [01:06] [09-01-26]',
      'time: [23:14] [08-30-26]',
      'time: [09:45am] [08-06-26]',
      'time: [01:15pm] [07-19-26]',
      'time: [11:52pm] [07-18-26]',
    ],
  );
  assert.equal(misplaced[0].against.direction, 'newer than the entry at line');
  assert.equal(describeGap(misplaced[0].against.gap), '66 days');
  assert.equal(describeGap(misplaced[1].against.gap), '65 days');
});

test('merge-order descents of hours are in place, up to a day', () => {
  const text = ledger(
    ['07:26', '08-31-26'],
    ['07:07', '08-31-26'],
    ['10:22', '08-31-26'],
    ['08:48', '08-31-26'],
    ['18:54', '08-31-26'],
    ['00:41', '08-31-26'],
    ['03:21', '09-01-26'],
    ['02:55', '09-01-26'],
  );
  assert.deepEqual(misplacedEntries(readStamps(text)), []);
});

test('the band is measured against the newest entry so far, not the neighbour', () => {
  const steps = [];
  for (let i = 0; i < 6; i++) steps.push([`${String(20 - i * 3).padStart(2, '0')}:00`, '08-31-26']);
  const text = ledger(['09:00', '09-01-26'], ...steps);
  const misplaced = misplacedEntries(readStamps(text));
  assert.deepEqual(
    misplaced.map((e) => e.header.replace(/\s+/g, ' ')),
    ['time: [08:00] [08-31-26]', 'time: [05:00] [08-31-26]'],
    'each descent is under a day from its neighbour, and the last two are over a day from the top',
  );
});

test('a backdated append is the one entry named, not the two hundred above it', () => {
  const headers = [];
  for (let day = 1; day <= 28; day++) headers.push(['10:00', `08-${String(day).padStart(2, '0')}-26`]);
  headers.push(['10:00', '07-18-26']);
  const misplaced = misplacedEntries(readStamps(ledger(...headers)));
  assert.equal(misplaced.length, 1);
  assert.equal(misplaced[0].header.replace(/\s+/g, ' '), 'time: [10:00] [07-18-26]');
  assert.equal(misplaced[0].against.direction, 'older than the entry at line');
  assert.equal(describeGap(misplaced[0].against.gap), '41 days');
});

test('an unparsed header is skipped, and the entries around it are still judged', () => {
  const text = `${entry('10:00', '08-01-26')}\n${entry('soon', '08-02-26')}\n${entry('10:00', '08-03-26')}\n${entry('10:00', '07-01-26')}`;
  const misplaced = misplacedEntries(readStamps(text));
  assert.deepEqual(misplaced.map((e) => e.line), [22]);
});

test('the tolerance is one day, exactly, and a day is not over it', () => {
  assert.equal(TOLERANCE_MINUTES, DAY);
  const onTheLine = ledger(['10:00', '08-02-26'], ['10:00', '08-01-26']);
  assert.deepEqual(misplacedEntries(readStamps(onTheLine)), []);
  const overIt = ledger(['10:01', '08-02-26'], ['10:00', '08-01-26']);
  assert.equal(misplacedEntries(readStamps(overIt)).length, 1);
});

test('describeGap picks the unit a reader would', () => {
  assert.equal(describeGap(3), '3 minutes');
  assert.equal(describeGap(90), '90 minutes');
  assert.equal(describeGap(26 * 60), '26 hours');
  assert.equal(describeGap(3 * DAY + 60), '3 days');
});

test('main exits 0 on an in-place ledger, 1 on a misplaced entry, and says which', async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'ledger-order-'));
  const clean = path.join(dir, 'clean.md');
  await writeFile(clean, ledger(['10:00', '08-01-26'], ['09:00', '08-01-26'], ['10:00', '08-02-26']));
  const ok = await capture(() => main([clean]));
  assert.equal(ok.code, 0);
  assert.match(ok.out[0], /3 entries, every one within a day of merge order$/);

  const bad = path.join(dir, 'bad.md');
  await writeFile(bad, ledger(['10:00', '09-01-26'], ['soon', '08-01-26'], ['10:00', '08-01-26']));
  const red = await capture(() => main([bad]));
  assert.equal(red.code, 1);
  assert.match(red.out[0], /1 of 3 entries sit more than a day out of order, 1 header not parsed and skipped$/);
  assert.match(red.out[1], /line {5}1 {2}time: {6}\[10:00\] \[09-01-26\] {2}31 days newer than the entry at line 15$/);
  assert.match(red.out[2], /grows at the END/);
});

test('main exits 2 when the check could not run at all', async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'ledger-order-'));
  const missing = await capture(() => main([path.join(dir, 'missing.md')]));
  assert.equal(missing.code, 2);
  assert.match(missing.err[0], /cannot read/);
  const empty = path.join(dir, 'empty.md');
  await writeFile(empty, '# nothing here\n');
  const none = await capture(() => main([empty]));
  assert.equal(none.code, 2);
  assert.match(none.err[0], /holds no `time:` entries/);
  const help = await capture(() => main(['--help']));
  assert.equal(help.code, 0);
  assert.match(help.out[0], /Exit 0 when every entry is in place/);
});
