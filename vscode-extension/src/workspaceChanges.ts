import { execFile } from "node:child_process";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const maxDiffChars = 90000;
const maxGitBuffer = 12 * 1024 * 1024;

export interface WorkspaceChangesSnapshot {
  root: string;
  status: string;
  unstagedStat: string;
  stagedStat: string;
  unstagedDiff: TruncatedText;
  stagedDiff: TruncatedText;
}

interface TruncatedText {
  text: string;
  truncated: boolean;
}

export async function collectWorkspaceChanges(
  root: string,
): Promise<WorkspaceChangesSnapshot> {
  const status = await git(root, ["status", "--short"]);
  const [unstagedStat, stagedStat, unstagedDiff, stagedDiff] = await Promise.all([
    git(root, ["diff", "--stat", "--no-ext-diff"]),
    git(root, ["diff", "--cached", "--stat", "--no-ext-diff"]),
    git(root, ["diff", "--no-ext-diff"]),
    git(root, ["diff", "--cached", "--no-ext-diff"]),
  ]);

  return {
    root,
    status,
    unstagedStat,
    stagedStat,
    unstagedDiff: truncateMiddle(unstagedDiff, maxDiffChars),
    stagedDiff: truncateMiddle(stagedDiff, maxDiffChars),
  };
}

export function hasWorkspaceChanges(snapshot: WorkspaceChangesSnapshot): boolean {
  return Boolean(snapshot.status.trim());
}

export function formatWorkspaceChangesPrompt(
  snapshot: WorkspaceChangesSnapshot,
): string {
  const sections = [
    "You are reviewing the current VS Code/Cursor workspace changes.",
    "Use the git status, stats, and diffs below as source of truth.",
    "Prioritize bugs, regressions, missed tests, risky behavior, and unclear changes.",
    "Return concise findings first, ordered by severity, with file references when possible.",
    "If the diff is clean, say so and call out any residual test or verification gaps.",
    "",
    `Workspace: ${snapshot.root}`,
    "",
    "Git status:",
    codeBlock(snapshot.status || "(clean)"),
  ];

  if (snapshot.unstagedStat.trim()) {
    sections.push("", "Unstaged stat:", codeBlock(snapshot.unstagedStat));
  }
  if (snapshot.stagedStat.trim()) {
    sections.push("", "Staged stat:", codeBlock(snapshot.stagedStat));
  }
  if (snapshot.unstagedDiff.text.trim()) {
    sections.push(
      "",
      snapshot.unstagedDiff.truncated
        ? "Unstaged diff (truncated):"
        : "Unstaged diff:",
      codeBlock(snapshot.unstagedDiff.text, "diff"),
    );
  }
  if (snapshot.stagedDiff.text.trim()) {
    sections.push(
      "",
      snapshot.stagedDiff.truncated ? "Staged diff (truncated):" : "Staged diff:",
      codeBlock(snapshot.stagedDiff.text, "diff"),
    );
  }

  return sections.join("\n");
}

export function formatWorkspaceChangesContext(
  snapshot: WorkspaceChangesSnapshot,
): string {
  const sections = [
    `Workspace: ${snapshot.root}`,
    "",
    "Git status:",
    codeBlock(snapshot.status || "(clean)"),
  ];

  if (snapshot.unstagedStat.trim()) {
    sections.push("", "Unstaged stat:", codeBlock(snapshot.unstagedStat));
  }
  if (snapshot.stagedStat.trim()) {
    sections.push("", "Staged stat:", codeBlock(snapshot.stagedStat));
  }
  if (snapshot.unstagedDiff.text.trim()) {
    sections.push(
      "",
      snapshot.unstagedDiff.truncated
        ? "Unstaged diff (truncated):"
        : "Unstaged diff:",
      codeBlock(snapshot.unstagedDiff.text, "diff"),
    );
  }
  if (snapshot.stagedDiff.text.trim()) {
    sections.push(
      "",
      snapshot.stagedDiff.truncated ? "Staged diff (truncated):" : "Staged diff:",
      codeBlock(snapshot.stagedDiff.text, "diff"),
    );
  }

  return sections.join("\n");
}

async function git(root: string, args: string[]): Promise<string> {
  try {
    const { stdout } = await execFileAsync("git", ["-C", root, ...args], {
      maxBuffer: maxGitBuffer,
    });
    return stdout.trimEnd();
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    throw new Error(`git ${args.join(" ")} failed: ${detail}`);
  }
}

function truncateMiddle(text: string, maxChars: number): TruncatedText {
  if (text.length <= maxChars) {
    return { text, truncated: false };
  }
  const headLength = Math.floor(maxChars * 0.6);
  const tailLength = maxChars - headLength;
  return {
    text: [
      text.slice(0, headLength).replace(/\s+$/g, ""),
      `\n\n[... diff truncated: ${text.length - maxChars} chars omitted ...]\n\n`,
      text.slice(text.length - tailLength).replace(/^\s+/g, ""),
    ].join(""),
    truncated: true,
  };
}

function codeBlock(value: string, language = "text"): string {
  return ["```" + language, value, "```"].join("\n");
}
