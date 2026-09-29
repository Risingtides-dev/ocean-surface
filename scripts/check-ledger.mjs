#!/usr/bin/env node
// Answers one question: does every events.md entry still close with its `___`
// rule? Two entries that end with the SAME line are what the union merge driver
// (.gitattributes) cannot keep apart — it emits a line both sides added only
// once, so xdiff anchors each parallel append before the rule they share and one
// rule comes out for two entries. The entries then FUSE: the second `time:`
// header lands directly under the first entry's prose. No conflict, no marker,
// nothing a merge check would see — #181 folded onto #180 twelve minutes after
// #180 landed this checker, which caught the fold and could not prevent it. So
// a rule now carries its entry's own identity — `___ HH:MM <worktree>` — and
// two appends no longer END on the same line. They can still SHARE one: two
// entries written in the same minute open with the same blank and the same
// `time:` line, which union folds whatever branch each was written on, because
// `worktree:` sits below both. The loop still runs this before and after every
// rebase and diffs the verdict, because the 289 rules already in this ledger
// are bare and identical, and events.md is append-only: history keeps folding
// for as long as it is history.
//
// Ported from ocean-bedrock's scripts/check-ledger.mjs (its PR #62 for the
// checker, #98 for the identity separator, #103 for the realpath entry guard at
// the bottom and #124 for the stamp below the imports), the first two by way of
// the ocean-os copy. Only comments and the usage text differ from bedrock's,
// and the usage text only because it can name no npm script — this repo has no
// node manifest to hold one. That used to be a sentence here; it is now a
// digest that scripts/check-ledger.test.mjs recomputes on every run.
//
// WHAT THIS CHECK OWNS, AND THE TWO NEIGHBOURING THINGS IT DOES NOT:
//   It owns FUSION — an entry whose rule is gone and whose prose the next
//   header runs into. That is the damage; everything below is cosmetic.
//   It does NOT own the blank line between a rule and the next header, and that
//   is a RULING rather than an omission left for the next reader. A blank line
//   cannot be given an identity, so it is the one part of the format no
//   convention can protect from union: the entry owns its RULE and never the
//   blank. A merged append still lands its `time:` header flush against the
//   previous rule — every join of the three-way fixture in
//   scripts/events-merge-driver.test.mjs does exactly that, while wave 52's
//   real rebase kept this repo's blank and ate the sibling's, so it turns on
//   where xdiff anchors rather than on anything worth asserting. The entry
//   boundary survives either way, so a lost blank stays cosmetic and
//   hand-repaired, never red.
//   It does NOT own separator uniqueness. Requiring the identity form would red
//   every one of the 278 entries written before it and every entry a slice in
//   flight is writing right now. The bare form stays valid forever; `--fix` is
//   what writes the new one.
//
// TWO THINGS THIS CHECK DELIBERATELY DOES NOT DO:
//   Compare totals. Rule lines against entry count is a different question and
//   this ledger is the cleanest demonstration available that it lies: 298 rules
//   against 287 entries reads as eleven rules to SPARE, while the true open
//   count is zero — 11 entries carry a second rule inside their prose, so the
//   surplus IS the miscount. The same subtraction once read eleven short here
//   while 22 entries were open. Only "is THIS entry closed before the next one
//   starts" holds everywhere. (Counts as of the port of #124; they move with
//   every append and are not asserted anywhere.)
//   Match a fixed rule width. Every rule this ledger has written is 81
//   underscores and scripts/events-merge-driver.test.mjs builds its fixtures at
//   the same width, but the sibling repos use widths of their own; all of them
//   are the same rule, so the shape is what matters and never the length.
//
// AND ONE THING IT DOES NOT DO THAT A SIBLING SCRIPT NOW DOES: read the clock.
// Five entries sat at the TOP of this ledger newest-first for months and this
// checker called the file clean, because it never reads a `time:` header past
// the word. scripts/check-ledger-order.mjs owns that question, in its own file
// on purpose: an order rule that tolerates the union merge landing entries in
// MERGE order rather than clock order is this repo's own business, and the
// three copies of THIS file are meant to digest the same.
import path from 'node:path';
import { createHash } from 'node:crypto';
import { readFile, realpath, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

const scriptPath = fileURLToPath(import.meta.url);
const repoRoot = path.resolve(path.dirname(scriptPath), '..');

// ONE OF THREE COPIES. ocean-bedrock's scripts/check-ledger.mjs is the source
// and ocean-os carries the third. bedrock #103 made the entry guard at the
// bottom of this file compare REALPATHS, because comparing the two paths as
// typed silences the whole program under any symlinked invocation path and a
// ledger of open entries then exits 0 — and this copy sat on the pre-#103 guard
// through three bedrock waves while its header claimed byte-identity, which is
// exactly the rot bedrock #124 replaced with this stamp. CODE_REVISION names
// the code SHAPE and not the copy — copies in sync read the same revision —
// and CODE_DIGEST is a digest over this file's logic that
// scripts/check-ledger.test.mjs asserts against the bytes. Edit anything the
// digest covers and the suite reds until both are bumped on purpose: a header
// claim rots in silence, a digest is recomputed every run. Checking a sibling
// is one grep for CODE_DIGEST, or `--digest <copy>` against its file. r2 reads
// 56adab136337 in bedrock and here; r1, the shape ocean-os still carried on
// 09-01-26, answers de98a632f0df, and a copy that prints that is a copy still
// owed #103's fix.
//
// WHAT THE DIGEST COVERS, and why "every non-comment line" is the wrong rule.
// The usage text is excluded by name: copies that agree on every executable
// line already differ inside it — this one names no npm script — and a digest
// that read it would report a fork between copies that have none. Comments are
// excluded because each repo explains this file in its own terms.
//   The strip is textual — a line is a comment when its first non-space is a
// comment marker — which is exact for this file today, with no trailing
// comments and no string holding a marker, and would OVER-report if either
// arrived. That direction is deliberate: a fork reported where there is none
// costs a two-minute diff, and the quiet that let #103 sit unported here is
// the failure this is here to stop.
export const CODE_REVISION = 'r2';
export const CODE_DIGEST = '56adab136337';

const STAMP_LINE = /^export const CODE_(?:REVISION|DIGEST) = /;
const USAGE_OPEN = /^const HELP = `/;
const USAGE_CLOSE = /`;$/;

// Skips its own stamp lines, or the constant could never satisfy the digest it
// names. Rename either one and the skip stops matching, both lines join the
// body and the digest moves — which is right: a rename is an edit.
export function codeDigest(source) {
  const body = [];
  let inUsage = false;
  for (const raw of source.split('\n')) {
    const line = raw.replace(/\s+$/, '');
    if (inUsage || USAGE_OPEN.test(line)) {
      inUsage = !USAGE_CLOSE.test(line);
      continue;
    }
    if (line === '' || line.trimStart().startsWith('//') || STAMP_LINE.test(line)) continue;
    body.push(line);
  }
  return createHash('sha256').update(body.join('\n')).digest('hex').slice(0, 12);
}

const HELP = `Usage:
  node scripts/check-ledger.mjs                  check this repo's events.md
  node scripts/check-ledger.mjs <path>           check another ledger
  node scripts/check-ledger.mjs [path] --fix     close every open entry in place
  node scripts/check-ledger.mjs [copy] --digest  print a checker copy's code digest

Reports every entry not closed by a \`___\` rule before the next \`time:\`
header. A rule may be bare or carry the entry's identity (\`___ HH:MM
<worktree>\`); both close an entry, and \`--fix\` writes the identity form.
Exit 0 when the ledger is clean, 1 when an entry is open, 2 when the check
could not run — an unreadable file, or one holding no entries at all.
Order is a different question, owned by scripts/check-ledger-order.mjs.`;

const ENTRY_HEADER = /^time:/;
// Both forms, and the bare one forever: every rule written before the identity
// convention is bare, and an append-only ledger never stops carrying them.
const SEPARATOR_RULE = /^_{5,}(?:[ \t].*)?$/;
const RULE_BAR = /^_+/;
// What makes one entry's rule unlike its neighbours'. `HH:MM` alone is minute
// resolution and two slices in one wave land in the same minute often enough to
// have done it; the worktree is what the clock cannot give, and two parallel
// appends are by definition on two different branches. An entry with no
// worktree was written on the main checkout, where there is one writer and
// nothing to race, so its time alone is enough.
const HEADER_TIME = /\[(\d{1,2}:\d{2})\]/;
const WORKTREE_FIELD = /^worktree:[ \t]*(\S+)/;

// Only the fallback for a ledger with no rule left to copy: the width a repair
// writes is the width that file already uses, so a repaired entry stays
// byte-identical in shape to the entries around it.
const DEFAULT_RULE_WIDTH = 81;

function ruleWidth(lines) {
  const seen = new Map();
  for (const line of lines) {
    if (!SEPARATOR_RULE.test(line)) continue;
    // The underscore RUN, not the line: an identity-bearing rule carries a
    // suffix, and measuring the whole line would widen every repair after it.
    const width = line.match(RULE_BAR)[0].length;
    seen.set(width, (seen.get(width) || 0) + 1);
  }
  let width = DEFAULT_RULE_WIDTH;
  let commonest = 0;
  for (const [candidate, count] of seen) {
    if (count > commonest) {
      width = candidate;
      commonest = count;
    }
  }
  return width;
}

// Pure over the ledger text so the verdict is testable without a filesystem.
// `start`/`end` are 0-based half-open line indices; `line` and `runsInto` are
// what a human reads off an editor gutter.
export function readEntries(text) {
  const lines = text.split('\n');
  const starts = [];
  lines.forEach((line, index) => {
    if (ENTRY_HEADER.test(line)) starts.push(index);
  });
  return starts.map((start, n) => {
    const next = starts[n + 1];
    const end = next === undefined ? lines.length : next;
    return {
      start,
      end,
      line: start + 1,
      runsInto: next === undefined ? null : next + 1,
      header: lines[start].trim(),
      closed: lines.slice(start, end).some((line) => SEPARATOR_RULE.test(line)),
    };
  });
}

export function openEntries(text) {
  return readEntries(text).filter((entry) => !entry.closed);
}

// Read off the entry's own lines, so a repair is reproducible from the ledger
// and needs nothing about the checkout it runs in. An entry missing both fields
// gets the bare rule: the identity is a property of the entry, and one that
// names neither a time nor a branch has none to write.
export function entryIdentity(lines) {
  const parts = [];
  const time = lines[0]?.match(HEADER_TIME);
  if (time) parts.push(time[1]);
  const worktree = lines.map((line) => line.match(WORKTREE_FIELD)).find(Boolean);
  if (worktree) parts.push(worktree[1]);
  return parts.join(' ');
}

// Inserts each missing rule after the entry's last non-blank line, plus the
// blank line the format puts between a rule and the next header when the fold
// ate that too. The rule written is the IDENTITY form — a repair that emitted a
// bare one would close this entry and hand the next merge the same shared line
// to fold on. Repairs run back to front so the earlier indices stay valid.
export function closeEntries(text) {
  const lines = text.split('\n');
  const bar = '_'.repeat(ruleWidth(lines));
  const open = openEntries(text);
  for (const entry of [...open].reverse()) {
    let last = entry.end - 1;
    while (last > entry.start && lines[last].trim() === '') last--;
    const identity = entryIdentity(lines.slice(entry.start, entry.end));
    const rule = identity ? `${bar} ${identity}` : bar;
    const atEof = lines[last + 1] === undefined;
    const spaced = atEof || lines[last + 1] === '';
    lines.splice(last + 1, 0, ...(spaced ? [rule] : [rule, '']));
  }
  return { text: lines.join('\n'), closed: open };
}

function report(entries) {
  return entries.map(
    (entry) =>
      `  line ${String(entry.line).padStart(5)}  ${entry.header}  ` +
      (entry.runsInto === null ? 'runs to the end of the file' : `runs into the entry at line ${entry.runsInto}`),
  );
}

export async function main(argv = process.argv.slice(2)) {
  const fix = argv.includes('--fix');
  const digest = argv.includes('--digest');
  const args = argv.filter((arg) => arg !== '--fix' && arg !== '--digest');
  if (args.some((arg) => arg === '-h' || arg === '--help') || args.length > 1) {
    console.log(HELP);
    return args.length > 1 ? 2 : 0;
  }
  // `--digest` reads a CHECKER copy rather than a ledger, so it defaults to this
  // file and not to events.md. Everything else about the read is the same, which
  // is why it shares the branch: an unreadable path is still exit 2.
  const file = path.resolve(args[0] || (digest ? scriptPath : path.join(repoRoot, 'events.md')));
  let text;
  try {
    text = await readFile(file, 'utf8');
  } catch (error) {
    console.error(`check-ledger: cannot read ${file}: ${error.message}`);
    return 2;
  }

  if (digest) {
    console.log(`${codeDigest(text)}  ${file}`);
    return 0;
  }

  const entries = readEntries(text);
  // A ledger with nothing that parses as an entry is not a clean ledger. It is
  // the wrong path, or a rebase that emptied the real one — and the sibling
  // repo has already seen a checkout holding 37 entries against master's 72.
  // Both read as "0 open" and the loop diffs this check's verdict either side
  // of a rebase, so the one thing it must never do is answer that green.
  if (!entries.length) {
    console.error(`check-ledger: ${file} holds no \`time:\` entries — wrong path, or a ledger that lost its contents`);
    return 2;
  }

  const open = entries.filter((entry) => !entry.closed);
  if (!open.length) {
    console.log(`${file}: ${entries.length} entries, every one closed by its rule`);
    return 0;
  }

  if (fix) {
    await writeFile(file, closeEntries(text).text);
    console.log(`${file}: closed ${open.length} of ${entries.length} entries`);
    for (const line of report(open)) console.log(line);
    return 0;
  }
  console.log(
    `${file}: ${open.length} of ${entries.length} entries are not closed by a rule before the next entry begins`,
  );
  for (const line of report(open)) console.log(line);
  console.log('union folds two entries that end with the SAME rule — rerun with --fix to close them by identity');
  return 1;
}

// Both sides RESOLVED, because the two are not written the same way.
// `process.argv[1]` is the path the shell was handed, symlinks and all, while
// the ESM loader has already realpathed `import.meta.url`. Compare them as
// written and any symlinked component makes them disagree — /tmp is one on
// macOS, and so is the /var every `os.tmpdir()` sits under — so `main()` never
// runs, nothing is printed, and a ledger of open entries exits 0. Reporting
// nothing is the worst answer this script can give: the `ledger` job in CI and
// the loop's rebase gate both read that 0 as a clean file.
//
// bedrock #103's fix, and the last as-typed guard under this repo's scripts/.
// scripts/check-ledger.test.mjs runs this file through a symlink and expects
// the open-ledger exit, so the guard cannot quietly regress to the comparison
// it replaced.
async function invokedAsScript() {
  if (!process.argv[1]) return false;
  try {
    return (await realpath(scriptPath)) === (await realpath(process.argv[1]));
  } catch {
    // An argv[1] that resolves to nothing is not this script being run.
    return false;
  }
}

if (await invokedAsScript()) {
  process.exitCode = await main();
}
