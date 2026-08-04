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

const copied = readdirSync(resolve(here, "../extension/dist"), { withFileTypes: true })
  .filter((entry) => entry.isFile() && entry.name.endsWith(".css"))
  .map((entry) => entry.name)
  .sort();

const missing = expected.filter((name) => !copied.includes(name));
if (missing.length) {
  throw new Error(`extension/dist missing expected css: ${missing.join(", ")}`);
}
