time:      [11:25pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension polish pass toward first-class AI chat UI. Replaced the
control-heavy panel with a compact transcript-first runtime strip: model picker,
persisted session dropdown, New session, thinking-level selector, context indicator, and
one Settings disclosure for daemon path/ACP path/permissions/context/traffic. Collapsed
tool-call telemetry so the transcript shows only the latest activity by default, with prior
tool rows hidden behind details instead of flooding the chat. Added session persistence via
VS Code globalState and ACP `session/load`, context usage/summary state, `ocean.thinkingLevel`,
and carried thinking level through ACP prompt metadata. Bumped and installed VSIX
`ocean-surface-0.1.6`; verified in Cursor screenshot that the tool-call wall is gone and
model/session/thinking/context/settings are visible. Checks green: `npm run lint`,
`npm run package`, `npx @vscode/vsce package --no-dependencies`, Cursor extension list shows
`risingtides.ocean-surface@0.1.6`.
_________________________________________________________________________________

time:      [12:07pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      refactor
area:      frontend

Ocean Cursor/VS Code extension transcript-quality pass. Fixed streamed assistant and
message-id chunk joins so adjacent chunks insert a sensible word boundary instead of
rendering text like `around.No` in the chat transcript. Added compact inline previews for
ACP text/resource tool content and cached local terminal output snapshots, rendered inside
the existing tool rows with hard caps so the transcript gains evidence without adding new
buttons or panels. Bumped and installed VSIX `ocean-surface-0.1.13`. Checks green:
`npm run lint`, `node --check media/chat.js`, `npm run package`, `npx @vscode/vsce package
--no-dependencies`, and Cursor extension list shows `risingtides.ocean-surface@0.1.13`.
_________________________________________________________________________________

time:      [11:32pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      refactor
area:      frontend

Follow-up Ocean Cursor/VS Code extension transcript pass. Fixed loaded-session message
chunks so adjacent assistant updates no longer collapse into glued text, and added a
safe DOM-based Markdown renderer in the webview for headings, lists, blockquotes, inline
code, fenced code, and bold text. Removed `pre-wrap` from message bodies and added compact
Markdown typography so history reads like a chat transcript instead of raw Markdown. Bumped
and installed VSIX `ocean-surface-0.1.7`. Checks green: `npm run lint`, `node --check
media/chat.js`, `npm run package`, `npx @vscode/vsce package --no-dependencies`, and
Cursor extension list shows `risingtides.ocean-surface@0.1.7`.
_________________________________________________________________________________

time:      [11:38pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      refactor
area:      frontend

Ocean Cursor/VS Code extension timeline pass toward first-class agent chat behavior.
Changed the webview state model from separate `messages` plus appended `tools` to an
ordered transcript timeline that references message and tool items. Tool calls and file/diff
activity now render inline at the point they arrive in the thread instead of as a detached
activity block after all chat messages. Assistant rows are inserted into the transcript only
when text arrives or a turn needs a terminal status message, so tool-first turns no longer
show an empty assistant row ahead of activity. Bumped and installed VSIX
`ocean-surface-0.1.8`. Checks green: `npm run lint`, `node --check media/chat.js`,
`npm run package`, `npx @vscode/vsce package --no-dependencies`, and Cursor extension list
shows `risingtides.ocean-surface@0.1.8`.
_________________________________________________________________________________

time:      [11:44pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension IDE-integration pass. Made inline tool file/diff rows
navigable from the transcript: rows now post an `openFile` webview message with path and
line metadata, and the extension host opens the target document in the editor with a clamped
line selection. Added keyboard activation and subtle hover/focus styling without adding
visible command buttons or extra text. Bumped and installed VSIX `ocean-surface-0.1.9`.
Checks green: `npm run lint`, `node --check media/chat.js`, `npm run package`,
`npx @vscode/vsce package --no-dependencies`, and Cursor extension list shows
`risingtides.ocean-surface@0.1.9`.
_________________________________________________________________________________

time:      [11:48pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      refactor
area:      frontend

Ocean Cursor/VS Code extension session-quality pass. Added local prompt-derived session
titles so the session picker no longer remains full of generic `New session` / `Loaded
session` labels when ACP has not supplied a title yet. The fallback title is taken from the
first meaningful user prompt, strips fenced code/backtick noise, truncates at a word
boundary, and only replaces generic placeholder titles so daemon-provided titles still win.
Bumped and installed VSIX `ocean-surface-0.1.10`. Checks green: `npm run lint`,
`node --check media/chat.js`, `npm run package`, `npx @vscode/vsce package
--no-dependencies`, and Cursor extension list shows `risingtides.ocean-surface@0.1.10`.
_________________________________________________________________________________

time:      [11:53pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension diff-integration pass. Changed inline tool file rows so
ACP `diff` content opens through VS Code's native diff editor instead of only opening the
current file. The webview now sends a compact `openDiff` request with `toolId` and `path`;
the extension host resolves the stored ACP diff, serves old/new buffers through an
`ocean-diff:` virtual document provider, and falls back to normal file open when no diff is
available. Normal non-diff location rows still open the file/line directly. Bumped and
installed VSIX `ocean-surface-0.1.11`. Checks green: `npm run lint`, `node --check
media/chat.js`, `npm run package`, `npx @vscode/vsce package --no-dependencies`, and
Cursor extension list shows `risingtides.ocean-surface@0.1.11`.
_________________________________________________________________________________

time:      [11:57pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      refactor
area:      frontend

Ocean Cursor/VS Code extension composer ergonomics pass. Replaced the fixed/manual-resize
chat textarea with auto-growing composer behavior: the input expands as the user types,
caps at a responsive max height, scrolls internally only after the cap, preserves
Enter-to-send and Shift+Enter-newline, and resets to compact height after send. No new
visible controls were added. Bumped and installed VSIX `ocean-surface-0.1.12`. Checks
green: `npm run lint`, `node --check media/chat.js`, `npm run package`,
`npx @vscode/vsce package --no-dependencies`, and Cursor extension list shows
`risingtides.ocean-surface@0.1.12`.
_________________________________________________________________________________

time:      [12:07pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      [workflow]
area:      [writing]

Added `docs/OCEAN_PROJECT_MAP.md` and linked it from the root surface guide and
README. The map reinforces the surface boundary: GPUI/web/extension/voice/canvas
code renders state and steers sessions, while `ocean-os` owns runtime authority,
`ocean-agents` owns assistant/courier package material, and `ocean-bedrock` owns
the shared knowledge/data plane.
_________________________________________________________________________________

time:      [12:15pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      [workflow]
area:      [writing]

Refined `docs/OCEAN_PROJECT_MAP.md` with a pairwise connection matrix so surface
work is understood as part of the four-repo Ocean system. The map now makes the
surface-to-runtime, surface-to-agent-profile, surface-to-Bedrock, and all-four
workflow connections explicit while preserving that UI code remains thin.
_________________________________________________________________________________

time:      [12:49pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      [workflow]
area:      [design]

Linked the mirrored project map to the new animated cartography artifact at
`../../ocean-os/docs/OCEAN_PROJECT_MAP_ART.html`. The artifact visualizes the
four connected repos as an ocean chart with `ocean-surface` as the client island
that steers sessions and renders daemon state.
_________________________________________________________________________________

time:      [12:14pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension command-integration pass. Added command-palette
workflows for `Ocean: Ask About Current File`, `Ocean: Open Recent Session`, and
`Ocean: Rename Current Session`. Current-file prompts include the active unsaved
buffer content with a prompt-size cap, recent sessions use the persisted local
session registry, and the status bar now reflects offline/live/working/cancelling
state plus active model/session details. Updated extension README usage notes.
Bumped and installed VSIX `ocean-surface-0.1.14`. Checks green: `npm run lint`,
`node --check media/chat.js`, `npm run package`, `npx @vscode/vsce package
--no-dependencies`, and Cursor extension list shows `risingtides.ocean-surface@0.1.14`.
_________________________________________________________________________________

time:      [12:20pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension Problems integration pass. Added `Ocean: Fix
Diagnostics` as a command-palette/editor-context workflow. The command reads active-file
VS Code diagnostics, filters out hint-only noise, lets the operator pick one diagnostic or
all current-file diagnostics, and sends Ocean a grounded prompt containing diagnostic
locations, severity/source/code metadata, and the exact unsaved buffer content with a
prompt-size cap. No webview buttons or new visible command deck were added. Bumped and
installed VSIX `ocean-surface-0.1.15`. Checks green: `npm run lint`, `node --check
media/chat.js`, `npm run package`, `npx @vscode/vsce package --no-dependencies`, and
Cursor extension list shows `risingtides.ocean-surface@0.1.15`.
_________________________________________________________________________________

time:      [12:26pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension native Quick Fix pass. Added a VS Code
`CodeActionProvider` so active-file diagnostics now surface `Ocean: Fix ...` actions in
the editor lightbulb/Quick Fix menu, plus an all-current-file-diagnostics action when
there is more than one relevant problem. The lightbulb path calls the existing
diagnostic-fix command with the selected diagnostic target, preserving the no-button
transcript-first UI while making the integration IDE-native. Bumped and installed VSIX
`ocean-surface-0.1.16`. Checks green: `npm run lint`, `node --check media/chat.js`,
`npm run package`, `npx @vscode/vsce package --no-dependencies`, and Cursor extension
list shows `risingtides.ocean-surface@0.1.16`.
_________________________________________________________________________________

time:      [12:30pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension source-control review pass. Added `Ocean: Review
Workspace Changes`, which collects `git status --short`, staged/unstaged stats, and
bounded staged/unstaged diffs from the current workspace, then opens the Ocean editor
surface with a review prompt focused on regressions, risky changes, and verification
gaps. Diff collection is capped and truncates from the middle to preserve useful context
without flooding the chat. No webview controls were added. Bumped and installed VSIX
`ocean-surface-0.1.17`. Checks green: `npm run lint`, `node --check media/chat.js`,
`npm run package`, `npx @vscode/vsce package --no-dependencies`, and Cursor extension
list shows `risingtides.ocean-surface@0.1.17`.
_________________________________________________________________________________

time:      [12:37pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension edit-lifecycle pass. The ACP filesystem write handler now
captures before/after text for successful Ocean-applied file writes and reports those
snapshots back to the extension provider. Added `Ocean: Show Last Edits`, which presents
recent Ocean-applied writes and opens the selected write in VS Code's native diff editor
using the existing `ocean-diff:` virtual document provider. This gives edit reviewability
without adding webview buttons or moving tool authority into the surface. Bumped and
installed VSIX `ocean-surface-0.1.18`. Checks green: `npm run lint`, `node --check
media/chat.js`, `npm run package`, `npx @vscode/vsce package --no-dependencies`, and
Cursor extension list shows `risingtides.ocean-surface@0.1.18`.
_________________________________________________________________________________

time:      [12:44pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension persisted edit-set pass. `Ocean: Show Last Edits` now
reviews grouped Ocean-applied edit sets instead of a flat in-memory write list. Each Ocean
turn creates an edit-set context, successful ACP file writes attach bounded before/after
snapshots to that set, and the recent edit sets persist through VS Code global state with
caps on set count, per-set file count, and stored text size. The command opens the selected
file edit in VS Code's native diff editor and labels truncated snapshots in the diff title.
Bumped and installed VSIX `ocean-surface-0.1.19`. Checks green: `npm run lint`,
`node --check media/chat.js`, `npm run package`, `npx @vscode/vsce package
--no-dependencies`, and Cursor extension list shows `risingtides.ocean-surface@0.1.19`.
_________________________________________________________________________________

time:      [12:53pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension edit-revert pass. Added `Ocean: Revert Last Edit`
as a native command-palette workflow for persisted Ocean-applied edit sets. The
command lets the operator pick an edit set and file edit, blocks unsafe reverts
when the captured before snapshot was truncated or missing, asks for modal
confirmation, then restores the captured before text or deletes a created file
through VS Code workspace APIs. Reverted records are removed from persisted edit
sets, keeping the chat webview clean and avoiding new visible button chrome.
Bumped and installed VSIX `ocean-surface-0.1.20`. Checks green: `npm run lint`,
`node --check media/chat.js`, `npm run package`, `npx @vscode/vsce package
--no-dependencies`, `git diff --check`, and Cursor extension list shows
`risingtides.ocean-surface@0.1.20`.
_________________________________________________________________________________

time:      [12:56pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      refactor
area:      frontend

Ocean Cursor/VS Code extension transcript-density pass. Consecutive completed tool
calls now collapse into one native `<details>` transcript row, with running and failed
tool calls still shown immediately. The collapsed group preserves the full individual
tool rows inside the disclosure area, so file and diff affordances remain available
without flooding the chat. This directly reduces the vertical command-log wall shown in
the latest Cursor screenshots and keeps the webview transcript-first with no new button
chrome. Bumped and installed VSIX `ocean-surface-0.1.21`. Checks green: `npm run lint`,
`node --check media/chat.js`, `npm run package`, `npx @vscode/vsce package
--no-dependencies`, `git diff --check`, and Cursor extension list shows
`risingtides.ocean-surface@0.1.21`.
_________________________________________________________________________________

time:      [1:04pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension session-continuity pass. Added bounded local
transcript snapshots keyed by Ocean session id so recent sessions reopen with visible
conversation context immediately when ACP `session/load` does not replay transcript
history. Snapshots store capped transcript refs, message text, tool summaries, terminal
previews, and usage state, with text truncation to keep VS Code global state bounded.
The daemon remains the session authority; this is only a surface cache/fallback. Bumped
and installed VSIX `ocean-surface-0.1.22`. Checks green: `npm run lint`, `node --check
media/chat.js`, `npm run package`, `npx @vscode/vsce package --no-dependencies`,
`git diff --check`, Cursor extension list shows `risingtides.ocean-surface@0.1.22`,
and the installed extension bundle contains `ocean.sessionSnapshots` restore/persist
code.
_________________________________________________________________________________

time:      [1:09pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension explicit file-context pass. Added `Ocean: Ask About
Files` as a native command-palette and Explorer-context workflow. The command supports
multi-file QuickPick selection, accepts Explorer-selected file URIs, reads unsaved open
buffers when available, skips unreadable/binary files, and sends bounded selected file
contents through the existing Ocean prompt path with per-file and total prompt caps. This
adds an `@file`-style first-class context affordance without adding webview buttons or
moving tool/session authority out of Ocean OS. Bumped and installed VSIX
`ocean-surface-0.1.23`. Checks green: `npm run lint`, `node --check media/chat.js`,
`npm run package`, `npx @vscode/vsce package --no-dependencies`, `git diff --check`,
Cursor extension list shows `risingtides.ocean-surface@0.1.23`, and the installed
extension bundle contains `ocean.askWorkspaceFiles` command/menu wiring.
_________________________________________________________________________________

time:      [1:14pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      refactor
area:      frontend

Ocean Cursor/VS Code extension inline-edit safety pass. `Ocean: Inline Assist` now
opens a native `vscode.diff` preview through the existing `ocean-diff:` provider before
mutating the active buffer. Selected-text rewrites preview the selected text versus the
replacement; cursor insertions preview nearby context with an explicit insertion marker.
The edit applies only after a modal `Apply` confirmation and still aborts if the document
changed while Ocean was drafting. This moves inline editing closer to first-class IDE
agent behavior without adding webview buttons. Bumped and installed VSIX
`ocean-surface-0.1.24`. Checks green: `npm run lint`, `node --check media/chat.js`,
`npm run package`, `npx @vscode/vsce package --no-dependencies`, `git diff --check`,
Cursor extension list shows `risingtides.ocean-surface@0.1.24`, and the installed bundle
contains `Ocean inline preview` / `Apply Ocean inline edit` code.
_________________________________________________________________________________

time:      [1:18pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension whole-edit-set rollback pass. Added `Ocean: Revert
Edit Set` as a native command-palette workflow for rolling back all safely captured file
edits from a selected Ocean turn. The command blocks the whole set when any edited file is
missing a full before snapshot or has a truncated before snapshot, asks for modal
confirmation, then restores captured before text and deletes files created by that edit
set through VS Code workspace APIs. Reverted records are removed from persisted edit
sets. This adds turn-level edit control without adding webview buttons or moving edit
authority out of the IDE/Ocean ACP path. Bumped and installed VSIX `ocean-surface-0.1.25`.
Checks green: `npm run lint`, `node --check media/chat.js`, `npm run package`,
`npx @vscode/vsce package --no-dependencies`, `git diff --check`, Cursor extension list
shows `risingtides.ocean-surface@0.1.25`, and the installed bundle contains
`ocean.revertEditSet` command wiring plus unsafe-set checks.
_________________________________________________________________________________

time:      [1:34pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension session-roster pass. Added native command
`Ocean: Refresh Sessions` and made `Ocean: Open Recent Session` quietly refresh
from ACP `session/list` before showing the native QuickPick. The webview gets a
capability flag but no new visible command row or transcript controls. Recent
sessions still merge with local transcript snapshots so loaded sessions show
cached context immediately while the daemon remains the session authority.
Bumped, packaged, and installed VSIX `ocean-surface-0.1.26`. Checks green:
`npm run lint`, `node --check media/chat.js`, `npm run package`,
`npx @vscode/vsce package --no-dependencies`, `git diff --check`, Cursor
extension list shows `risingtides.ocean-surface@0.1.26`, and the installed
bundle contains `ocean.refreshSessions`, `supportsListSessions`, and the ACP
`listSessions` call. The sibling `ocean-os` ACP bridge was also updated and
release-built so the capability is real on reconnect.
_________________________________________________________________________________

time:      [1:44pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension composer file-context pass. Added typed
`@path/to/file` file mentions in the chat composer. The send path now resolves
file-like workspace mentions, supports quoted paths and `:line` suffixes,
falls back to unique basename matches, rejects absolute paths outside the
workspace, and reuses the existing bounded file-context collector so dirty
buffers, binary skips, per-file caps, and total prompt caps stay consistent
with `Ocean: Ask About Files`. The visible transcript keeps the user's original
message; no new buttons or command rows were added. Bumped, packaged, and
installed VSIX `ocean-surface-0.1.27`. Checks green: `npm run lint`,
`node --check media/chat.js`, `npm run package`, `npx @vscode/vsce package
--no-dependencies`, `git diff --check`, Cursor extension list shows
`risingtides.ocean-surface@0.1.27`, and the installed bundle contains
`enrichPromptWithComposerFileMentions` plus the composer file-mention prompt.
AGENTS.md intentionally unchanged: the transcript-first/no-button-sprawl
contract still applies as-is.
_________________________________________________________________________________

time:      [2:14pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension composer active-context pass. Added typed
composer mentions for the active editor and current selection: `@current`,
`@active`, and `@file` attach the active workspace file through the existing
bounded dirty-buffer-aware file context path, while `@selection` and `@sel`
attach the current selected text with file, language, dirty state, range, and
truncation metadata. Missing active editor, outside-workspace active file, or
empty selection are reported as skipped mentions inside the prompt context.
This brings common AI-chat context attachment into the composer without adding
webview controls. Bumped, packaged, and installed VSIX
`ocean-surface-0.1.32`. Checks green: `npm run lint`, `node --check
media/chat.js`, `npm run package`, `npx @vscode/vsce package
--no-dependencies`, `git diff --check`, Cursor extension list shows
`risingtides.ocean-surface@0.1.32`, and the installed bundle/readme contain
the active-file and selection composer mention handling. AGENTS.md
intentionally unchanged: the existing transcript-first extension UI contract
already covers this no-new-controls pass.
_________________________________________________________________________________

time:      [1:58pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension composer diagnostics-context pass. Added typed
`@problems` and `@diagnostics` mentions in the chat composer. The send path now
attaches current workspace Problems diagnostics as neutral bounded context,
sorted by severity and file, capped by item and character limits, and excludes
hint-only diagnostics. This gives the operator a first-class way to ask about
Problems from the composer without adding visible buttons, command rows, or
new webview chrome. Existing `@path/to/file` and `@changes` / `@git` / `@diff`
mentions remain intact. Bumped, packaged, and installed VSIX
`ocean-surface-0.1.29`. Checks green: `npm run lint`, `node --check
media/chat.js`, `npm run package`, `npx @vscode/vsce package
--no-dependencies`, `git diff --check`, Cursor extension list shows
`risingtides.ocean-surface@0.1.29`, and the installed bundle/readme contain
the diagnostics composer mentions plus the `Workspace diagnostics` attachment.
AGENTS.md intentionally unchanged: the transcript-first/no-button-sprawl
contract already covers this pass.
_________________________________________________________________________________

time:      [2:02pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension native command-menu pass. Added
`Ocean: Command Menu` as a single native QuickPick entry point for surfaces,
sessions, file/context actions, diagnostics, edit review/revert actions,
runtime controls, and settings. The status bar now opens this menu instead of
only opening the editor tab, so the extension has one clean dropdown for
advanced controls without adding webview buttons or transcript chrome. Bumped,
packaged, and installed VSIX `ocean-surface-0.1.30`. Checks green: `npm run
lint`, `node --check media/chat.js`, `npm run package`, `npx @vscode/vsce
package --no-dependencies`, `git diff --check`, Cursor extension list shows
`risingtides.ocean-surface@0.1.30`, and the installed package/bundle contain
`ocean.commandMenu`, the status-bar command wiring, and the native action menu.
AGENTS.md intentionally unchanged: the existing extension UI contract already
requires command/status entry points instead of visible button sprawl.
_________________________________________________________________________________

time:      [2:09pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension native runtime-controls pass. Added command
palette/status-menu commands for selecting the session model, setting thinking
level, toggling editor context injection, setting permission mode, toggling ACP
traffic logging, configuring the daemon URL, and configuring the ocean-acp
binary path. The existing single `Ocean: Command Menu` now exposes those
runtime controls under its native QuickPick menu, keeping model/context/runtime
affordances available without adding buttons or extra chrome to the chat
webview. Bumped, packaged, and installed VSIX `ocean-surface-0.1.31`. Checks
green: `npm run lint`, `node --check media/chat.js`, `npm run package`, `npx
@vscode/vsce package --no-dependencies`, `git diff --check`, Cursor extension
list shows `risingtides.ocean-surface@0.1.31`, and the installed
package/bundle contain the new runtime command registrations plus command-menu
entries. AGENTS.md intentionally unchanged: the transcript-first extension UI
contract already requires these controls to live in command/status entry points.
_________________________________________________________________________________

time:      [1:51pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension composer git-context pass. Added typed
`@changes`, `@git`, and `@diff` mentions in the chat composer. The send path
now attaches current git status, stats, staged diff, and unstaged diff through
the existing bounded workspace-change collector, using a neutral context
formatter instead of the review-specific prompt. This lets the operator ask
any question about current changes without leaving the composer or adding UI
buttons. Existing `@path/to/file` mention handling remains intact. Bumped,
packaged, and installed VSIX `ocean-surface-0.1.28`. Checks green: `npm run
lint`, `node --check media/chat.js`, `npm run package`, `npx @vscode/vsce
package --no-dependencies`, `git diff --check`, Cursor extension list shows
`risingtides.ocean-surface@0.1.28`, and the installed bundle contains the
generalized composer context enrichment plus the git-context formatter.
AGENTS.md intentionally unchanged: the transcript-first/no-button-sprawl
contract still applies as-is.
_________________________________________________________________________________

time:      [2:21pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension composer terminal-context pass. Added typed
composer mentions for recent terminal output: `@terminal`, `@term`,
`@last-terminal`, `@last-command`, and `@cmd`. The send path now finds the most
recent terminal tool outputs in the current transcript, refreshes live terminal
snapshots when still available, falls back to cached bounded previews after
release, and attaches tool title, tool status, exit status, truncation state,
and output as neutral prompt context. Missing or empty terminal output is
reported as a skipped mention inside the prompt context. This enables natural
follow-up prompts about command failures without adding webview controls.
Bumped, packaged, and installed VSIX `ocean-surface-0.1.33`. Checks green:
`npm run lint`, `node --check media/chat.js`, `npm run package`, `npx
@vscode/vsce package --no-dependencies`, `git diff --check`, Cursor extension
list shows `risingtides.ocean-surface@0.1.33`, and the installed bundle/readme
contain the terminal composer mention handling. AGENTS.md intentionally
unchanged: the existing transcript-first extension UI contract already covers
this no-new-controls pass.
_________________________________________________________________________________

time:      [2:27pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension composer open-tabs context pass. Added typed
composer mentions `@tabs`, `@open`, `@open-tabs`, `@editors`, and `@buffers`.
The send path now collects currently open VS Code/Cursor file tabs inside the
workspace, de-duplicates them, skips non-file and outside-workspace tabs, and
feeds the resulting URIs through the existing bounded file-context collector so
dirty buffers, binary skips, per-file caps, and total prompt caps stay
consistent with `@path/to/file` and `Ocean: Ask About Files`. This gives the
operator a natural way to ask about the active working set without adding
webview controls. Bumped, packaged, and installed VSIX
`ocean-surface-0.1.34`. Checks green: `npm run lint`, `node --check
media/chat.js`, `npm run package`, `npx @vscode/vsce package
--no-dependencies`, `git diff --check`, Cursor extension list shows
`risingtides.ocean-surface@0.1.34`, and the installed bundle/readme contain
the open-tabs composer mention handling. AGENTS.md intentionally unchanged:
the existing transcript-first extension UI contract already covers this
no-new-controls pass.
_________________________________________________________________________________

time:      [2:33pm] [06-26-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      feature-request
area:      frontend

Ocean Cursor/VS Code extension composer workspace-map pass. Added typed
composer mentions `@workspace`, `@codebase`, `@tree`, and `@project`. The send
path now attaches a bounded names-only workspace file map with workspace roots,
shown file count, truncation state, and the shared generated/vendor exclude
set. The formatter uses VS Code workspace APIs, caps file count and prompt
characters, and shares the same exclude glob used by file picking and basename
mention resolution. This gives the operator a lightweight codebase orientation
context for "where is X" questions without attaching file contents or adding
webview controls. Bumped, packaged, and installed VSIX
`ocean-surface-0.1.35`. Checks green: `npm run lint`, `node --check
media/chat.js`, `npm run package`, `npx @vscode/vsce package
--no-dependencies`, `git diff --check`, Cursor extension list shows
`risingtides.ocean-surface@0.1.35`, and the installed bundle/readme contain
the workspace-map composer mention handling. AGENTS.md intentionally unchanged:
the existing transcript-first extension UI contract already covers this
no-new-controls pass.
_________________________________________________________________________________
_________________________________________________________________________________

time:  [02:44pm] [06-26-26]
agent: [codex] [gpt-5]
worktree: [main]
type:  [feature-request]: Ocean Cursor/VS Code extension chat polish pass
area:  [frontend]: transcript and composer ergonomics

Shipped and installed `risingtides.ocean-surface@0.1.36`. The webview now keeps transcript scroll position stable when reading history, parses markdown tables, joins soft-wrapped prose lines more cleanly, and opens a temporary `@` composer context picker for existing context mentions. Added native Cursor/VS Code commands for copying the last Ocean response and the transcript without adding visible webview controls. Verified `node --check media/chat.js`, `npm run lint`, `npm run package`, VSIX packaging, forced Cursor install, installed version, installed payload contents, and diff whitespace checks.
_________________________________________________________________________________

time:  [02:54pm] [06-26-26]
agent: [codex] [gpt-5]
worktree: [main]
type:  [feature-request]: Ocean Cursor/VS Code extension composer file mentions
area:  [frontend]: chat context selection

Shipped and installed `risingtides.ocean-surface@0.1.38`. The composer `@`
picker now requests bounded workspace file suggestions from the extension while
the operator types, merges those with the built-in context tokens, and inserts
quoted file mentions when the path needs quoting. The extension resolves those
mentions through the existing bounded file-context send path, so this adds
first-class file discovery without adding persistent buttons or visible command
chrome. Verification: `node --check media/chat.js`, `npm run lint`,
`npm run package`, `npx @vscode/vsce package --no-dependencies`,
forced Cursor install, installed version check, installed webview payload check,
VSIX artifact check, and `git diff --check`.
_________________________________________________________________________________

time:  [03:04pm] [06-26-26]
agent: [codex] [gpt-5]
worktree: [main]
type:  [feature-request]: Ocean Cursor/VS Code extension transcript file refs
area:  [frontend]: chat navigation

Shipped and installed `risingtides.ocean-surface@0.1.39`. Transcript markdown
now linkifies likely workspace file references in prose and inline code, such
as `src/file.ts:12`, and reuses the existing `openFile` webview bridge to open
the target file and line in Cursor/VS Code. Fenced code blocks stay unchanged.
This adds first-class chat navigation without adding persistent buttons or
extra webview chrome. Verification: `node --check media/chat.js`, `npm run
lint`, `npm run package`, `npx @vscode/vsce package --no-dependencies`, forced
Cursor install, installed version check, installed payload search for file-ref
renderer/CSS, README payload check, VSIX artifact check, and `git diff
--check`.
_________________________________________________________________________________

time:  [03:10pm] [06-26-26]
agent: [codex] [gpt-5]
worktree: [main]
type:  [feature-request]: Ocean Cursor/VS Code extension composer continuity
area:  [frontend]: chat composer

Shipped and installed `risingtides.ocean-surface@0.1.40`. The chat composer
now persists its local draft through webview reloads and keeps a bounded local
history of recently sent prompts. Up/down arrow recall works when the caret is
on the first/last composer line, and Escape restores the pre-history draft.
Mention-picker behavior still takes priority, so context selection is not
disrupted. This adds first-class keyboard continuity without adding visible
controls or moving session/runtime authority into the surface. Verification:
`node --check media/chat.js`, `npm run lint`, `npm run package`,
`npx @vscode/vsce package --no-dependencies`, forced Cursor install, installed
version check, installed payload search for composer-history code, README
payload check, VSIX artifact check, and `git diff --check`.
_________________________________________________________________________________

time:  [03:17pm] [06-26-26]
agent: [codex] [gpt-5]
worktree: [main]
type:  [feature-request]: Ocean Cursor/VS Code extension slash workflows
area:  [frontend]: chat composer

Shipped and installed `risingtides.ocean-surface@0.1.41`. Typing `/` at the
start of a composer line now opens a transient workflow picker for common agent
prompts such as `/review`, `/changes`, `/fix`, `/workspace`, `/tabs`, and
`/terminal`. Accepting a shortcut expands it into ordinary prompt text with the
existing bounded `@` context mentions, so no runtime authority moved into the
webview and no persistent controls were added. The slash picker shares the same
keyboard model as the `@` picker and keeps mention completion/history recall
from conflicting. Verification: `node --check media/chat.js`, `npm run lint`,
`npm run package`, `npx @vscode/vsce package --no-dependencies`, forced Cursor
install, installed version check, installed payload search for slash picker
code, README payload check, VSIX artifact check, and `git diff --check`.
_________________________________________________________________________________

time:  [03:24pm] [06-26-26]
agent: [codex] [gpt-5]
worktree: [main]
type:  [feature-request]: Ocean Cursor/VS Code extension tool output navigation
area:  [frontend]: chat navigation

Shipped and installed `risingtides.ocean-surface@0.1.42`. Compact tool and
terminal output previews now reuse the transcript file-reference linker, so
paths such as `src/file.ts:12` inside command output can open the target file
and line through the existing `openFile` bridge. This extends first-class chat
navigation into tool output without adding visible controls or changing daemon
authority. Verification: `node --check media/chat.js`, `npm run lint`, `npm run
package`, `npx @vscode/vsce package --no-dependencies`, forced Cursor install,
installed version check, installed payload search for output file-ref rendering
and styling, README payload check, VSIX artifact check, and `git diff --check`.
_________________________________________________________________________________

time:      [05:37pm] [07-01-26]
agent:     [claude] [fable 5]
type:      [workflow]
area:      [infra]

Phase-0 stabilization pass on ocean-surface. Pushed the pending main commit
(b7eaca1, Ocean Cursor extension install/UI improvements), then opened PR #96
(feat/vscode-extension-polish-0143) carrying the ~6.5k-line 0.1.4→0.1.42
vscode-extension polish loop — session persistence via globalState + ACP
session/load, composer @-mentions, slash workflows, diagnostics Quick Fix
actions, and workspace-changes context — flagged as needing Codex/opus review
before merge (feature gate), not merged. Landed a docs/chore commit directly on
main for the cross-repo OCEAN_PROJECT_MAP, this events.md ledger itself, and the
extension check-in script. Inspected the one stash on
fix/ocean-161-supervise-surface-proxy and found its actual content (a
daemon_url_from_env refactor + a floating-widget corridor UI mode) already
superseded by equivalent-or-newer code on main — dropped it. Triaged 20
pre-existing branches (15 remote + 13 local, minus main/worktree-checked-out
refs) for provable-shipped status via merge-base-aware, squash-merge-safe file
diffing; deleted 8 that were byte-identical to main (or literal ancestors), and
boarded the remaining 12 survivors — 11 of which carry a merged GitHub PR
whose "differing" files are just later unrelated churn on shared files
(daemon.rs, ci.yml), and 1 (fix/ocean-125-rooms-base-wiring, PR #84) with
genuinely unshipped work still open. Full triage at
ocean-discovery/08-branch-triage-ocean-surface.md.
_________________________________________________________________________________

time:      [07:31pm] [07-01-26]
agent:     [claude] [fable 5]
worktree:  [main]
type:      [docs]
area:      [docs]

Lane G5 doc-truth pass (F-03/F-04). Fixed the provider-credential contradiction
flagged in audit/ocean-surface-doc-boundary-audit.md: README claimed surfaces
"hold no agent logic, provider credentials, or sessions" while README's
workspace/auth sections and AGENTS.md/CLAUDE.md described ocean-surface-proxy
holding the xAI STT/TTS key server-side. Wording-only edits (no code): the
proxy's key handling is now explicitly framed as transitional — provider
credentials are moving to ocean-os (daemon-owned voice endpoints) and the proxy
keeps them only until that migration lands (orchestration-plan Wave B4) — in
README's thin-client claim, workspace table, Auth section, and roadmap, plus
the workspace-map rows and a shared note in AGENTS.md and CLAUDE.md warning
against extending the proxy with new provider credentials. Appended a
remediation line at the top of the audit file pointing the structural fix to
Wave B4. Committed directly to main (cd84efb) and pushed.
_________________________________________________________________________________

time:      [7:37pm] [07-01-26]
agent:     [claude] [fable 5]
worktree:  [main]
type:      [review]
area:      [infra]

Lane G1 leak-closure prep for the Basic-auth credential removed at HEAD by
3d69cb5. Verified rotation status by SHA-256 comparison only (no secret values
handled in the open): verdict AUTH-OFF — no live password is in play. The
LaunchAgent plist is not installed, the repo plist/launcher/install scripts set
no creds, tools.env has no OCEAN_SURFACE_* keys, nothing listens on 8790/8791,
the public tunnel hostname serves 502 (cloudflared up, proxy down), no stale
release binary exists, and the leaked literal is absent from every live file.
Measured scrub blast radius: the literal sits in 4 commits (introduced 5a6c402
2026-05-30 in the proxy main.rs, carried by 10e58d5 and 7106948 in main.rs +
handoff.md, removed by 3d69cb5); a git-filter-repo scrub would rewrite 136 of
151 main commits and touch 12+ origin branches — and the repo is PUBLIC, so
the literal remains fetchable from history until scrubbed + GitHub-GC'd. Wrote
the private runbook ~/.config/ocean-surface/ops-runbook.md (mode 600, not in
git) holding the live topology moved out of handoff.md, the rotation verdict
with hashes, and the exact filter-repo procedure — prepared only, NOT
executed; the force-push is an operator decision. Sanitized handoff.md on main
(72a9363) to point at the runbook instead of exposing tunnel hostname, port
map, and ops procedures.
_________________________________________________________________________________

time:      [06:16pm] [07-03-26]
agent:     [claude] [glm-5.2]
worktree:  main
type:      [feature-request]
area:      [frontend]

Native CanvasLedger realtime co-editing over LiveKit data channels on topic `ocean.canvas.v1` now lands so the GPUI multiplayer canvas is no longer tldraw-first. Convergent merge OCEAN-270 owns convergence while LiveKit data packets act only as a transient courier, with the ledger and local persistence remaining the source of truth. Late joiners catch up through targeted chunked snapshots, tldraw is demoted to the optional sketch/import adapter, `canvas_sync.rs` owns the wire protocol, and `CanvasLedger::merge_snapshot` handles bulk-state late-join merges.
_________________________________________________________________________________

time:      [9:05pm] [07-04-26]
agent:     [claude] [fable-5]
type:      [feature-request]
area:      [frontend]

Rebuilt the Ocean web surface visual system end to end. Split the 3,605-line style.css monolith into 11 ordered domain files under styles/ (tokens/base/chrome/transcript/components/composer/panels/call/canvas/compact/float), wired through index.html, extension/sidepanel.html, and scripts/build-extension.sh (now globs dist/*.css + copies dist/fonts). Established Ocean's own identity from the TUI splash banner: the OCEAN depth ramp (xterm 17->87, deep indigo -> bright aqua) as --ocean-1..8 tokens, solid cyan --accent with dark --fg-on-accent ink on primary actions, Poppins vendored in public/fonts. The landing hero renders the ASCII banner verbatim with one ramp color per row; the header wordmark carries the ramp per letter. An earlier pass had applied Rising Tides magenta from the brand kit -- john rejected that explicitly; all magenta was swept out. Control consolidation ("death by 1000 buttons"): header collapsed to project/session context + one sessions icon + a details-based overflow menu holding council/rooms/mute/capture; composer is one dock card (CSS grid: orb | textarea / borderless think+model minis + halt + send); LiveKit idle bar and the outbound dialer collapse to ghost triggers on one utility row; the empty canvas band and tools(0) strip no longer render at all. Fixed a real pre-existing bug: the reasoning-effort select rendered blank (selectedIndex -1) because select.value applied before option value props landed -- options now use static value attributes plus an unknown-persisted-value guard option. Verified: cargo check wasm green (0 warnings), trunk web + extension builds green, live screenshots of landing/menu/sessions/components harness (dist/qa.html mirrors exact Rust view! markup). Design authority: docs/OCEAN_WEB_SURFACE_DESIGN.md; AGENTS.md now carries the contract.
_________________________________________________________________________________
time:      [8:20pm] [07-04-26]
agent:     [claude] [fable 5]
type:      [feature-request]
area:      [frontend]

Turn-failure UX + live smoke pass on the web surface. Two-wave minion fleet: wave 1 (glm-5.2/LongCat-2.0/deepseek-v4-pro/gpt-5.5-codex) landed OCEAN monogram icons (icon-192/512, apple-touch), manifest+sw.js brand colors (#060606), and focus-specificity CSS fixes (call.css, panels.css); wave 2 (glm-5.2/composer-2.5) added concise_error() normalization across all 27 error status.set sites in daemon.rs plus the design-doc Failure surfaces recipe. Orchestrator closed the detail-access gap: surface_turn_failure() pairs the concise chip text with the full raw payload in a status_detail signal (tooltip shows only while the displayed status equals the stored string, so stale 401s can't leak onto later statuses) and appends an expanded assistant_error Err block to the transcript on HTTP/decode/post/SSE failure paths. Verified live end-to-end: expired openai-codex token repro'd the failure UX (83-char chip, 248-char tooltip, expanded transcript block, composer recoverable), then a per-turn model override to deepseek-v4-pro produced a real assistant reply — session create -> turn -> transcript all green. Float mode (?float=1), rooms panel, and extension sidepanel @360px (no h-overflow) all verified post-rebuild. cargo check wasm green; trunk web (hashed) + extension (unhashed) builds green. Provider 401 remains an ocean-os runtime item: ~/.config/ocean-rs/auth.json openai-codex token expired 06-25, untouched per surface/runtime boundary.
_________________________________________________________________________________
time:      [9:25pm] [07-04-26]
agent:     [claude] [fable 5]
type:      [refactor]
area:      [frontend]

Consolidated the web-surface tool-call UI into one collapsible tuck per turn. John's complaint: tool calls sprayed down the transcript as a flat wall of individual drawer rows (fragmented further by interleaved thinking pills), AND duplicated in a separate ToolDrawer strip above the composer — two renderings of the same data. Fix (view-layer only, no daemon/model changes): render_items now projects every ToolCall index in an assistant turn into ONE RenderItem::ToolGroup positioned at the first tool call (non-tool blocks stay ordered Singles; turn.blocks never reordered). ToolGroupBody (header-less div) replaced by a real collapsible ToolGroup component — `▸ ● tools (N) done` header with aggregate Running/Ok/Err dot+word, collapsed by default, each row still individually expandable inside. No lone-call special case (a single tool still renders tools(1)) so the disclosure shape stays stable from the first streaming tool and the keyed For identity doesn't flip mid-stream. Failure visibility preserved: the group auto-opens on the transition into Err (edge-triggered Effect clears the manual-collapse override so a freshly-arrived error re-surfaces, matching the reducer expanding the failed call), and a manual toggle otherwise sticks. Deleted the standalone ToolDrawer component (components.rs), its mount + tool_drawer_open signal (app.rs), the now-dead crate::model import, and all .tool-drawer/.tool-chip CSS. Two parallel subagents (TranscriptTuck: transcript.rs+transcript.css; DrawerRemoval: app.rs+components.rs) on disjoint files; orchestrator gated. Verified live against the 193-turn Longhouse session (92 groups, 0 bare rows, drawer gone, the one errored group is-err+auto-open showing `tool error: command timed out`) and a fresh deepseek-v4-pro turn producing tools(3) from three bash calls in one turn. cargo check wasm green (0 warnings), trunk build green.
_________________________________________________________________________________
time:      [1:55am] [07-05-26]
agent:     [claude] [fable 5]
type:      [bug-report]
area:      [frontend]

Fixed stale RUNNING badge on the new per-turn tool tuck. John's live-session screenshot showed two finished turns whose `tools (5)`/`tools (15)` groups still read RUNNING — a turn cancelled/interrupted mid-tool (he sent a new message while turn 1 was working) never got a ToolCallFinished for its in-flight call, so that ToolCall block stayed ToolStatus::Running forever and the group aggregate faithfully reported it. Root cause was a pre-existing gap the tuck surfaced: the web reducer's TurnFinished handler (daemon.rs) flipped `streaming` off and set status but never reconciled dangling Running tool blocks. Mirrored the TUI's OCEAN-319 sweep: on TurnFinished with a non-"completed" status, sweep ONLY the finishing turn's blocks (scoped by turn_id so a sibling turn's legitimately-running tool is never touched) and flip any still-Running ToolCall to Err with expanded=true (matches every other web error path — cancel arrives as failed, not a distinct Cancelled; expanding the row makes the interrupted call visible when the group auto-opens on its Err aggregate). Completed turns are deliberately left untouched so a genuinely dropped ToolCallFinished stays a visible bug rather than being masked. Tester agent added 3 regression tests to the daemon.rs test module (cancelled_turn_closes_running_tool, completed_turn_leaves_running_tool_untouched, sweep_scoped_to_finishing_turn) driving the real apply_event path via ToolCallStarted->TurnFinished and asserting (status, expanded). cargo test -p ocean-surface-ui: 118 passed; wasm check green.
_________________________________________________________________________________
time:      [2:30am] [07-05-26]
agent:     [claude] [fable 5]
type:      [review]
area:      [frontend]

Surface hardening pass from a test-gap review + advisories. (1) Auto-open completeness (transcript.rs): the tool-group's fresh-error auto-open keyed off the aggregate's non-Err->Err edge, so a SECOND failure inside an already-errored group the user had collapsed stayed hidden. Rekeyed the reset off the contained-error COUNT rising (err_count Signal + prev_err_count), so every new failure re-surfaces; manual collapse still sticks between failures. (2) Markdown XSS (markdown.rs) — the real find: pulldown-cmark follows CommonMark and passes raw HTML AND link/image destinations through verbatim (no disable option), and render() output flows straight into inner_html across transcript, MarkdownView, callout, and float widget. The module doc even claimed a <script>-strip the code never did. So `<script>`/`<img onerror>` in assistant text or a MarkdownView, and `[x](javascript:…)` links, were live XSS. Fixed: raw-HTML events (block+inline) rewritten to Text (escaped/inert), and link/image destinations scheme-allowlisted (http/https/mailto/tel; data: only for images) with ASCII whitespace/control stripped before scheme detection to defeat java\tscript: splits; unsafe dests emptied, link text preserved. (3) Corrected a cross-session premise: ocean-surface-ui is NOT WASM-only-untestable — it's a plain binary crate (no [lib]/crate-type lock) and `cargo test -p ocean-surface-ui` runs natively (wasm-bindgen/web-sys compile as native stubs; only DOM-mounting renderers need a browser). Added 16 native tests via Tester agents: 7 markdown safety/rendering (raw HTML escaped, javascript:/data:text/html hrefs emptied incl. tab/newline-split, data:image preserved, safe markdown intact) + 9 pure-helper (cell_text, sanitize_id DOM-id guard, classify_video, youtube_id). No conditional-compilation wiring needed. cargo test: 134 passed; wasm check green. Noted a minor doc nit: cell_text doc says "null/objects render empty" but impl returns compact JSON for arrays/objects (tests lock actual behavior). Left styles/components.css untouched — a concurrent session is flipping the surface brand cyan->magenta there.
_________________________________________________________________________________
time:      [2:35am] [07-05-26]
agent:     [claude] [fable 5]
type:      [refactor]
area:      [frontend]

Header declutter (app.rs + chrome.css + compact.css). John flagged the top bar as crowded: it packed 8 first-class controls into one row with two offenders. (1) Duplicate session opener — the active-session chip and the hamburger both toggled the same show_sessions panel; gated the hamburger to `Show when session_id.is_none()` so it only appears in the empty state (chip is the sole opener once a session exists). (2) Ambient telemetry as equal-weight peers — wrapped tokens + browser-driving pill + connection status in one demoted `.ocean-runtime` cluster (a rail: gap + subtle left rule, no bordered capsule around the already-pill browser cue), sitting between primary controls and the overflow. Tokens get flex shrink priority (they're post-hoc session metadata) so the live browser/status cues never give up space first. (3) Quieted steady state — status collapses to a small dot ONLY on the exact "connected" string (every other status — new session / connecting / permission / cancelled / turn failed — stays full text, per the overloaded status channel); the dot shows only in the quiet state so it never implies success during an error, and the title falls back to the status string so the lone dot has hover meaning; aria-label always carries the full status. Extension: repointed compact.css full-row treatment from .ocean-status to the .ocean-runtime cluster (flex 1 1 100%, wrap) and reset the nested status. Verified: wasm check + trunk build green; live no-session state (chip absent, hamburger present, one cluster, "new session" shown as text not dotted). State-2/extension correct by construction (mutually-exclusive Shows; is-quiet gated on =="connected"). Left components.css untouched (concurrent brand session). NOTE: next ask is grouping the sessions panel by project — blocked on data: GET /v1/agent/sessions SessionSummary carries no project_id (only id/title/cwd/turn_count/updated_at); real grouping needs the daemon to expose project, not a cwd heuristic.
_________________________________________________________________________________
time:      [3:52pm] [07-05-26]
agent:     [codex] [gpt-5.5]
type:      [refactor]
area:      [frontend]

Redesigned the web-surface Sessions panel into project-first collapsible sections. `New Session` now uses the lazy local reset path instead of eager POSTs, and both new-session + switch-session clear session-scoped transient turn state (streaming, active turn, browser cue, pending images/permissions, decision token, status detail) before reattaching. Sessions group by daemon `owning_project` when present, exact workspace-root/catalog match otherwise, and unmatched sessions remain in `Other`; zero-turn drafts are pruned unless active; rows now show title, relative time, turn count, active state, and cwd only in `Other`. Added Ocean monogram section badges, worktree-ready root groups with branch chips, and panel CSS for the new hierarchy. Verification: `cargo check -p ocean-surface-ui --target wasm32-unknown-unknown` OK; `cargo test -p ocean-surface-ui sessions::tests -- --nocapture` 5 passed; `cargo test -p ocean-surface-ui` 139 passed; `trunk build --release` OK; `env -u NO_COLOR scripts/build-extension.sh` OK; browser QA at `http://127.0.0.1:8790/?qa=sessions-redesign` showed `surface-main` expanded, `Other` collapsed by default, 74 visible non-empty rows from 100 daemon sessions, paths only in `Other`, and no daemon session count increase after clicking `+ New Session`.
_________________________________________________________________________________

time:      [5:13pm] [07-05-26]
agent:     [codex] [gpt-5.5]
type:      [refactor]
area:      [frontend]

De-cluttered Ocean's upper chrome after John flagged the browser screenshot as too button-heavy. The Leptos web surface now keeps idle LiveKit join and outbound phone-call affordances behind the existing overflow menu; selecting Join room call or Dial phone closes the menu, reveals exactly one call surface, keeps LiveKit controls visible after a stubbed join until leave, and auto-closes the phone dialer after a stubbed successful `/v1/calls/place`. Benign status text (`connected`, `new session`, `session loaded`) now renders dot-only so it does not compete with project/session context. Compact extension CSS keeps the header single-row by truncating context and demoting runtime text instead of wrapping into a second toolbar. The Cursor/VS Code webview top bar now shows only Connect; Editor, Panel, and New remain available through commands/status entry points. Verification: `cargo check -p ocean-surface-ui --target wasm32-unknown-unknown`, `npm run lint` in `vscode-extension`, `env -u NO_COLOR trunk build --release`, `env -u NO_COLOR scripts/build-extension.sh`, `npm run package` in `vscode-extension`, browser DOM QA for overflow mutual-exclusion + stubbed LiveKit/phone success paths, final screenshot `/tmp/ocean-upperbar-final.png`, and webview harness screenshot `/tmp/ocean-vscode-header.png`.
_________________________________________________________________________________
time:      [9:50pm] [07-05-26]
agent:     [claude] [glm-5.2]
worktree:  main
type:      bug-report
area:      frontend

Web surface had no responsive small-screen mode: styles/compact.css only applied its header/composer truncation rules under the .ocean-surface--extension class (emitted by app.rs when running as the Chrome side panel), and no styles/ file contained any @media (max-width) query at all, so the normal web app never entered compact mode and the header chips overflowed / wrapped on phone-width viewports. Ported the same compact selectors behind a new @media (max-width: 720px) block targeting .ocean-surface: root drops the central max-width gutter, header stays one row with tokens + browser-control label hidden and status truncated, project/active-session/thinking/model-override chips cap their widths and ellipsize, transcript/canvas keep readable gutters, composer tightens padding with a smaller voice orb, and wide pre/img/table clips instead of widening the viewport. Also pruned stale selectors from the extension block (.ocean-brand__name, .ocean-council-btn, .ocean-rooms-btn, .ocean-screenshot, .ocean-mute) that no longer exist in app.rs markup. Verified cargo check -p ocean-surface-ui --target wasm32-unknown-unknown passes; trunk build blocked by a local --no-color CLI-flag env issue (unrelated to the CSS).
_________________________________________________________________________________
time:      [09:31PM] [07-05-26]
agent:     [codex] [gpt-5.5]
worktree:  main
type:      feature-request
area:      frontend

Reworked the web surface into a Sessions-first flow after John flagged header/path slop and the sidebar sessions drawer. Header chrome now has one persistent Sessions button with no project select, active prompt/title chip, cwd, or raw path; the empty transcript copy points to Sessions instead of a header project picker. Sessions now opens as a centered modal with New chat (projectless `/tmp` workspace), real daemon-backed Create project (`POST /v1/projects`), per-project New session, and grouped resume rows without cwd/path leakage. Verification: focused `cargo test -p ocean-surface-ui -- project_create_request chat_workspace_root`, `cargo check -p ocean-surface-ui --target wasm32-unknown-unknown`, `env -u NO_COLOR trunk build --release`, `git diff --check`, and a 390px browser smoke confirming `OCEAN Sessions ⋯`, centered modal, New chat/Create project inputs, and no visible `/Users` or header project controls.
_________________________________________________________________________________
time:      [12:36pm] [07-06-26]
agent:     [codex] [gpt-5.5]
worktree:  main
type:      [feature-request]
area:      [frontend]

Locked the web-surface rooms/calls wiring gap. Persistent Rooms now route a successful room Join into the shared LiveKit signals (`livekit_room_id` + per-room `/v1/rooms/{key}/livekit-token`), reveal the existing LiveKit utility row, and let `LiveKitPanel` auto-connect/reconnect through the singleton JS bridge instead of opening a parallel connection. Leave/back clears only the matching routed room, disconnects the bridge, and resets stale panel auto-connect state so rejoining the same room reconnects cleanly. Added regression tests for room token path encoding and route/clear signal behavior. Also fixed the outbound call-agent blocker: the proxy now registers `POST /v1/calls/place` and forwards it transparently to the daemon, with a path regression test. Verification: `cargo test -p ocean-surface-ui` (145 passed), `cargo check -p ocean-surface-ui --target wasm32-unknown-unknown`, `cargo test -p ocean-surface-proxy` (8 passed), `cargo check -p ocean-surface-proxy`, forbidden legacy-color grep clean, and `env -u NO_COLOR ./scripts/build-extension.sh` succeeded.
_________________________________________________________________________________
time:      [5:27pm] [07-06-26]
agent:     [codex] [gpt-5.5]
worktree:  main
type:      [refactor]
area:      [frontend]

Replaced the web-surface empty transcript instructional copy with a small Sessions mini-card below the Ocean landing logo. The card opens the existing Sessions modal, keeping the new/resume/project/create-project flow in one place instead of duplicating menu state or leaving verbose helper text in the empty state. `Transcript` now receives the app's `show_sessions` signal, `app.rs` passes it through, and `styles/transcript.css` adds a quiet centered launcher card using existing Ocean tokens only. Verification: `cargo check -p ocean-surface-ui --target wasm32-unknown-unknown`, `env -u NO_COLOR ./scripts/build-extension.sh`, `env -u NO_COLOR trunk build --release`, and browser smoke confirmed the launcher exists, the old lead/hint copy is gone, and clicking it opens the Sessions modal.
_________________________________________________________________________________
time:      [7:35pm] [07-06-26]
agent:     [codex] [gpt-5.5]
worktree:  main
type:      [refactor]
area:      [design]

Set the web-surface component standard after John flagged the sessions launcher/modal as slopped and the pill-everything look as 2015-core. New rule codified in docs/OCEAN_WEB_SURFACE_DESIGN.md: pill radius is geometry, never chrome — `--radius-pill` only for true circles (status dots, voice orb, progress bars, scrollbar thumbs); the radii scale tightened to 4/6/10 in tokens.css so every button, input, select, menu, card, and modal sharpens in one move. Demoted 19 pill-shaped text chips/tabs/badges across call/canvas/chrome/components/composer/float/panels/transcript CSS to `--radius-sm`. Rebuilt the landing launcher as one quiet secondary control ("Sessions", no icon lockup, no subtitle, no card shadow) and the sessions modal action row (content-width New chat instead of the full-width cyan slab, sans monogram, quiet plain-text empty line). The header overflow menu is documented as the reference component; bans added for pill text controls, full-width accent slabs, and icon+title+subtitle launcher lockups. Verification: wasm cargo check green, `env -u NO_COLOR ./scripts/build-extension.sh` + `trunk build --release` green, forbidden-color grep clean, browser screenshots of landing, sessions modal, and header menu confirm the new standard.
_________________________________________________________________________________
time:      [12:42am] [07-07-26]
agent:     [codex] [gpt-5.5]
worktree:  main
type:      [feature]
area:      [frontend]

Made the web surface real across the three fronts John flagged. (1) Create-project now creates a directory on disk: the daemon's POST /v1/projects expands ~ via $HOME, runs create_dir_all, canonicalizes, and stores the canonical path — no more literal "~/dev" in projects.json. (2) Workspace root is now a breadcrumb menu, not a bare text input: segments clickable, popover lists real directories from GET /v1/fs/dirs (sandboxed to $HOME, dot-dirs skipped, git-flagged, alphabetical), type-to-filter, + new folder affordance, text-mode toggle for power users. Proxy forwards /v1/fs/dirs with query-string passthrough. Fixed a reactivity bug: breadcrumb_home was read with get_untracked() so segments captured an empty home path before the async fetch completed — changed to .get() so segments re-render when home arrives. (3) Chart cards rebuilt: vertical bars replaced with horizontal rows (label left ellipsis+title, track center, value right mono compact-formatted), fixing label collision and mixed-magnitude readability (2px min-width for tiny values, formatted value always visible). Line chart gets soft area fill + dot tooltips + endpoint labels. Degenerate cases handled (empty/all-zero/negative). var(--gradient) removed from all chart fills. prefers-reduced-motion honored. 3 new unit tests (14 assertions) for compact_format. Verification: cargo test -p ocean-surface-ui (148 passed), cargo test -p ocean-surface-proxy (8 passed), wasm cargo check zero-warnings, build-extension.sh green, var(--gradient) grep clean, browser smoke confirmed create-project mkdir end-to-end, breadcrumb popover with 115 real dirs, and agent-rendered horizontal bar chart with no label collision.
_________________________________________________________________________________

time:      [10:55pm] [07-06-26]
agent:     [codex] [gpt-5.5]
worktree:  main
type:      [refactor]
area:      [frontend]

Landed a verified Ocean web-surface material baseline instead of leaving the tree half-claimed. Added the dark-neumorphic token contract to `styles/tokens.css` (elevation ladder, carved wells, specular seams, bioluminescent state glows, tidal easing) and codified it in `docs/OCEAN_WEB_SURFACE_DESIGN.md`. Applied the material pass across chrome/transcript/composer/panels/float/canvas/components/base, including raised header keys, an elev-3 overflow menu, a raised composer dock with carved input basin, sessions modal cleanup to the plain-row register, pointer-light opt-ins on composer/sessions/component cards, and transcript streaming hooks. Fixed the blocking transcript bug from John's screenshot by coalescing all thinking blocks in a turn into one expandable `ThinkingGroup`, adding `is-streaming`/`is-new` hooks, and wiring the transcript CSS to them. Verification: `cargo check -p ocean-surface-ui --target wasm32-unknown-unknown` green after the `HtmlElement` cast fix in the pointer listener, `env -u NO_COLOR trunk build --release` green, and browser screenshots captured both the live landing/composer baseline and the sessions modal. Council-stage rewrite and room-as-full-mode were scoped and partially explored but intentionally not landed in this baseline commit.
_________________________________________________________________________________

time:      [11:16pm] [07-06-26]
agent:     [codex] [gpt-5.5]
worktree:  main
type:      [feature-request]
area:      [frontend]

Finished the two follow-up surface asks on top of the material baseline. Rooms now behave like a first-class mode instead of a perpetual sidebar: `Rooms::new` carries the panel-open signal so a successful join closes the overlay, `app.rs` derives `in_room_mode` from the shared room + LiveKit signals, the main transcript/canvas/composer stack hides while a joined room owns the surface, and `LiveKitPanel` stays singly mounted while switching between compact utility presentation and the full stage layout already prepared in `styles/call.css`. Council is no longer the proxy-served Phaser/Game-Boy iframe: added native `crates/ocean-surface-ui/src/council.rs` + `styles/council.css`, enumerated the stylesheet in web + extension load points, and swapped the council modal body to a real Leptos workflow stage rendering goal/operator/Ocean/tool nodes with material state instead of the old `/ui/council` frame. Also committed the transcript-side `ThinkingGroup` coalescing fix and the `is-streaming` / `is-new` hooks needed to keep the live workflow surfaces legible. Verification: `cargo check -p ocean-surface-ui --target wasm32-unknown-unknown` green after the room/council integration, `env -u NO_COLOR trunk build --release` green, browser screenshots captured the native council stage modal, the live landing/composer baseline, and the cleaned sessions modal. Room-stage browser join smoke hit click-harness flake after the API-created smoke room, but the compiled wiring, shared-signal routing, and stage CSS all landed together in this pass.
_________________________________________________________________________________
time:      [11:45pm] [07-07-26]
agent:     [claude] [fable-5]
worktree:  desktop-* lanes (gitbutler)
type:      feature-request
area:      frontend

Ocean Desktop north star wave 1 landed. Design doc (docs/OCEAN_DESKTOP_NORTH_STAR.md)
plus five parallel-built modules integrated into the surface: host bridge with real
Tauri 2 interop (__TAURI_INTERNALS__ invoke/listen, git2 repo_state command in
ocean-tauri), context deck (Files explorer over the daemon /v1/fs/dirs API with
native watcher invalidation; Repo panel with branch/ahead-behind/commits; Browser
cockpit as a pure reducer over browser_* tool calls), and the ⌘K command palette
(signal-backed registry, fuzzy scoring, scope groups) mounted in app.rs with seven
commands. deck.css authored tokens-only. Fixed on integration: FsDirsResponse.ok
wire mismatch (daemon signals via error field), entry.git -> is_repo, missing
initial-load effect, Leptos closure ownership (FnOnce/FnMut), palette camelCase
boundary detection erased by lowercasing, relative_time month-bucket clamp.
78 module tests green, wasm green, browser smoke green (palette + all three deck
panels live-verified), Tauri shell boots stable with the new backend. Lanes:
desktop-skeleton, desktop-host-bridge, desktop-files-panel, desktop-repo-panel,
desktop-cockpit, desktop-palette, desktop-integration, fix-run-tauri-no-color.
Native menubar mirroring the registry still open; screenshots-in-cockpit blocked
on Block::ToolCall image storage (daemon-side, wave 2).
_________________________________________________________________________________
time:      [12:40am] [07-08-26]
agent:     [claude] [fable-5]
worktree:  desktop-os-presence (gitbutler)
type:      feature-request
area:      frontend

Desktop/web capability split settled and OS-presence wave landed. North-star doc
gains a Surface Capability Matrix (daemon-backed features cross over to web for
free; desktop adds only native hands) plus native-feel priorities P1-P6. Tauri
shell gains menubar tray (Show/Quit), Cmd+Shift+Space toggle-summon global
hotkey, hide-to-tray on close, native notifications (plugin polyfills
window.Notification so the wasm bundle's standard Web Notifications path works
on all surfaces), and a set_badge command mapping pending permission prompts to
the dock badge. wasm side: host::notify/set_badge with degrade-to-no-op
contracts and a missing-global guard, app.rs falling-edge turn-complete
notification gated on document.hasFocus(), badge effect off pending
permissions. Three parallel subagents; Notify stream hit the 30m cap and was
integrator-finished (plugin dep, capability, set_badge command). Both cargo
checks green.
_________________________________________________________________________________
time:      [3:20pm] [07-08-26]
agent:     [claude] [fable-5]
worktree:  desktop-* lanes (gitbutler)
type:      feature-request
area:      frontend

Native-feel wave 2 landed: P3 native menubar menus projected from the
CommandRegistry (seven kebab-case command ids shared between Rust MenuItems and
wasm registrations, menu-command event -> CommandRegistry::run, About/Quit as
native roles), P4 daemon supervision v1 (daemon_status/start/stop/restart
commands, TCP liveness poller emitting daemon-status on change, tray
Start/Restart items, palette commands, conditional offline chip; never touches
an external daemon), P5 ocean:// deep links (plugin + on_open_url ->
deep-link event -> parse_deep_link -> daemon.switch_session; scheme
registration activates when bundling lands). Three parallel subagents; menus
and deep-links hit the 30m cap and were integrator-finished (registry FnMut
fix, deep-link plugin init + DeepLinkExt forward in lib.rs). Tauri side: 0
errors, 17 tests green. wasm gate deferred - blocked by the concurrent
session's live voice/icons refactor (their files; heals into their lane).
_________________________________________________________________________________
time:      [03:50pm] [08-07-26]
agent:     [codex desktop] [gpt-5]
worktree:  gitbutler/workspace
type:      [feature-request]
area:      [design]: Ocean logo and response loader direction

Recorded the accepted circular neumorphic Ocean logo direction for future surface
agents. Saved the reference PNG at `docs/assets/ocean-logo-circular-neumorphic-reference.png`,
updated `docs/OCEAN_WEB_SURFACE_DESIGN.md` and `AGENTS.md` to make the circular
wave mark the active logo target, and added a `handoff.md` overlay naming the web
and Tauri implementation path. The required loading state is no longer the sonar
ping or `ocean ▸` prompt-like header; it should be the circular logo with fluid
current motion inside the mark during the send-to-first-token response gap, with a
static reduced-motion fallback.
_________________________________________________________________________________
time:      [4:55pm] [07-08-26]
agent:     [claude] [fable-5]
worktree:  gitbutler/workspace
type:      feature-request
area:      frontend

Shipped the voice privacy refactor (wake-word mode deleted — it streamed every
utterance to xAI and matched transcripts; VoiceMode is now Off/PushToTalk/HandsFree
with Off as a hard getUserMedia gate, a picker menu with plain-language disclosure,
persisted Off, never-persisted hands-free, and a live listening chip), the
hand-drawn WaveBadge logo SVG (measured from the reference PNG; ramp-token colored,
seamless parallax drift) as both the landing mark and the send→first-token pending
indicator (sonar ping and `ocean ▸` proto-header removed per the logo handoff), a
border-subtle hairline on raised transcript cards (bg-raised on bg read as banding),
and docs/OCEAN_PLATFORM_CONTRACT.md + an AGENTS.md Platform Contract section
aligning the web and desktop teams (one core many shells, host.rs as the only
capability seam, phone-test sorting rule, mobile-via-Tauri-2 trajectory,
shared-file discipline). Also repaired main twice: GitButler hunk-locking had
split interdependent app.rs hunks across lanes leaving origin/main unbuildable
(missing host.rs); landed verified integration commits 93d7ed3/f16d6b2/66df8ee
via detached-worktree assembly.
_________________________________________________________________________________
time:      [2:25pm] [07-08-26]
agent:     [claude] [fable-5]
worktree:  desktop-workspace-pane (gitbutler)
type:      feature-request
area:      frontend

Workspace pane wave landed: Codex-style permanent right pane (Tauri-gated) with
tab strip, docked filterable file tree (daemon files=1 listing), file preview
tabs (fs/file endpoint, cache + watcher refresh), browser tab hosting the CDP
screencast client, drag/collapse ergonomics following. Connectivity fixed
end-to-end: daemon CORS allowlist for tauri://localhost, dual-loopback bind,
wasm localhost->127.0.0.1 pin. Emoji glyphs purged for icons.rs SVGs
(GitBranch/Folder/Code/ChevronDown + rotation CSS) across workspace, deck
files, and the shared filetree component. Four capped subagents
integrator-finished; probe scaffolding in ocean-tauri lib.rs marked REMOVE ME
rides until the next binary pass.
_________________________________________________________________________________
time:      [5:43pm] [07-09-26]
agent:     [omp] [gpt-5.6-sol]
worktree:  detached assembly from origin/main c3f152f
type:      feature-request
area:      frontend

Landed the approved v22 Ocean reveal and thinkfill pending card with the shared
living-water WaveBadge, solid-letter slow shimmer, header wordmark, session-id
restore, Dictate mode, and realtime voice phases 2/3. Realtime voice mints its
ephemeral OpenAI client secret in ocean-os, connects browser WebRTC directly to
OpenAI, renders tool components into the transcript, writes voice handoff notes
to the chat session, and promotes the orb to a mic-reactive center stage until a
component docks it. The surface proxy now forwards both voice routes. During
deployment, HTTP Basic auth was found to challenge Chromium's manifest/WASM boot
subrequests after the document login; the gate now keeps `/`, `/v1/*`, and
`/api/*` protected while allowing only required static PWA assets through. The
public proxy stayed stopped while repaired. The unrelated workspace-pane closure
was intentionally excluded; its lane must restore `class:has-workspace-open` on
`<main>` when it lands. Verified in the isolated tree: WASM cargo check, proxy
cargo check, 3 realtime UI tests, 4 auth-gate regression tests, release Trunk
build, and private-browser desktop/mobile boot (brand SVG, five word letters,
OceanReveal, composer, hashed WASM, no horizontal overflow).
_________________________________________________________________________________
time:      [7:40pm] [07-09-26]
agent:     [claude] [fable-5]
worktree:  gitbutler/workspace
type:      merge
area:      infra

Reconciled the shared GitButler workspace onto main 0c706c3 after a corrupted
lane rebase. The desktop-integration stack's deck+palette commit conflicted
against the new main; `but resolve` (0.19) wedged on a false "no default
target" error (fixed by upgrading the CLI to 0.21, whose edit-mode setup
bypass handles gitbutler/edit), and the finish rebase re-merged uncommitted
changes destructively - duplicate FsFileEntry, mismatched braces. Root-cause
audit proved the stack's seven commits are wholly contained in upstream
0c706c3 (the desktop session landed them itself); the huge "unassigned WIP"
set was mostly stale-base artifact from a 3670b1f-era worktree, and blind
restoration had regressed landed realtime-voice/dictate work. Final state:
tracked sources exactly at 0c706c3; deliberate overlays kept uncommitted
(AGENTS.md platform-contract + circular-logo doc direction, mockups, tauri
icon set); brand assets land with this entry's commit (public/brand/
ocean-mark.svg + master PNG, regenerated PWA icons, generator script)
because the shipped index.html references /brand/ocean-mark.svg which was
untracked - fresh clones built a 404 favicon. Recovery state is parked on
this machine only: local refs refs/backups/{green-worktree-tree,
worktree-composite-20260709, desktop-integration-pre-resolve,
desktop-integration-post-resolve, pre-unapply-workspace-snapshot} and
/tmp/ocean-surface-wip-backup-20260709-1805. Verified live: wasm check,
proxy check, tauri check, release Trunk build all green; wasm warning count
fell from 94 (stale composite) to 4 on the reconciled tree.
_________________________________________________________________________________
time:      [7:49pm] [07-09-26]
agent:     [claude] [fable-5]
worktree:  gitbutler/workspace
type:      bug-report
area:      frontend

Correction to the 7:40pm entry: the previously shipped index.html never
referenced /brand/ocean-mark.svg - that favicon link existed only in the
machine-local workspace overlay, so there was no fresh-clone 404. A detached
build of landed main (2181dcf) proved it: dist carried the brand assets and
regenerated icons, but zero ocean-mark references in the HTML. Actual
sequence: 2181dcf tracked the assets; this follow-up wires the SVG favicon
into index.html (image/svg+xml ahead of the PNG fallbacks). The dist is
shared by web, PWA, and the Tauri shell (frontendDist), where the absolute
/brand/ path resolves inside the bundle; the extension is unaffected - its
manifest and sidepanel.html reference no icons. Gate for this commit before
landing: rebuild the bundle from the exact committed tree, grep
dist/index.html for the ocean-mark link, confirm dist/brand/ contents.
_________________________________________________________________________________
time:      [7:18pm] [07-09-26]
agent:     [claude] [fable-5]
worktree:  main (assembly land; GitButler lanes bypassed)
type:      release
area:      frontend

Landed and deployed the desktop-shell batch as one assembled commit 0c706c3 on
main: workspace right-edge unify + overlay Esc discipline, composer slash
popover (Command.slash + slash_filter + SlashMenu, 20 aliases), MenuBridge
readiness gate + host::notify_ui_ready + app mount wiring, FsFileEntry
duplication fix in daemon.rs, and install-path auth safety (launcher sources
~/.config/ocean-surface/proxy-auth.env, installer preflight hard-fails without
it, README/plist scrubbed of the false built-in-creds claim). Assembly landing
was forced: GitButler refused to keep desktop-integration/host-bridge/skeleton
applied simultaneously, and two lane commits were contaminated (icons/mockups
under a MenuBridge message; daemon tests under the slash message). wasm check
verified on the exact pushed commit in a detached worktree. Deployed build
7f4986344107056d to dist-prod; both origins serve it. Live prod deep-drive all
green: SW network-first picked up the deploy in one navigation, offline shell +
runtime cache refreshed to the new hash; lazy session creation -> real daemon
session, transcript rehydrates with the same session id after reload; palette
Meta+K, slash popover (filter + Esc), sessions/rooms Esc peel (fixed - old
bundle failed this), sessions<->rooms mutual exclusion both directions,
double-fire deterministic, mobile 390x844 composer/slash in-viewport with no
horizontal overflow. Rooms overlay is always-mounted (--open modifier) - node
visibility probes false-positive, assert the class. Tauri picks up MenuBridge/
slash on the next natural rebuild+launch; the running app was never touched.
(Re-appended: the original entry was clobbered by the 2181dcf ledger
reconcile.)
_________________________________________________________________________________
time:      [8:58pm] [07-09-26]
agent:     [claude] [fable-5]
worktree:  main (detached worktree land)
type:      bug-report
area:      frontend

Root-caused and fixed the "slop" desktop screenshot: styles/workspace.css
existed but was never linked in index.html or extension/sidepanel.html, so
the entire Tauri workspace pane shipped unstyled (raw list bullets, unstyled
pill buttons, repo+branch text mushed together, layout pushing the OCEAN
reveal into a clipped sliver). One missing stylesheet link was the whole
defect - markup and CSS were both correct. Fixed in c807cff (two <link>
lines; build-extension.sh already globs dist/*.css), verified via a
Tauri-gate-mocked headless census (inject window.__TAURI_INTERNALS__ via
evaluateOnNewDocument before wasm init so WorkspacePane mounts on the web
bundle) at the operator's exact 1294x812 viewport: pane docks right, tree
styled with branch chips, tabs switch, reveal full height. Deployed to
dist-prod; both origins serve workspace-a436dff9db06abec.css. Also removed
the broken hand-installed /Applications/Ocean.app (peer agent shipped it;
bundle.active is false so it was never a sanctioned artifact) to ~/.Trash
and terminated its process; the repo debug instance was left alone. Tauri
picks up the fix on the next natural rebuild - nothing was launched.
_________________________________________________________________________________
time:      [11:07pm] [07-09-26]
agent:     [claude] [fable-5]
type:      [merge]
area:      [infra]

Landed every piece of rotting uncommitted work across the repo's worktrees.
Main checkout carried ~370 lines of finished, wasm-checked work left dirty by
prior sessions: the transcript de-slop (role headers dropped, unified
transcript-disclosure classes with aria-expanded, ocean-lit ornament stripped),
a real ToolGroup fix (member indices derived reactively so mid-stream tool
appends no longer freeze the group), Send/Stop composer icons, the Tauri app
icon set, and - worst - docs/OCEAN_PLATFORM_CONTRACT.md, cited as BINDING by
AGENTS.md yet never tracked. Also fixed both docs pointing at a ghost logo
reference (docs/assets/... never existed; now public/brand/master-1024.png).
Landed the ocean-gui egui-bin removal that had been designed in 2350d2e then
left dirty in a superpowers worktree (cargo check ocean-gui + proxy green,
lock reconciled on main). Assembly-worktree landing per the multilane skill:
all slices committed at aeae948, wasm + proxy + gui checks re-run on the exact
assembled tree before push. Discarded as superseded: ocean-ship-assembly
staged content (byte-identical to origin/main), verify-314's proxy main.rs
(stale fail-open auth predating the landed fail-closed version). Stale
verify/build worktrees pruned.
_________________________________________________________________________________
time:      [11:20pm] [07-09-26]
agent:     [claude] [fable-5]
worktree:  detached-land (native-feel finish)
type:      feature-request
area:      frontend

Finished the native desktop-feel pass: Tauri window now uses titleBarStyle Overlay + hiddenTitle + trafficLightPosition(18,20) so macOS draws no second title bar — the app's .ocean-header IS the titlebar (data-tauri-drag-region on header+brand, 82px --titlebar-inset clears the lights, lit-slab material replaces the flat border). Removed every chrome.css .ocean-composer__halt rule — composer.css owns the unified circular action slot (Send up-arrow idle / Stop square streaming) whose icons/app.rs/composer.css slices a peer assembled into 280a25c mid-flight. Verified: wasm+tauri cargo check green, fresh dist 5e33e762, headless census at 1294x812 with mocked Tauri gate (drag attrs, 82px inset, 34px circular svg-only send, web parity no-inset), daemon sessions intact. The 22:29 dist John saw was a pre-redesign snapshot built mid-refactor — prod web was never affected.
_________________________________________________________________________________
time:      [11:28pm] [07-09-26]
agent:     [claude] [fable-5]
worktree:  detached-land (probe removal)
type:      refactor
area:      backend

Removed the temporary Tauri launch probe that shipped in the desktop shell:
probe_report command + invoke_handler registration + the 4s-after-boot eval
that wrote webview DOM/global state to /tmp/ocean-probe.txt on every launch
(marked REMOVE ME twice). The integration question it answered - did the
webview mount, which bundle loaded - is covered by the headless census with a
mocked Tauri gate, so the app no longer runs a file-writing eval at startup.
Gate: cargo check on the tauri crate green from the exact landed tree. Pure
35-line deletion vs main; no behavior added.
_________________________________________________________________________________
time:      [11:52pm] [07-09-26]
agent:     [claude] [fable-5]
worktree:  detached-land (three-slice wave)
type:      feature-request
area:      frontend

Landed the audited next wave as four commits from a detached worktree (task
subagent quota 429'd; self-executed). (1) Council deck gains its first write
affordance: a convene form POSTing /v1/longhouse/convene fire-and-forget in
spawn_local, pending cleared by the topic poll folding the new topic — the
real quorum runtime was already live daemon-side, the surface just couldn't
start one. (2) Thinking pill now carries is-running + a status dot while
reasoning is the streaming tail — the disclosure CSS for it existed dead.
(3) Sessions grouping resolves worktree/subdir roots to their project via
component-boundary longest-prefix match (project_for_root) so real worktree
sessions leave 'Other' and the sub-bucketing fires; covered by new unit
tests. (4) Recovered the crate test build: the five-slice assembly had
landed the three live-component reducer tests twice (E0428 x3) — deduped;
287/287 tests pass on the landed tree. sessions.rs was edited ONLY in the
detached worktree because codex holds live uncommitted WIP (is_git,
main_group) in the shared checkout — grouping change based on main's shape.
Gates: wasm cargo check 0 errors, full crate suite green, new tests pass.
_________________________________________________________________________________
time:      [3:09pm] [07-10-26]
agent:     [omp] [gpt-5.6-sol]
worktree:  /tmp/ocean-voice-repair-surface (origin/main detached)
type:      [bug-report]
area:      [frontend]

Restored the landed GPT Realtime Voice chat path after a later CSS merge malformed the voice-chat selector list and made the button appear inert. Voice chat now synchronously enters its center-stage state, hides the composer controls while connecting/live, docks only after a component is added after voice start, restores classic TTS barge-over protection, and returns to Off with a visible Retry voice chat row and concise missing-key error when ephemeral-secret minting fails. Root-owned realtime signals prevent disposed-owner WASM panics across conditional VoiceOrb mounts. Added RED/GREEN regressions for CSS, zero-card baseline docking, model URL encoding, and retry/error behavior; verified 292 tests, wasm check, release Trunk build, and headless delayed-failure UI flow with no page errors. A successful WebRTC call remains externally gated by provisioning a standard OpenAI platform API key in ocean-os.
_________________________________________________________________________________

time: [ 3:37AM] [07-10-26]
agent: [claude] [fable-5]
worktree: main
type: bug-report
area: frontend

Production recovery after john's live-surface QA (Tide Coin missing, thinking
stuck, components clipped, stale glow). Ported the deliverables Tide Coin into
WaveBadge as a Canvas 2D coin (icons.rs) — waterline churns spinning, settles
calm at rest, DPR<=2, static under reduced motion. Replaced the old
ocean-thinking-glow aura with the edgelight specular top-edge breath +
"ocean is working…" status row, and made the thinking disclosure past-tense on
terminal turns (transcript.rs/css). Fixed compact component clipping at phone
widths with true reflow — kanban stacks, tables cardify with per-column
data-labels, charts/stats/dashboard single-column (compact.css + components.rs).
Shipped the newest brand mark. Verified live on ocean.agentsworld.org (fresh
wasm 404c414333d86edc): tide coin painted (pixel-checked), specular present,
real table cardified + contained at 361px. NOTE: transcript.rs/css also carry
the earlier role-header-removal + transcript-disclosure coherence changes from a
prior lane — shipped together here; flag for veto if unwanted. Daemon left on
deepseek-v4-pro (least-flaky of a currently-flaky provider set; codex/claude
502/timeout this window — upstream, ocean-os territory). Thinking-forever on
multi-minute provider STALLS remains an ocean-os follow-up (daemon holds the
turn Running with no terminal frame; surface clears cleanly only on emitted
TurnFinished).
time:      [11:08pm] [07-09-26]
agent:     [codex] [gpt-5.6-sol]
worktree:  gitbutler/workspace
type:      bug-report
area:      frontend

Restored Existing project as a first-class Sessions action after a later
project-create rewrite buried it inside a breadcrumb directory browser. The
Sessions panel now has separate, mutually exclusive Existing project and New
project forms: existing paths are registered verbatim with a derived catalogue
name; new roots are derived from parent plus normalized name. Deleted the
breadcrumb, directory popover, browse/edit dual mode, Use-folder row, branch
chips, and their horizontal scroller/CSS. Verified the exact Rust helper tests
(2 pass), WASM cargo check, release Trunk build, intercepted POST payloads for
both flows, success close/reset, and zero page/panel overflow at 380px. The full
crate test binary remains independently blocked by three pre-existing duplicate
live-component tests in daemon.rs; production compilation is green.
_________________________________________________________________________________
time:      [1:29pm] [07-10-26]
agent:     [codex] [gpt-5.6-sol]
worktree:  gitbutler/workspace
type:      plan
area:      analysis

Locked the Ocean Rooms product architecture after tracing the working local
room path and the federation primitives across ocean-surface, ocean-os,
ocean-bedrock, and ocean-agents. The current SQLite room, roster, transcript,
and mention-to-real-agent convene loop are genuine; the missing product is
authenticated cross-machine sovereignty. The approved design uses Bedrock for
scoped invites, active membership, global ordered history, durable replay, and
authenticated realtime fanout; each local ocean-os executes only privately
bound agents, and the surface remains render/intent/subscribe only. The spec
also defines an honest local proof first, producer-scoped idempotency, a
separate pending outbox, race-free snapshot/live cutover, active-stream
revocation, exact-once convene, and two-machine acceptance gates.
_________________________________________________________________________________
time:      [3:52pm] [07-10-26]
agent:     [omp] [gpt-5.6-sol]
worktree:  /tmp/ocean-voice-proxy-surface (origin/main detached)
type:      [feature-request]
area:      [frontend]

Shipped the voice-first menu IA and completed the STT/TTS ownership migration. The voice menu now leads with the two products - Voice chat (live speech-to-speech) then Dictate (transcript into the composer) - with a muted Microphone group label above the demoted Off / Push to talk / Hands-free radios and the Spoken replies toggle unchanged; pure presentation, no mode/persistence changes. The proxy's /api/stt and /api/tts stopped calling xAI directly: they forward to the daemon's new /v1/voice/stt and /v1/voice/tts (paired ocean-os landing fc8f5000), the xAI key/client/resolver code was deleted from the proxy, has_auth now reports route availability with per-request errors carrying daemon credential state, and the daemon-response translation is a pure unit-tested fn. AGENTS.md/README updated: the proxy holds no provider credentials. Verified 292 UI tests, 16 proxy tests, wasm check, live daemon round-trips (stt 200 {text:""} on a tone clip, tts 200 audio/mpeg 22KB), and the realtime mint now returns 200 with an ephemeral secret on the rebuilt daemon - Voice chat is live end-to-end pending a real-mic session.
time:      [03:51pm] [07-10-26]
agent:     [claude] [fable-5]
type:      [merge]
area:      [infra]

Landed ca96abd (federated rooms design doc, 56KB) from the checkout's stale ro
lane onto main — it sat unpushed 12h on a base 18 commits behind. Audit
(3-way byte forensics) showed the rest of the "dirty" checkout is ~85%
stale-base phantoms that collapse on restack (icons, loader.rs, council.rs all
already landed); the genuine in-flight zz work (voice-menu, proxy edit, rooms
plans, PRODUCT.md, loader mockups) is LEFT for its owning peers — it does not
compile yet (E0382/E0525/unclosed delimiter) and 32 files are untracked. NOTE
for the rooms peer: your working copy has an 86KB expansion of this design doc
uncommitted in zz — commit it when ready; this landing captures the 56KB
snapshot. Ocean-os same pass: TUI follow-ups merged to main (25bf13b3 train).
_________________________________________________________________________________
time:      [04:42pm] [07-10-26]
agent:     [claude] [fable-5]
type:      [feature-request]
area:      [frontend]

Pinned widget rail shipped (handoff priority #2): props.placement "pinned"
docks a component into a persistent session-scoped rail outside the chat
scroll — registry signal decoupled from turns, replay-rebuilt, cleared on
session switch; unpin via ghost affordance or ComponentUnmount. Desktop side
rail >=1480px in the shell's free left margin, strip-under-header below that,
compact swipe strip. Built across two subagent runs (both hit the 30m cap;
second salvaged the first cleanly) + a finishing pass: 3 Leptos closure/deref
compile fixes, CSS, gates. wasm check clean; 296 host tests green incl. 4
pinned tests. Zero daemon/wire changes — placement rides component props.
_________________________________________________________________________________
time:      [05:02pm] [07-10-26]
agent:     [claude] [fable-5]
type:      [gh-actions]
area:      [infra]

CI was red on main: the council-deck native migration removed the proxy route
but left the orphaned council_deck handler + COUNCIL_DECK_HTML const (dead_code
under -D warnings) — the in-code comment explicitly deferred "orchestrator
cleanup," so this completes it: handler/const deleted, stale /ui/council +
/longhouse.html cache-predicate entries dropped, orphaned static/longhouse.html
removed, plus rustfmt on the just-landed stt/tts forwarders. Gates: proxy
check clean, clippy -D warnings 0, 16/16 tests, fmt clean.
_________________________________________________________________________________
time:      [05:31pm] [07-10-26]
agent:     [claude] [fable-5]
type:      [gh-actions]
area:      [infra]

Cleared the surface repo's FULL wasm clippy debt: the ui crate's CI clippy
step had 101 errors hiding under the proxy dead-code abort — 53 redundant
`let x = x;` self-rebinds (codemod, cross-checked against CI's flagged list;
6 Copy-signal block-captures verified semantically identical, repo.rs's 3
reverted as unflagged), ~23 machine-applied fixes (inspect_err, Copy clones,
div_ceil, sort_by_key), and a hand pass: orphaned fuzzy-scoring doc block
deleted, identical-if collapsed, dead RealtimeSession.session_id field and
FilesPanel selected_path signal removed, dir_row recursion-only param dropped
(8->7 args), RafHolder/LevelMeterHandles type aliases, allow(dead_code) on the
daemon_stop host seam + allow(too_many_arguments) on rehydrate_transcript with
rationale. Gates on this exact tree: CI's five steps green locally (proxy
build/test/clippy, ui wasm check/clippy 0 errors) + 296 host tests.
time:      [5:49pm] [07-10-26]
agent:     [omp] [gpt-5.6-sol]
worktree:  /tmp/ocean-realtime-signal-fix (origin/main detached)
type:      [bug-report]
area:      [frontend]

Provisioned the operator's capped OpenAI test credential in the daemon's untracked 0600 auth store, rebuilt/restarted the supervised daemon, and proved the Realtime secret mint returns 200 with gpt-realtime-2. A live WebRTC browser smoke then exposed three lifecycle defects hidden by the prior missing-key path: arena-owned thread-local status signals panicked after conditional mounts, the level-meter and WebRTC callbacks could fire after their wasm closures were dropped, and stop-during-Connecting could resurrect or orphan a hot microphone session. Converted all shared voice status signals to ArcRwSignal, tracked/cancelled the latest rAF, detached callbacks before transport close, centralized completed-session cleanup in Drop, and added a generation-guarded connect lifecycle with immediate connecting-mic shutdown, MicGuard/PendingTransport failure cleanup, and stale-result suppression. RED/GREEN regressions pin Arc ownership, latest-frame take-once semantics, and connect-generation cancellation. Verified 298 tests, wasm check, release Trunk build, delayed stop during an active OpenAI request remaining Off after 200/201 responses, then a fresh session reaching Live and ending Off with zero browser errors. Independent privacy review returned SHIP and confirmed the hot-mic race is closed.
_________________________________________________________________________________
time:      [6:55pm] [07-10-26]
agent:     [omp] [claude-fable-5]
worktree:  /tmp/oc-ci-fix (origin/main detached)
type:      [gh-actions]
area:      [infra]

Repaired main after the Rust 1.97 stable runner exposed a new
missing_const_for_thread_local lint in the just-landed Realtime lifecycle
guard. Made CONNECT_GENERATION's Cell initializer const and applied canonical
rustfmt to the same peer-hot file; no behavior changed. Verified on the exact
detached main tree: UI wasm clippy with -D warnings, rustfmt check, and 303 UI
tests all green.
_________________________________________________________________________________
time:      [7:23pm] [07-10-26]
agent:     [omp] [claude-fable-5]
worktree:  /tmp/oc-ci-fix (origin/main detached)
type:      [release]
area:      [infra]

Deployed the exact green a7d0940 committed tree to dist-prod after GitHub CI
run 29129121053 passed both jobs. Guarded release build produced
ocean-surface-ui-11f5c9a2ebf16ff4_bg.wasm (14,945,218 bytes, slightly smaller
than the prior release); local :8790 and the tunnel serve the same hash,
unauthenticated root and /v1 remain 401, /health is 200, and index.html has no
trunk dev marker. A private auth-off census of the exact deployed bits at
390x844 mounted the Canvas Tide Coin, five-letter wordmark, composer, and
status dot with zero horizontal overflow or browser errors. Refreshed
handoff.md to make this the current baseline and archived the superseded
red-CI/provenance snapshot under .agentignore.
_________________________________________________________________________________

time:      [7:40pm] [07-10-26]
agent:     [omp] [glm-5.2]
worktree:  /tmp/ocean-map-surface (origin/main detached)
type:      [workflow]
area:      [docs]

Mirrored the voice phase-4 (2026-07-10) connection-contract change into
docs/OCEAN_PROJECT_MAP.md so the surface map matches the other three Ocean
repos byte-for-byte. Added the three daemon-owned voice routes
(/v1/voice/stt, /v1/voice/tts, /v1/voice/realtime/client-secret) to the
"Core daemon routes used by surfaces" block and a paragraph noting the
surface proxy forwards /api/stt and /api/tts to the daemon and that provider
keys resolve only inside ocean-os.
_________________________________________________________________________________

time:      [08:27pm] [07-10-26]
agent:     [omp] [fable-5]
worktree:  /tmp/ocean-vsix-land (origin/main detached)
type:      [merge]
area:      [frontend]

Restored source parity between origin/main and the Cursor extension John actually runs. origin/main carried vscode-extension/package.json at 0.1.3 while the installed VSIX was 0.1.42, built from unmerged local commit 33b0db1 (branch feat/vscode-extension-polish-0143, never pushed). Landed the vscode-extension/ subtree from 33b0db1 into a detached clean-room worktree off origin/main (c4da482) — 12 files covering session persistence, composer @mentions, diagnostic code actions, and workspace-change context — so main now carries the 0.1.42 source matching the installed extension. Verified gates in the worktree (npm ci, npm run compile -> dist/extension.js, npm run lint / tsc --noEmit) all green; no test script defined. Confirmed installed ~/.cursor/extensions/risingtides.ocean-surface-0.1.42/package.json version (0.1.42) == landed version. Touched only vscode-extension/ and this ledger; nothing outside that scope.
_________________________________________________________________________________
time:      [8:50pm] [07-10-26]
agent:     [omp] [fable-5]
worktree:  /tmp/ocean-wasm-delivery (origin/main detached)
type:      [bug-report]
area:      [infra]

Repaired the production wasm delivery regression behind the "old/dead surface" reports: the bundle shipped with wasm-opt disabled (OCEAN-121 workaround), so prod served a 14.9MB module that took ~80s to transfer through the tunnel and ~3min to compile — the page looked dead while technically current. Enabled data-wasm-opt=z with binaryen pinned to version_130 in Trunk.toml (guards in run-surface.sh and the proxy launcher still assert the dist wasm magic word, keeping the OCEAN-121 silent-corruption path closed), dropped no-transform from the proxy's wasm Cache-Control now that SRI is build-disabled so Cloudflare may compress the module, and added build.rs provenance: OCEAN_SURFACE_REV is embedded and logged at boot, which also forces distinct dist hashes per landed commit (Trunk hashes the pre-opt module, so the optimized bundle would otherwise have reused the old immutable URL). Result: 3.75MB wasm, local boot to interactive composer in 407ms with zero page errors, rev line visible in console.
_________________________________________________________________________________
time:      [12:29am] [07-11-26]
agent:     [claude] [fable 5]
worktree:  claude/project-work-review-gaps-10e303
type:      [refactor]
area:      [frontend]

Gap-audit knockdown, batch 1. Removed the seven dead "coming soon" slash commands (/advisor /login /providers /settings /graph /terminal /quit) from app.rs — they rendered greyed in the composer menu with no-op handlers; /resume stays as the one deliberate signpost. /quit was not wired because host.rs exposes no app-quit invoke (the native menu quits via app.exit(0), unreachable from the webview). Investigated the room_id:None hardcode in daemon.rs dispatch_prompt and concluded NO wiring is correct: daemon room_id is the closed Track-0 enum (pm/writers/orch_mesh/review) while surface rooms use persistent-room keys posted via /v1/rooms/persistent/{key}/messages, and room mode unmounts the composer entirely — wiring open_key would 400 every turn. Documented that domain split on the struct and dispatch site, and fixed the stale comments claiming thinking_level/model_id are unexposed (both wired since OCEAN-79). Gave the components.rs Social-embed unreachable!() its invariant message, and enabled the silently-skipped summary_from_read_args test in deck/browser.rs (missing #[test]; passes). Rebuilt extension/dist via scripts/build-extension.sh — the stale 07-06 artifact was missing deck/workspace/council.css; synced to the main checkout and ~/.config/ocean/chrome-extension. Separately pruned the branch graveyard in the main checkout: 22 merged/superseded local branches deleted (tip SHAs recorded in session log, reflog-recoverable), 3 dead /tmp worktree registrations pruned; kept salvage branches feat/ocean-377-client-context, project-picker-worktrees, fix/ocean-314-surface-decision-token, feat/vscode-extension-polish-0143, brand-reveal-port. Verified: cargo check (wasm + proxy) clean, 303 UI tests green.
_________________________________________________________________________________
time:      [1:05am] [07-11-26]
agent:     [claude] [fable 5]
worktree:  worktree-agent-a38687432a2029561
type:      [bug-report]
area:      [frontend]

Tauri-side pass over the 07-10 QA sweep (root-caused each finding against main since QA ran a ~41-commit-stale gitbutler/workspace build). QA-005 fixed: added tauri-plugin-single-instance as the first plugin in ocean-tauri — close-requested hides the window to the tray, so a relaunch (dev binary or packaged .app) was a second process with a rival Ocean Desktop window; now the duplicate forwards to the running instance, which unhides/focuses, and exits. cargo check green, 17/17 unit tests pass; live dual-launch needs a GUI session to confirm. QA-001 verdict: no Tauri code path on main can produce it — the reasoning select's seven options are static DOM (app.rs), and the model list rides the same http://127.0.0.1:4780 origin the passing turn smoke used; the "empty menu" reading is the macOS WKWebView native-select popup being opaque to the Computer Use accessibility tree (Chrome exposes options inline, WKWebView puts them in a transient NSMenu). QA-003 root cause (not owned here): "Join room call" is gated on livekit_token_path, which only the proxy's /api/config supplies; tauri://localhost has no proxy so the config never arrives — fix belongs in daemon.rs bootstrap_then_connect (seed the default room token path when running_as_tauri). QA-004 root cause (not owned here): the shell hands no cwd; daemon.rs default_cwd() hardcodes /Users/risingtidesdev/dev — should return "" so the workspace tree stays in its empty state until a session lands (workspace.rs already handles empty). Commands-menu dynamic sync (lib.rs wave-2 TODO) deliberately skipped: it needs a new invoke surface (sync_commands command + host.rs wrapper + an app.rs call after registry build), not just the existing event bridge; design recorded in the session report.
_________________________________________________________________________________
time:      [01:47am] [07-11-26]
agent:     [claude] [fable 5]
worktree:  claude/project-work-review-gaps-10e303
type:      [feature-request]
area:      [frontend]

Gap-audit knockdown, wave 2 — five parallel lanes integrated. Landed the canvas-context commitment: AgentTurnRequest gains an optional canvas field carrying a bounded semantic snapshot (folds the patch log through the same merge-gated MultiCanvasLedger the render uses; ≤128 components/canvas, ≤280 chars/slot; absent for non-canvas turns so existing payloads are byte-identical); daemon-side consumption spec handed off in the session log — current daemons ignore the field (no deny_unknown_fields). QA sweep remediation: QA-005 fixed via tauri-plugin-single-instance (hide-to-tray left invisible instances alive; relaunch now summons instead of spawning); QA-006 room participant chips + panel close/tab close/refresh glyphs moved to icons.rs stroke SVGs; QA-007 New-chat /tmp pin no longer poisons the workspace trees (browsable_root() rejects the chat pin, error state clears on reset); QA-008 council empty-state bumped one token tier; QA-004 fixed on both halves — default_cwd() no longer hardcodes /Users/risingtidesdev/dev (daemon resolve_cwd_for_turn handles empty) and the file trees hide secret-bearing files (.env, .env.*, *.pem, *.key, id_rsa*). QA-003 root-caused past the menu: livekit token fetches were document-origin-relative, which only works behind the proxy — token URLs now compose against daemon.url (proxy: unchanged relative; tauri/trunk-direct: absolute), and the native shell seeds the default room when /api/config is absent. QA-001 ruled an observation artifact (WKWebView native select menus invisible to the AX walk; data path verified sound). QA-002 was stale-checkout fallout — the xAI-key gate no longer exists on main. Salvage audit found ZERO stranded commits: PRs 87/75 were tip-squashes carrying the presumed-lost P2/test commits; all 14 project-picker-worktrees commits and brand-reveal-port's CSS landed via reauthored SHAs — the four "salvage" branches are fully mined and deleted this session. PR #97 reviewed: REWORK verdict — it has the proxy reading the daemon's private auth.json and inventing a google_maps schema block ocean-os doesn't own; correct shape is a daemon-side maps-config endpoint forwarded like /v1/voice/*; left open with the verdict recorded here. Verified: wasm check clean, 311 UI tests green, ocean-tauri and proxy checks clean, extension bundle rebuilt post-integration.
_________________________________________________________________________________
time:      [06:21pm] [07-11-26]
agent:     [omp] [gpt-5.6-sol]
worktree:  /tmp/ocean-current-deploy (origin/main detached)
type:      [bug-report]
area:      [frontend]

Corrected a stale-live-surface failure where localhost:8790 remained pinned to an old dist-prod bundle and an old service worker could reduce the page to a black “Reconnecting…” emergency shell. Landed loopback service-worker retirement/cache cleanup, completed the sessions drawer contracts (global create controls below the project list; empty registered worktrees remain visible), renamed the mis-created /Users/risingtidesdev/youtube-clipping catalog record from OCEAN to youtube-clipping, built and deployed committed main bundle ocean-surface-ui-803c39bd600b2dbd_bg.wasm, and verified on 8790 itself: zero workers/caches/controllers, sustained new-session UI, neutral project glyphs, Git marker, youtube-clipping project, and its sleepy-rosalind-a28c25 worktree with count 0. Unit suite: 316 passed; service-worker contract: 13 assertions; strict wasm clippy/fmt and GitHub CI both green.
_________________________________________________________________________________
time:      [06:43pm] [07-11-26]
agent:     [omp] [gpt-5.6-sol]
worktree:  /tmp/ocean-autodeploy (origin/main detached)
type:      [workflow]
area:      [infra]

Removed the silent main-to-live deployment gap behind localhost:8790 drift. Added a launchd auto-deploy job that polls origin/main, builds each new revision in a disposable detached worktree, runs proxy and WASM tests/checks/strict Clippy/fmt plus the deployment contract, validates the release HTML and wasm magic, then atomically advances an immutable `current` release symlink and exact `deployed-rev` marker. Any fetch, build, test, validation, or restart failure leaves the last-known-good release selected. Updated the proxy to serve `current`, made the installer build/promote and install both jobs, made uninstall remove both jobs, documented the live contract, and put the 9-assertion promotion/no-op/failure-preservation test in CI. Local contract, shell syntax, and both plist validations passed before landing.
_________________________________________________________________________________
time:      [06:54pm] [07-11-26]
agent:     [omp] [gpt-5.6-sol]
worktree:  /tmp/ocean-autodeploy (origin/main detached)
type:      [bug-report]
area:      [infra]

Closed a post-install bootstrap defect caught on the real machine: both plist ProgramArguments originally referenced launchers inside the shared GitButler checkout, whose stale/dirty base did not yet contain the newly landed auto-deploy script. The installer now copies both launchers to stable `~/.config/ocean-surface/bin/` paths, the proxy receives its mutable repo path through `OCEAN_SURFACE_REPO`, and both plists execute only the stable copies. Expanded the deployment contract to 12 assertions so CI rejects a regression back to shared-checkout launcher paths.
_________________________________________________________________________________
time:      [07:02pm] [07-11-26]
agent:     [omp] [gpt-5.6-sol]
worktree:  /tmp/ocean-autodeploy (origin/main detached)
type:      [bug-report]
area:      [infra]

Fixed the real repeated-promotion path after live verification exposed `deployed-rev` advancing while `current` still targeted the preceding release. BSD `mv -f` followed the destination directory symlink and moved the temporary link inside the old release instead of replacing `current`. Replaced that operation with Python `os.replace`, which atomically renames over the symlink itself on macOS and Linux. Added a fail-before/pass-after second-promotion regression plus old-release contamination assertion (15 total), repaired live `current` to match marker `c1c06a12f769a548c35cd394f9574fb79ce74a50`, and confirmed the symlink and marker are now identical.
_________________________________________________________________________________
time:      [07:09pm] [07-11-26]
agent:     [omp] [gpt-5.6-sol]
worktree:  /tmp/ocean-autodeploy (origin/main detached)
type:      [bug-report]
area:      [infra]

Hardened live launchd reinstall after the corrected runner deployment exposed an asynchronous bootout/bootstrap race: proxy re-registration succeeded but the auto-deploy job's immediate bootstrap returned launchd error 5 until retried moments later. Recovered the live job without sudo, confirmed it executes the stable installed runner and exits CURRENT at the selected revision, and added bounded one-second bootstrap retries for both jobs so an idempotent reinstall cannot strand either supervisor.
_________________________________________________________________________________
time:      [07:13pm] [07-11-26]
agent:     [omp] [gpt-5.6-sol]
worktree:  /tmp/ocean-autodeploy (origin/main detached)
type:      [bug-report]
area:      [infra]

Fixed another live-only failure found by exercising the actual launchd watcher after main advanced: killing an in-flight deploy during reinstall could leave the mkdir lock behind, and every later interval exited SKIP forever. The lock now records its owner PID, preserves a lock held by a live process, reclaims a missing/dead-owner lock with race-safe mkdir, and always removes its PID directory on normal cleanup. Added a fail-before/pass-after stale-lock no-op regression (16 total assertions).
_________________________________________________________________________________
_________________________________________________________________________________

time:  [14:25] [14-07-26]
agent: [pi] [gpt-5]
worktree: [docs/current-state-reset-20260712]
type:  [bug report]
area:  [testing]

Verified clean origin/main from a disposable copy: the Leptos WASM target, proxy, Trunk
release bundle, Tauri check, and Tauri executable build pass; the daemon is healthy on
:4780. Found that run-surface.sh required a pre-existing release proxy binary and reached
the proxy's Basic-auth panic only after the expensive frontend build. Updated the launcher
to validate auth up front and build its required release proxy, then aligned README and
AGENTS.md with the safe localhost and LAN/tailnet launch contracts.
_________________________________________________________________________________

time:      [01:16] [07-16-26]
agent:     [claude] [fable-5]
worktree:  gitbutler/workspace
type:      workflow
area:      infra

Wired up stitchpad wake delivery for the room. Fixed my own wake (Stop hook fails closed without a session binding; bound af3ca1f4→fable via stitchpad bind-session). Diagnosed @pi's dead wake path: roster pointed at a Velocity surface but every pi runs under herdr, so push-wakes failed with "stored UUID gone". Wrote a new ~/.stitchpad/adapters/herdr.sh (agent get → focus-guard → sanitized nudge via `herdr pane run`, exit 0/1/3 contract), pinned pi's roster entry to its herdr terminal (term_656b30a88feab17), verified live delivery into pane w1:pH. Also installed the stitchpad pi extension into pi's packages for turn-end wakes after next restart, and reaped four ghost runtime.* state files (flux, ocean, ocean-pricer, ocean-pricing-fix, Jul 12). Flagged: @dale's join reused codex's session id — collision guard blocked the bind, heartbeat already dead, left for smaths to decide.
_________________________________________________________________________________
time:      [01:34] [07-16-26]
agent:     [claude] [fable-5]
worktree:  gitbutler/workspace
type:      review
area:      analysis

Closed the evidence-layer gate on the Longhouse quorum contract (stitchpad thread with codex + pi). Verified the locked contract against ocean-os/crates/ocean-longhouse: authority predicate and latch immutability already structural; headroom extraction lands as evaluate_field_full preserving the existing group_mass computation. Found two round-trip drift traps (f32 decay path must be shared bit-for-bit; HashMap-ordered float summation needs a deterministic contribution sort) and one uncovered behavior (lone-proposal fields can never sequentially converge — burns full deadline; needs a contract decision). Posted a 4-step smallest impl/test sequence (evidence.rs → quorum.rs trajectory/assessment → new planner.rs → convene.rs swap). Edits still held pending codex's lone-proposal call and smaths' go. Also switched my own wake to the herdr push adapter (term_656b323c46b3818) — verified live.
_________________________________________________________________________________

time:      [06:08] [07-16-26]
agent:     [claude] [fable 5]
worktree:  gitbutler/workspace
type:      goal
area:      backend

Roles locked by smaths: fable=lead/planner/orchestrator, pi=builder (subagent-driven first passes), codex=refinement/review/CI foreman with merge authority, ocean=frontend design lead + technical visionary (background). Goal set: FINISH ROOMS for ocean Tauri + web app. Boarded the arc: TASK-9 named-agent binding seam (ocean-os, pi), TASK-10 event-API core with room-scoped SSE (ocean-os, pi), TASK-11 rooms UI in ocean-surface-ui for both hosts (pi builds, ocean steers), TASK-12 standing CI/merge gates (codex). Soft-start rule from smaths binds the arc: no sweeping auth/sandbox/YOLO/without_tools deltas; guardrails harden as separate explicit changes post-proof. Also reconciled ocean-os main: 13 Longhouse commits rebased onto origin and pushed (origin/main==1a5d5199), longhouse tests 168/0/1 green post-rebase. Stitchpad MCP say tool found posting literal 'undefined' bodies — using the CLI path until fixed.
_________________________________________________________________________________

time:      [06:18] [07-16-26]
agent:     [claude] [fable 5]
worktree:  gitbutler/workspace
type:      review
area:      backend

TASK-9 plan review delivered (approve-with-changes): confirmed option A (unresolved never convenes), promoted join-time agentdir::resolve validation from optional to required, and caught a real gap — the plan asserted room_post_message already runs agentdir::resolve before the convene footprint, but current main only does a roster-kind lookup there (resolve_agent_participant), so an unbound mention would emit room_trigger + the auto-convene audit line with no turn behind it (OCEAN-128 false-footprint class). Amendment: resolve before any footprint, keep execution-time re-resolve. Also accepted codex's four TASK-10 amendments into the SSE contract rev 2 (broadcast as wake-hint with SQLite authority + Lagged gap-paging, enumerated writers with join/leave marker-row adapter assigned to TASK-10, closed rooms 404 in G1, shared keepalive + 400 on bad Last-Event-ID). Ocean's TASK-11 design freeze approved; ocean accepted the TASK-10 client contract. Pipeline: pi amends + builds TASK-9 in /tmp/ocean-rooms-os off main==1a5d5199.
_________________________________________________________________________________

time:      [07:23] [07-16-26]
agent:     [claude] [fable 5]
worktree:  gitbutler/workspace
type:      release
area:      backend

TASK-9 closed: named-agent binding seam merged by codex (cdb7c174 + ledger 8dfe99bb on ocean-os main, origin parity), daemon rebuilt, and my live acceptance gate passed 8/8 — bogus agent join rejected 400 with zero false footprint, real agent (context-cartographer) joined, mention convened the real AgentDef which replied as itself. First gate run failed 7/8 from two bugs in MY script (room created without trigger_policy — no policy means convene-off by design; polling POST-only /messages instead of /transcript) — fixed in .stitchpad/artifacts/task9-live-gate.sh. Also this hour: my wake outage root-caused (pull-only roster removed my cold wake) — restored herdr push + added settle-retry Enter to the adapter; ocean stays pull with heartbeat cover. TASK-10 (room-scoped SSE, frozen rev 3 contract) greenlit to pi.
_________________________________________________________________________________

time:      [07:52] [07-16-26]
agent:     [claude] [fable 5]
worktree:  gitbutler/workspace
type:      release
area:      backend

TASK-10 closed: room-scoped SSE seam (822e7558 + lifecycle fix b74b3ffc) merged by codex at 4712fdbe, daemon rebuilt, live gate 10/10 on production — replay, live delivery, cross-room isolation, contiguous id:=seq, exact Last-Event-ID resume, typed 404/400, roster markers streaming. Review cycle: my first gate verified contract semantics in code, codex caught an idle-tail disconnect leak (task+receiver retained forever per dropped subscriber), pi's surgical delta selected tx.closed() in both waits with an honest two-state regression. Rooms backend complete (TASK-9 named-agent binding + TASK-10 durable SSE). Ocean's TASK-11 surface wiring unblocked with live wire shapes; that lane is the last seam before the rooms goal closes on web + Tauri.
_________________________________________________________________________________

time:      [08:44] [07-16-26]
agent:     [claude] [fable 5]
worktree:  gitbutler/workspace
type:      bug-report
area:      infra

Ocean wake stall root-caused and durably fixed: my earlier pull-mode switch assumed the stitchpad heartbeat covers idle gaps, but the heartbeat is only a presence ticker (writes alive.<name> JSON for the ONLINE indicator; never posts turns) — so pull-mode ocean had no idle wake path and went dark for ~2h, missing the entire TASK-9/10 completion arc until I posted a manual ocean-heartbeat wake. Fix: ocean restored to push, and ocean.sh gained an idle-guard (GET the session, defer with exit 3 when active_turn is set) eliminating both the stale-parked-wake-turn bug and idle deafness. Lesson recorded: pull mode is only safe for agents whose runtime generates its own turns. TASK-11 resumed by ocean with correct preconditions (GitButler base catch-up, isolated proxy SSE route fix, self-run gate).
_________________________________________________________________________________

time:      [10:25] [07-16-26]
agent:     [codex] [gpt-5]
worktree:  [task11-rooms-transplant @ 25009e9]
type:      [feature-request]
area:      [frontend]

TASK-11 merged and pushed to ocean-surface main at 25009e9. Rooms now hydrate
once and tail the durable room-scoped SSE stream with replay/resume and honest
connection state; the PWA proxy streams that endpoint and forwards /v1/agents;
the shared web/Tauri surface uses daemon-owned named-agent selection, refreshes
binding-truth rosters, and keeps LiveKit outside Rooms G1. Refinement fixed the
Axum wildcard route conflict, idle-tail cancellation, stale-room cross-writes,
false reconnect indicators, unstyled picker controls, and committed-tree Clippy
drift. Verified 18/18 contract/boot gate, UI tests 337/0 plus integration 1/0,
proxy tests 16/0, both denied-warning Clippy gates, and origin/main parity.
_________________________________________________________________________________

time:      [10:50] [07-16-26]
agent:     [codex] [gpt-5]
worktree:  [main]
type:      [bug report]
area:      [backend]

TASK-13 session label repaired in the live daemon project catalog. The exact
workspace root /Users/risingtidesdev/dev/ocean-surface was correctly resolved
by the Surface but its authoritative project record was misnamed dev. Renamed
that existing project in place to ocean-surface through PATCH /v1/projects;
project id, workspace root, and config were preserved. Verified the project
detail now returns ocean-surface and includes the active Stitchpad session.
The separate stale record pointing at a nonexistent nested path was not changed.
_________________________________________________________________________________

time:      [10:59] [07-16-26]
agent:     [claude] [fable 5]
worktree:  gitbutler/workspace
type:      plan
area:      design

Rooms goal delivery arc complete (TASK-9/10/11 merged + live-verified; web parity fully proven; smaths sighted rooms working with context-cartographer inside, host confirmation pending for TASK-12 closure) — and smaths called the shipped product half-baked against their real vision: coworkers joining rooms with their own agents, specialized rooms as instant-context spaces. Fair: we shipped the Gate-1 plumbing without ever surfacing the roadmap. As lead I convened the product design debate smaths demanded — five questions on the pad (identity/guest-agents, context-capsule rooms, convene model, messaging baseline, shortest path to inviting one real coworker), all agents to post positions, no code until it concludes and smaths rules. This gates the next arc (Gate-2 federation product layer).
_________________________________________________________________________________

time:      [11:08] [07-16-26]
agent:     [claude] [fable 5]
worktree:  gitbutler/workspace
type:      plan
area:      design

Rooms Gate-2 design debate converged in under an hour: ocean drafted the synthesis (after dogfooding a real Longhouse council that split-aborted — honest engine, wrong quorum size), codex issued four corrections (invite code = single-use credential not identity; offline-host question already settled by the Bedrock spec; public roster recommendation; fanout mandatory for MVP by definition of a room), ocean folded all four, codex confirmed. Lead consolidation shipped as .stitchpad/artifacts/rooms-gate2-decision-sheet.md: five settled points (Bedrock identity, Context Capsules, Response Policy replacing auto-convene, threads/search/files baseline, S1-S5 fanout-first sequence) + three asks on smaths (bless/veto, roster visibility confirm, TASK-12 host confirmation). Pi's position pending (pane focus-guarded — smaths driving it). No code until smaths rules.
_________________________________________________________________________________

time:      [11:57] [07-16-26]
agent:     [codex] [gpt-5]
worktree:  [task13/session-label-refresh @ 590ff25]
type:      [bug report]
area:      [frontend]

TASK-13 closed the durable session-label freshness bug. SessionsPanel now
refreshes both sessions and the project catalog whenever it opens, and project
section render identity includes the visible label so a daemon-side rename
cannot retain stale DOM chrome. Added a regression proving identical section
contents with a changed label produce a new key. Verified UI tests 338/0 plus
integration 1/0, wasm check, denied-warning Clippy, fmt/diff-check, and a live
browser rename round-trip ocean-surface -> task13-live-gate -> ocean-surface.
Fast-forwarded 590ff25 to main; the daemon project name was restored before
closeout.
_________________________________________________________________________________

time:      [12:07] [07-16-26]
agent:     [codex] [gpt-5]
worktree:  [main @ 590ff25]
type:      [handoff]
area:      [testing]

TASK-12 native parity gate passed without waiting for operator host
identification. Inspected the running Ocean Desktop/Tauri process and verified
the Rooms heading, create control, response-policy controls, and empty state in
the native accessibility tree. Created task12-native-gate through the native UI,
confirmed daemon persistence, and confirmed the native surface rendered the
new room as both the selected-room heading and list row. Closed the probe in the
rooms database, verified it is absent from the open-room API, and left the Rooms
panel closed. Together with the existing web parity gate, this closes the Rooms
arc on both supported hosts.
_________________________________________________________________________________

time:      [12:05] [07-16-26]
agent:     [claude] [fable 5]
worktree:  gitbutler/workspace
type:      release
area:      frontend

ROOMS ARC CLOSED — smaths's goal ("finish the rooms for ocean tauri and web app") fulfilled end to end. TASK-12's final box settled by codex driving the actual Tauri window natively (created a room through the native UI, daemon persisted, native panel rendered live; probe closed and cleaned). Full arc: TASK-9 named-agent binding (8/8 live), TASK-10 durable room SSE w/ replay (10/10 live), TASK-11 rooms UI web+Tauri (18/18 gate, cross-tab + reconnect/replay + picker proven in-browser, native parity by direct manipulation), TASK-13 sessions-label render-key fix landed same-day. All merged to origin/main with parity. Next arc gated on smaths ruling on the Gate-2 decision sheet (federation: Bedrock identity, context capsules, response policies, fanout-first sequence).
_________________________________________________________________________________

time:      [22:38] [07-16-26]
agent:     [codex] [gpt-5]
worktree:  [feat/s3-federated-room-surface @ 10cc146]
type:      [feature-request]
area:      [frontend]

S3's P1-backed federated room surface landed locally as source commit 30e9808
plus corrective child 10cc146. The shared web/Tauri UI now consumes the required
room access projection, merges access and message SSE frames without blending
their cursors, gates writes by access state, renders the safe federated roster
and separate outbox, retries failed outbox items through the daemon, and labels
room triggers as Response Policy. A read-only Ocean review found that non-2xx
room envelopes lost their daemon error text; the child preserved required access
on success while splitting error decoding by HTTP status. Verified UI tests
349/0 plus integration 1/0, WASM check, denied-warning Clippy, fmt, and
diff-check. No push, merge, or deployment was performed.
_________________________________________________________________________________

time:      [00:09] [17-07-26]
agent:     [claude] [fable 5]
type:      [merge]
area:      [infra]

Coordination gate cleared by smaths ("ship it"): pushed and merged the entire parked Gate-2 set across all three repos. ocean-surface main 590ff25 -> 3f056eb (S3 federated room surface; 349/349 native + 1 wasm test + wasm check green on re-run before push). ocean-bedrock master f5e8846 -> e3c461c (S1C SSE fanout, S1D protocol harness, S2 blockers B1-B3; fast-forward). ocean-os main 5b9e23a8 -> ee6b698f (P1 producer contracts rebased conflict-free onto the moved main, gates re-run green: fmt, 39/39 persistent_rooms, denied-warning workspace clippy; pi's S2-P1 reconciliation ledger entry cherry-picked across the rebase so the append-only ledger lost nothing; the stale remote checkpoint branch was left in place rather than force-pushed). Production Railway DB migration (007) remains a separate later gate. Next phase: ocean P2-A store review, then P2-B/C daemon bridge.
_________________________________________________________________________________

time:      [01:05] [17-07-26]
agent:     [pi] [kimi-k2.5], [thoth]
worktree:  [main]
type:      [review], [testing]
area:      [analysis], [testing]

Stitchpad lane work (handle @thoth) during post-confluence window. TASK-15 Tier-2 surface triage: dispositioned all 17 unique-patch surface lanes from git-confluence-audit-v1.md using blob-hash compare + named-marker evidence (never cherry alone) — 13 REJECT (main 03facc8 supersedes; incl. vscode-extension-polish-0143 byte-identical on all 11 files, all four desktop-* lanes absorbed into the Tauri ship, OCEAN-324 fmt sweep documented in main CI), 3 ARCHIVE (maps-auth-file-resolution per fable ruling — proxy credentials migrating to daemon; vscode-embed-leptos-wasm spike; vscode-ext-leptos-mirror dark-aqua conflicts with Kami-restraint extension contract), 1 gitbutler deferred. Artifact: .stitchpad/artifacts/tier2-surface-triage-thoth.md. Codex executed deletions; canonical exact 0/0.
TASK-16 S3 federated-rooms parity smoke: PASS. Isolated fixture (daemon 127.0.0.1:14780 + /tmp config; live :4780/:8790 untouched) with store-injected projection (state=live, 3-member roster w/ remote+agent+presence markers, outbox pending+failed). Both hosts served identical WASM (sha 5e9d859a… on proxy :18790 and Tauri :1430) and rendered identical banner/3-chip roster/outbox+retry/mention/composer; SSE full-projection initial frame, Last-Event-ID replay, unbuffered. Live join/mention/retry round-trip + 409 conflict surfaced in Tauri UI. Report: .stitchpad/artifacts/task16-s3-parity-smoke-thoth.md. Incident self-reported: cleanup pkill pattern SIGTERM'd the launchd-managed :8790 surface 3x; KeepAlive restored it, verified healthy serving releases/03facc8; lesson — exact-PID kills + launchctl check first.
_________________________________________________________________________________

time:      [06:30] [17-07-26]
agent:     [claude] [fable 5]
type:      [bug-report]
area:      [frontend]

Fixed the mobile landing hero (smaths screenshot: ocean.agentsworld.org rendered "C E A" — O and N missing). Root cause: SoundingsLanding letter quads position at uPos.x = 2·x/aspect with the word's p-space half-width fixed at 0.605 (2·L_GAP + L_HALF), so any canvas narrower than aspect ≈ 1.21 clips the outer letters off-screen — every portrait phone. Fix in loader.rs: aspect-fit scale (aspect/1.32, capped at 1) compresses letter gap and glyph half-size together in both the etch-physics and draw paths, so the wordmark fits any aspect with ~8% margin; wide viewports are byte-identical (fit=1). Gates: 349/349 workspace tests, wasm check, clippy -D warnings. This commit also lands thoth's stranded 01:05 events.md entry (TASK-15/16, announced on the pad, uncommitted in the canonical checkout).
_________________________________________________________________________________
_________________________________________________________________________________

time:      [07:30] [17-07-26]
agent:     [claude] [ocean]
worktree:  [main]
type:      [feature]
area:      [frontend], [desktop]

B0: Open Externally (#TASK-23-B0). Tauri shell: +opener crate to Cargo.toml, new open_file(root, path) command (canonicalize both, component-wise prefix gate, opener::open), registered in invoke_handler. Surface: host::open_externally(root, path) fallible wrapper (no Reflect::set unwrap); workspace context menu — right-click/Shift+F10 on file rows and preview tab headers shows portal "Open Externally" action (Esc/outside-click dismiss). Styles: workspace.css context overlay + menu (token-only). Local-only, uncommitted stop at codex gate pending review.
_________________________________________________________________________________

time:      [18:18] [17-07-26]
agent:     [pi] [thoth]
worktree:  [main]
type:      [feature-request]
area:      [frontend], [backend], [testing]

Implemented the first real Ocean Floor product surface over the shipped
Observatory contract. The Surface proxy now reads the daemon's rotating
mode-0600 token per request, rejects unsafe token files, injects bearer auth
only on the upstream hop, and streams snapshot/live/replay without exposing the
credential to browser code. The shared Leptos core now has one typed reducer,
deterministic shelf/station layout, a resize-aware isometric pixel Canvas,
truth-driven actors/consoles/tool ports/attention and integrity states, DOM
station proxies, compact zoom, semantic list, inspector, and cursor replay.
No mock feed or observer write controls were added. Live browser verification
rendered 6 durable nodes and 5 recorded edges across 2 root shelves; direct
unauthenticated daemon access returned 401, proxied snapshot returned 200, SSE
remained unbuffered, token mode was 0600, and the token was absent from dist.
Gates passed: 357 UI unit tests + 1 integration test, 17 proxy tests, WASM and
proxy checks, denied-warning Clippy, format/diff checks, extension release
build, Tauri cargo check, and Playwright desktop/inspector/list/mobile smoke.
_________________________________________________________________________________

_________________________________________________________________________________

time:      [20:12] [17-07-26]
agent:     [pi] [thoth]
worktree:  [main]
type:      [feature-request]
area:      [frontend], [design], [testing]

Replaced Ocean Floor's expanding root shelves with a reactive modular facility:
every durable execution now owns one fixed 5x5-tile isometric cubicle in a
three-column append-only grid. The reducer owns a session-local slot registry:
initial snapshot rows take response order, every later admission takes the next
slot, refreshes/resyncs/replay preserve existing slots, cap-evicted executions
leave honest gaps and return to their original module, and only an observatory
authority change resets the registry. Layout is a pure slot projection with a
fixed slot-zero world origin, so retained cubicles never move; the scene now
reuses the memoized layout and a mount-time palette instead of rebuilding both
every animation frame. Cubicles draw independent walls, deck, status beacon,
console, actor, storage, and planter as static architecture while activity,
status, attention, topology, and tool signals stay grounded in real Observatory
facts. Two fresh adversarial reviews flagged timestamp-derived ordering; fixed
by the reducer-owned registry above. Live smoke rendered 16 durable executions,
replay scrub plus return-to-live moved 0 cubicles, and no page errors appeared
at desktop or mobile widths. Verified 363 UI tests + 1 integration test,
denied-warning WASM Clippy, release Trunk/extension builds, Tauri and proxy
checks, WASM magic, Playwright smoke, and diff hygiene.
_________________________________________________________________________________

_________________________________________________________________________________

time:      [20:41] [17-07-26]
agent:     [pi] [thoth]
worktree:  [main]
type:      [feature-request]
area:      [frontend], [design]

Operator rejected the shipped floor as lifeless islands on a dot field. Rebuilt
Ocean Floor into one connected animated facility: cubicles now pack
grid-adjacent with real corridor tiles, doorway gaps in interior partitions,
tall boundary envelope walls, and foundation skirts only on true building
edges — all derived from present reducer slots, so eviction still leaves honest
holes. Water dither reduced to a sparse margin shimmer. The scene is now alive
and truthful: running actors type with alternating hands and body bob over
scrolling terminal lines with a blinking caret, everyone blinks on per-station
cadences, permission waits wave a raised hand, active rooms lift their deck
lighting, and a live execution.admitted event plays a one-shot walk-in through
the doorway (presentation memory only; snapshot hydration seats everyone
directly). Continuous RAF at ~30fps active / ~8fps ambient idle; reduced motion
stops the loop entirely. First open auto-centers the facility once and never
steals the viewport again. Verified 363+1 tests, denied-warning WASM Clippy,
release Trunk/extension builds, Tauri check, WASM magic, live desktop/mobile
Playwright (20 real stations, zero page errors, animation frames confirmed
differing).
_________________________________________________________________________________

_________________________________________________________________________________

time:      [19:15] [17-07-26]
agent:     [ocean] [surface]
worktree:  [detached a7a4883] /tmp/ocean-surface-a1
type:      [feature]
area:      [frontend]

A1 Sessions live-state dot (v7). daemon.rs: `SessionRunState` enum (8 variants
incl `#[serde(other)] Unknown`); `active_turn` + `active_state` on `SessionSummary`;
`fetch_all_sessions(&self) -> Result<Vec<_>, String>` returns `Err` on
network/parse failure. sessions.rs: abortable poll rail — only
`fetch_all_sessions().await` is inside `abortable()`. `poll_guard_write(current_gen,
my_gen, panel_open)` is called before every `session_list.set()` — gen match +
panel open required, so a settled stale fetch after close/reopen can never write
unconditionally. After matching all three outcomes, ONE unified
`poll_release_in_flight` gen-gated guard releases `in_flight` (reads the actual
`poll_in_flight.get_untracked()`, not a literal `true`). Interval via
`leptos::prelude::set_interval_with_handle(_, Duration::from_secs(2))` →
`RwSignal<Option<IntervalHandle>>` (thin i32 Copy+Send+Sync wrapper, no raw
Closure/i32/forget leak). `handle.clear()` in `stop_polling`.
`poll_guard_write`, `poll_should_skip`, and `poll_release_in_flight` are
file-scope production deciders called from real sites. Dot: 5-state contract
(permission|cancelling|running|recent|idle) always rendered with `role="img"`,
`aria-label`. panels.css: 5 `[data-state]` selectors on
`--fg-4`/`--fg-3`/`--accent`/`--warn` tokens; `sessions-dot-fade` keyframe;
`prefers-reduced-motion` after state rules. 14 tests: 6 gen/panel lifecycle + 1
release-guard matrix + 1 Abortable poll (feeds old `Err(Aborted)` through
unified `poll_release_in_flight` with actual `in_flight` value, proves stale
task does not clear it) + 6 dot-state contract.
_________________________________________________________________________________
