#!/usr/bin/env node
// Answers the question scripts/check-ledger.mjs deliberately does not: is this
// ledger still written at the END? Five entries sat at the TOP of events.md
// newest-first — 09-01, 08-30, 08-06, 07-19, 07-18 — above an entry from 06-26,
// for months, and every check called the file clean, because the checker reads
// a `time:` header only as far as the word. A ledger that is read newest-last
// and prepended newest-first is two ledgers interleaved, and the loop's rebase
// gate, which diffs the checker's verdict either side of a merge, had no
// verdict to diff.
//
// THE RULE, AND WHY IT IS NOT "SORTED". Entries land in MERGE order, not clock
// order: the union driver appends each PR's entry when the PR lands, and two
// slices of one wave written at 07:07 and 07:26 land whichever way their PRs
// merged. This ledger carries forty such adjacent descents, all of hours, and
// every one of them is a ledger doing exactly what it should. So the rule is a
// BAND, not a sort: an entry may sit below one newer than it by up to a day
// (TOLERANCE_MINUTES), and an entry that is more than a day newer than one
// that follows it — or more than a day older than one above it — is out of
// place. The five prologue entries were 26 hours to 67 days out of place; the
// forty merge-order descents are 3 minutes to 18 hours. A day divides them
// with room on both sides, and nothing about how the loop merges can widen a
// descent past a day without a human doing it by hand.
//
// WHICH ENTRY IS THE CULPRIT. A prepended entry makes every entry below it
// "older than one above"; a backdated append makes every entry above it
// "newer than one below". Reporting victims would list two hundred lines for
// one misplaced entry, so this finds the longest run of entries that IS within
// the band and names what is left out. One misplaced entry costs one line of
// report, whichever end it sits at.
//
// WHAT IT READS. `time: [HH:MM] [MM-DD-YY]` is the format the ledger job in
// .github/workflows/ci.yml asks for, and it is what the loop writes. History
// wrote `[9:45am]`, `[ 3:37AM]`, `[09:31PM]`, and dates as DD-MM-YY for a
// stretch of July and one day of August; all of those parse here, because an
// append-only ledger never stops carrying them and a check that could not read
// its own history would have to be told where to start. A date whose two
// fields are both twelve or under is read month-first, and day-first only when
// month-first lands more than a week from the entry above and day-first lands
// within one — which is what a DD-MM-YY slip looks like and what a genuine
// week-long gap does not. A header that parses as neither is skipped and
// counted, never red: this check owns order, not format.
//
// ONE COPY, AND DELIBERATELY UNSTAMPED. scripts/check-ledger.mjs exists in
// three repos and carries a CODE_DIGEST because three copies drift; this file
// exists in ocean-surface only, so there is nothing for a digest to hold
// equal and a stamp here would be a claim about copies that do not exist.
// The day a sibling repo ports it, stamp both in the same PR — the fork that
// went unnoticed for eleven waves one file over is the reason this sentence
// is here rather than left for that reader to work out.
//
// NO --fix. Moving an entry is a decision about where history goes, and the
// one time it was done (the five above, into the slots their stamps name) it
// was done by hand and recorded in the ledger itself.
import path from 'node:path';
import { readFile, realpath } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

const scriptPath = fileURLToPath(import.meta.url);
const repoRoot = path.resolve(path.dirname(scriptPath), '..');

const HELP = `Usage:
  node scripts/check-ledger-order.mjs          check this repo's events.md
  node scripts/check-ledger-order.mjs <path>   check another ledger

Reports every entry that sits more than a day out of clock order — newer than
an entry below it, or older than one above it, by more than TOLERANCE_MINUTES.
Descents of up to a day are merge order and pass. Headers that do not parse
are counted and skipped.
Exit 0 when every entry is in place, 1 when one is not, 2 when the check could
not run — an unreadable file, or one holding no entries at all.`;

export const TOLERANCE_MINUTES = 24 * 60;

const ENTRY_HEADER = /^time:/;
// Two bracketed fields, whitespace inside the brackets tolerated: `[ 3:37AM]`
// was written once and is history now.
const HEADER = /^time:\s*\[\s*([^\]]*?)\s*\]\s*\[\s*([^\]]*?)\s*\]/;
const TIME = /^(\d{1,2}):(\d{2})\s*(am|pm)?$/i;
const DATE = /^(\d{1,2})-(\d{1,2})-(\d{2})$/;
const WEEK = 7 * 24 * 60;
// The DD-MM-YY stretch this ledger wrote runs from mid-July to 31-08-26. Past
// that day the ledger job's MM-DD-YY is the only reading, so an ambiguous
// date is never rescued by proximity again: a canonical month-first backdate
// stamped after a September entry must stay a backdate, and reading it
// day-first "because that lands nearer" is exactly the miss a check for
// backdates cannot afford.
export const DAY_FIRST_ERA_ENDS = Date.UTC(2026, 7, 31, 23, 59) / 60000;

function minutes(year, month, day, hour, minute) {
  return Date.UTC(year, month - 1, day, hour, minute) / 60000;
}

function parseTime(text) {
  const m = text.match(TIME);
  if (!m) return null;
  let hour = Number(m[1]);
  const minute = Number(m[2]);
  const meridiem = m[3]?.toLowerCase();
  if (minute > 59) return null;
  if (meridiem) {
    if (hour < 1 || hour > 12) return null;
    if (meridiem === 'pm' && hour < 12) hour += 12;
    if (meridiem === 'am' && hour === 12) hour = 0;
  } else if (hour > 23) {
    return null;
  }
  return { hour, minute };
}

// `previous` is the stamp of the entry above, in minutes, or null. It settles
// only the ambiguous dates; a field over twelve settles itself.
export function parseStamp(header, previous = null) {
  const m = header.match(HEADER);
  if (!m) return null;
  const time = parseTime(m[1]);
  const date = m[2].match(DATE);
  if (!time || !date) return null;
  const a = Number(date[1]);
  const b = Number(date[2]);
  const year = 2000 + Number(date[3]);
  const valid = (month, day) => month >= 1 && month <= 12 && day >= 1 && day <= 31;
  const monthFirst = valid(a, b) ? minutes(year, a, b, time.hour, time.minute) : null;
  const dayFirst = valid(b, a) ? minutes(year, b, a, time.hour, time.minute) : null;
  if (monthFirst === null) return dayFirst;
  if (dayFirst === null || a === b) return monthFirst;
  const inEra = monthFirst <= DAY_FIRST_ERA_ENDS && dayFirst <= DAY_FIRST_ERA_ENDS;
  if (inEra && previous !== null && Math.abs(monthFirst - previous) > WEEK && Math.abs(dayFirst - previous) <= WEEK) {
    return dayFirst;
  }
  return monthFirst;
}

// Pure over the ledger text. One record per `time:` header: its 1-based line,
// the header as written, and its stamp in minutes or null when unparsed.
export function readStamps(text) {
  const lines = text.split('\n');
  const entries = [];
  let previous = null;
  lines.forEach((raw, index) => {
    if (!ENTRY_HEADER.test(raw)) return;
    const header = raw.trim();
    const stamp = parseStamp(header, previous);
    if (stamp !== null) previous = stamp;
    entries.push({ line: index + 1, header, stamp });
  });
  return entries;
}

// The longest run of stamped entries, in file order, in which no entry sits
// more than `tolerance` below the newest entry before it in the run.
// Everything stamped and not in that run is out of place.
//   One state per entry is not enough: two runs of equal length ending at the
// same entry can carry different newest stamps, and only the lower one may be
// able to continue. Sep 3, Sep 2, Sep 1, Sep 1, Sep 1 is the shape — the run
// [Sep 3, Sep 2] is longer at Sep 2 than [Sep 2] alone, but only [Sep 2] can
// take the Sep 1s, and a search that kept the longer one would blame Sep 2 as
// well as Sep 3. So each entry keeps every state that is not dominated: a
// state is dropped only when another at the same entry is at least as long AND
// no newer. Ties on length at the end go to the run that ends EARLIEST in
// the file, then to the lower newest stamp: an append-only ledger's older
// part is the trusted part, so between two equal runs the entries that came
// later are the ones reported — a backdated append loses to the entry above
// it, and a prepended copy loses to the body it copies.
export function misplacedEntries(entries, tolerance = TOLERANCE_MINUTES) {
  const stamped = entries.filter((entry) => entry.stamp !== null);
  const n = stamped.length;
  const states = stamped.map(() => []);
  const keep = (i, candidate) => {
    const list = states[i];
    for (const s of list) if (s.length >= candidate.length && s.newest <= candidate.newest) return;
    states[i] = list.filter((s) => !(candidate.length >= s.length && candidate.newest <= s.newest));
    states[i].push(candidate);
  };
  for (let i = 0; i < n; i++) {
    const stamp = stamped[i].stamp;
    keep(i, { length: 1, newest: stamp, prev: null });
    for (let j = 0; j < i; j++) {
      for (const s of states[j]) {
        if (stamp < s.newest - tolerance) continue;
        keep(i, { length: s.length + 1, newest: Math.max(s.newest, stamp), prev: { i: j, state: s } });
      }
    }
  }
  let best = null;
  for (let i = 0; i < n; i++) {
    for (const s of states[i]) {
      if (best === null || s.length > best.state.length || (s.length === best.state.length && (i < best.i || (i === best.i && s.newest < best.state.newest)))) {
        best = { i, state: s };
      }
    }
  }
  const kept = new Set();
  for (let cur = best; cur !== null; cur = cur.state.prev === null ? null : { i: cur.state.prev.i, state: cur.state.prev.state }) kept.add(cur.i);
  return stamped
    .map((entry, i) => ({ entry, i }))
    .filter(({ i }) => !kept.has(i))
    .map(({ entry, i }) => ({ ...entry, against: evidence(stamped, i, kept, tolerance) }));
}

// The in-place entry the culprit conflicts with, and by how much: the nearest
// one below that it is too new for, else the nearest one above that it is too
// old for.
function evidence(stamped, i, kept, tolerance) {
  for (let k = i + 1; k < stamped.length; k++) {
    if (!kept.has(k)) continue;
    const gap = stamped[i].stamp - stamped[k].stamp;
    if (gap > tolerance) return { line: stamped[k].line, gap, direction: 'newer than the entry at line' };
  }
  for (let k = i - 1; k >= 0; k--) {
    if (!kept.has(k)) continue;
    const gap = stamped[k].stamp - stamped[i].stamp;
    if (gap > tolerance) return { line: stamped[k].line, gap, direction: 'older than the entry at line' };
  }
  return null;
}

export function describeGap(gap) {
  const days = gap / (24 * 60);
  if (days >= 2) return `${Math.round(days)} days`;
  const hours = gap / 60;
  if (hours >= 2) return `${Math.round(hours)} hours`;
  return `${Math.round(gap)} minutes`;
}

function report(misplaced) {
  return misplaced.map((entry) => {
    const where = entry.against
      ? `${describeGap(entry.against.gap)} ${entry.against.direction} ${entry.against.line}`
      : 'out of the band the rest of the ledger keeps';
    return `  line ${String(entry.line).padStart(5)}  ${entry.header}  ${where}`;
  });
}

export async function main(argv = process.argv.slice(2)) {
  if (argv.some((arg) => arg === '-h' || arg === '--help') || argv.length > 1) {
    console.log(HELP);
    return argv.length > 1 ? 2 : 0;
  }
  const file = path.resolve(argv[0] || path.join(repoRoot, 'events.md'));
  let text;
  try {
    text = await readFile(file, 'utf8');
  } catch (error) {
    console.error(`check-ledger-order: cannot read ${file}: ${error.message}`);
    return 2;
  }
  const entries = readStamps(text);
  if (!entries.length) {
    console.error(`check-ledger-order: ${file} holds no \`time:\` entries — wrong path, or a ledger that lost its contents`);
    return 2;
  }
  const unparsed = entries.filter((entry) => entry.stamp === null).length;
  const skipped = unparsed ? `, ${unparsed} header${unparsed === 1 ? '' : 's'} not parsed and skipped` : '';
  const misplaced = misplacedEntries(entries);
  if (!misplaced.length) {
    console.log(`${file}: ${entries.length} entries, every one within a day of merge order${skipped}`);
    return 0;
  }
  console.log(`${file}: ${misplaced.length} of ${entries.length} entries sit more than a day out of order${skipped}`);
  for (const line of report(misplaced)) console.log(line);
  console.log('an append-only ledger grows at the END: move each entry to where its stamp falls, then rerun');
  return 1;
}

// Same guard as scripts/check-ledger.mjs, for the same reason: both sides
// resolved, or a symlinked invocation path silences the whole program.
async function invokedAsScript() {
  if (!process.argv[1]) return false;
  try {
    return (await realpath(scriptPath)) === (await realpath(process.argv[1]));
  } catch {
    return false;
  }
}

if (await invokedAsScript()) {
  process.exitCode = await main();
}
