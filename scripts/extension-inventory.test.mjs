#!/usr/bin/env node
import { readFileSync, readdirSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = execFileSync("git", ["rev-parse", "--show-toplevel"], { encoding: "utf8" }).trim();
process.chdir(repoRoot);
const sidepanel = readFileSync(new URL("../extension/sidepanel.html", import.meta.url), "utf8");
const cssLinks = [...sidepanel.matchAll(/href="dist\/([^\"]+\.css)"/g)].map((match) => match[1]);

const expected = [
  "tokens.css",
  "base.css",
  "chrome.css",
  "island.css",
  "transcript.css",
  "components.css",
  "composer.css",
  "panels.css",
  "deck.css",
  "workspace.css",
  "rooms-workspace.css",
  "rooms-interaction.css",
  "rooms-markdown.css",
  "council.css",
  "call.css",
  "canvas.css",
  "observatory.css",
  "compact.css",
  "float.css",
];

if (JSON.stringify(cssLinks) !== JSON.stringify(expected)) {
  throw new Error(`sidepanel stylesheet order mismatch\nexpected: ${expected.join(", ")}\nactual:   ${cssLinks.join(", ")}`);
}

const buildScript = readFileSync(new URL("../scripts/build-extension.sh", import.meta.url), "utf8");
if (!buildScript.includes("cp dist/*.css \"$DIST/\"")) {
  throw new Error("build-extension.sh no longer copies dist/*.css into extension/dist/");
}

// The two claims above read checked-in source. This one reads extension/dist/,
// a gitignored BUILD OUTPUT that only scripts/build-extension.sh produces, and
// producing it costs a full `trunk build --release`. CI runs this guard on a
// bare checkout, so it cannot make this claim — and a guard that goes red
// because nobody built first teaches people to ignore it, which is worse than
// the gap. Unbuilt, it says so and leaves the two source claims standing.
//
// No CSS at all counts as unbuilt rather than as drift. build-extension.sh
// copies the stylesheets under `set -e`, so it cannot leave the directory
// CSS-less; a bundle in that state was written by the auto-deploy script's
// rebuild_extension instead. (Its guard, scripts/surface-auto-deploy.test.mjs,
// used to trigger that against this very checkout; since #184 it runs against
// a stub repository and pins this tree untouched, so a CSS-less extension/dist
// here means someone ran the script by hand.) Reporting that as a drifted
// inventory sends the reader to the wrong file entirely.
let copied = [];
try {
  copied = readdirSync(resolve(here, "../extension/dist"), { withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name.endsWith(".css"))
    .map((entry) => entry.name)
    .sort();
} catch (err) {
  if (err.code !== "ENOENT") throw err;
}

if (!copied.length) {
  console.log(
    `SOURCE CLAIMS PASS: sidepanel.html links ${expected.length} stylesheets in order and build-extension.sh still copies them — ` +
      `extension/dist holds no built bundle, so its inventory went unchecked (run scripts/build-extension.sh to check it)`,
  );
} else {
  const missing = expected.filter((name) => !copied.includes(name));
  if (missing.length) {
    throw new Error(`extension/dist missing expected css: ${missing.join(", ")}`);
  }
  console.log(`ALL PASS: side panel stylesheet inventory — ${expected.length} sheets linked, copied, and present in the built bundle`);
}
