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
