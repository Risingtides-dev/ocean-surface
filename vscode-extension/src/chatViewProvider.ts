import * as vscode from "vscode";
import * as path from "node:path";
import { homedir } from "node:os";
import type {
  SessionNotification,
  TerminalOutputResponse,
  ToolCallContent,
  ToolCallLocation,
  ToolKind,
} from "@agentclientprotocol/sdk";
import { version as extensionVersion } from "../package.json";
import { OceanConnection } from "./connection";
import { augmentPrompt, collectEditorContext, workspaceRoot } from "./editorContext";
import type { AppliedFileEdit } from "./handlers/filesystem";
import { log, logError } from "./logger";
import {
  collectWorkspaceChanges,
  formatWorkspaceChangesContext,
  formatWorkspaceChangesPrompt,
  hasWorkspaceChanges,
} from "./workspaceChanges";

const maxPersistedEditTextChars = 180000;
const maxPersistedSessionSnapshots = 30;
const maxPersistedTranscriptItems = 80;
const maxPersistedMessageChars = 20000;
const maxPersistedToolTextChars = 40000;
const maxWorkspaceContextFiles = 12;
const maxWorkspaceFilePromptChars = 50000;
const maxWorkspaceFilesPromptChars = 180000;
const maxWorkspaceDiagnosticsPromptItems = 80;
const maxWorkspaceDiagnosticsPromptChars = 60000;
const maxComposerSelectionPromptChars = 60000;
const maxComposerTerminalPromptItems = 4;
const maxComposerTerminalPromptChars = 40000;
const maxComposerWorkspaceTreeFiles = 700;
const maxComposerWorkspaceTreeChars = 50000;
const maxComposerMentionFileSearch = 2500;
const maxComposerMentionSuggestions = 8;
const workspaceSearchExcludeGlob =
  "{**/.git/**,**/node_modules/**,**/dist/**,**/build/**,**/target/**,**/.next/**,**/out/**,**/coverage/**}";

interface ChatMessage {
  role: "user" | "assistant" | "system" | "tool";
  content: string;
  id?: string | null;
  streaming?: boolean;
}

interface ToolActivity {
  id: string;
  title: string;
  status: "pending" | "in_progress" | "completed" | "failed";
  kind?: ToolKind;
  locations?: ToolCallLocation[];
  content?: ToolCallContent[];
  terminalOutputs?: Record<string, TerminalOutputPreview>;
}

interface TerminalOutputPreview {
  output: string;
  truncated: boolean;
  exitCode?: number | null;
  signal?: string | null;
}

type TranscriptRef =
  | { type: "message"; messageIndex: number }
  | { type: "tool"; toolId: string };

interface TrackedSession {
  id: string;
  title?: string | null;
  updatedAt?: string | null;
  cwd?: string | null;
}

interface UsageSnapshot {
  used: number;
  size: number;
}

interface SessionTranscriptSnapshot {
  sessionId: string;
  updatedAt: string;
  messages: ChatMessage[];
  transcript: TranscriptRef[];
  tools: ToolActivity[];
  usage?: UsageSnapshot | null;
}

interface TrackedAppliedEdit extends AppliedFileEdit {
  id: string;
  timestamp: string;
  sessionId?: string | null;
  oldTextTruncated?: boolean;
  newTextTruncated?: boolean;
}

interface AppliedEditQuickPickItem extends vscode.QuickPickItem {
  edit: TrackedAppliedEdit;
}

interface AppliedEditSet {
  id: string;
  title: string;
  sessionId?: string | null;
  startedAt: string;
  endedAt?: string | null;
  edits: TrackedAppliedEdit[];
}

interface ActiveEditSet {
  id: string;
  title: string;
  sessionId?: string | null;
  startedAt: string;
}

interface AppliedEditSetQuickPickItem extends vscode.QuickPickItem {
  set: AppliedEditSet;
}

interface InlineAssistChoice extends vscode.QuickPickItem {
  instruction?: string;
  custom?: boolean;
}

interface SessionQuickPickItem extends vscode.QuickPickItem {
  session: TrackedSession;
}

interface DiagnosticQuickPickItem extends vscode.QuickPickItem {
  diagnostics: vscode.Diagnostic[];
  all?: boolean;
}

interface WorkspaceFileQuickPickItem extends vscode.QuickPickItem {
  uri: vscode.Uri;
  relativePath: string;
}

interface WorkspaceFilePromptContext {
  uri: vscode.Uri;
  relativePath: string;
  text: string;
  truncated: boolean;
  isDirty: boolean;
}

interface ComposerFileMention {
  raw: string;
  path: string;
  quoted: boolean;
}

interface ComposerContextAttachment {
  label: string;
  content: string;
}

interface ComposerMentionSuggestion {
  kind: "file";
  label: string;
  detail: string;
  insert: string;
}

interface WorkspaceDiagnosticPromptContext {
  relativePath: string;
  diagnostic: vscode.Diagnostic;
}

export interface DiagnosticFixTarget {
  uri?: vscode.Uri;
  diagnostics?: vscode.Diagnostic[];
  all?: boolean;
}

export interface OceanStatusSnapshot {
  connected: boolean;
  turnInProgress: boolean;
  cancelling: boolean;
  currentModelId: string | null;
  sessionId: string | null;
  sessionTitle: string | null;
}

type SurfaceMode = "sidebar" | "panel" | "editor";

interface WebviewMessage {
  type?: unknown;
  text?: unknown;
  value?: unknown;
  sessionId?: unknown;
  toolId?: unknown;
  path?: unknown;
  line?: unknown;
  requestId?: unknown;
  query?: unknown;
}

export class ChatViewProvider implements vscode.WebviewViewProvider {
  public static readonly primaryViewType = "ocean.chat";
  public static readonly panelViewType = "ocean.panelChat";
  public static readonly editorViewType = "ocean.chatEditor";

  private editorPanel: vscode.WebviewPanel | undefined;
  private readonly webviews = new Set<vscode.Webview>();
  private readonly connection: OceanConnection;
  private messages: ChatMessage[] = [];
  private transcript: TranscriptRef[] = [];
  private readonly messageTranscriptIndexes = new Set<number>();
  private readonly toolTranscriptIds = new Set<string>();
  private streamingAssistantIndex: number | null = null;
  private cancellingTurn = false;
  private readonly tools = new Map<string, ToolActivity>();
  private readonly messageIndexes = new Map<string, number>();
  private readonly diffDocuments = new Map<string, string>();
  private readonly terminalOutputCache = new Map<string, TerminalOutputPreview>();
  private readonly diffProvider: vscode.Disposable;
  private sessions: TrackedSession[] = [];
  private usage: UsageSnapshot | null = null;
  private appliedEdits: TrackedAppliedEdit[] = [];
  private editSets: AppliedEditSet[] = [];
  private activeEditSet: ActiveEditSet | null = null;
  private sessionSnapshots: SessionTranscriptSnapshot[] = [];

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly state: vscode.Memento,
    private readonly onStatusChange?: (status: OceanStatusSnapshot) => void,
  ) {
    this.sessions = sanitizeSessions(
      this.state.get<TrackedSession[]>("ocean.sessions", []),
    );
    this.editSets = sanitizeEditSets(
      this.state.get<AppliedEditSet[]>("ocean.editSets", []),
    );
    this.sessionSnapshots = sanitizeSessionSnapshots(
      this.state.get<SessionTranscriptSnapshot[]>("ocean.sessionSnapshots", []),
    );
    this.appliedEdits = flattenEditSets(this.editSets);
    this.connection = new OceanConnection(
      (update) => this.handleSessionUpdate(update),
      () => this.postState(),
      (edit) => this.recordAppliedEdit(edit),
    );
    this.diffProvider = vscode.workspace.registerTextDocumentContentProvider(
      "ocean-diff",
      {
        provideTextDocumentContent: (uri) =>
          this.diffDocuments.get(uri.toString()) ?? "",
      },
    );
  }

  resolveWebviewView(
    webviewView: vscode.WebviewView,
    _context: vscode.WebviewViewResolveContext,
    _token: vscode.CancellationToken,
  ): void {
    const surfaceMode =
      webviewView.viewType === ChatViewProvider.panelViewType ? "panel" : "sidebar";
    this.attachWebview(webviewView.webview, webviewView.onDidDispose, {
      surfaceMode,
    });
  }

  openEditorPanel(viewColumn: vscode.ViewColumn = vscode.ViewColumn.Beside): void {
    if (this.editorPanel) {
      this.editorPanel.reveal(viewColumn);
      return;
    }

    const panel = vscode.window.createWebviewPanel(
      ChatViewProvider.editorViewType,
      "Ocean",
      { viewColumn },
      {
        enableScripts: true,
        retainContextWhenHidden: true,
        localResourceRoots: [vscode.Uri.joinPath(this.extensionUri, "media")],
      },
    );
    panel.iconPath = vscode.Uri.joinPath(this.extensionUri, "media", "wave.svg");
    this.editorPanel = panel;
    this.attachWebview(panel.webview, panel.onDidDispose, {
      surfaceMode: "editor",
      onDispose: () => {
        if (this.editorPanel === panel) {
          this.editorPanel = undefined;
        }
      },
    });
  }

  async connect(): Promise<void> {
    try {
      this.pushSystem("Connecting to Ocean…");
      await this.connection.connect();
      const session = await this.connection.newSession();
      await this.recordSession({
        id: session.sessionId,
        title: "New session",
        cwd: workspaceRoot(),
      });
      const agent = this.connection.agentInfo;
      this.pushSystem(
        `Connected to ${agent?.title ?? agent?.name ?? "Ocean"}. Session ready.`,
      );
    } catch (error) {
      logError("Connect failed", error);
      this.pushSystem(`Connect failed: ${String(error)}`);
    }
  }

  async newSession(): Promise<void> {
    if (!this.connection.connected) {
      await this.connect();
      return;
    }
    try {
      const session = await this.connection.newSession();
      this.clearTranscript();
      this.usage = null;
      await this.recordSession({
        id: session.sessionId,
        title: "New session",
        cwd: workspaceRoot(),
      });
      this.pushSystem("New session started.");
    } catch (error) {
      logError("New session failed", error);
      this.pushSystem(`New session failed: ${String(error)}`);
    }
  }

  async loadSession(sessionId: string): Promise<void> {
    const trimmed = sessionId.trim();
    if (!trimmed || trimmed === this.connection.currentSessionId) {
      return;
    }
    if (this.streamingAssistantIndex !== null) {
      await vscode.window.showWarningMessage("Wait for the current Ocean turn to finish.");
      return;
    }

    try {
      if (!this.connection.connected) {
        this.pushSystem("Connecting to Ocean…");
        await this.connection.connect();
      }
      this.clearTranscript();
      this.usage = null;
      await this.connection.loadSession(trimmed);
      const restoredSnapshot = this.restoreSessionSnapshot(trimmed);
      const existing = this.sessions.find((session) => session.id === trimmed);
      await this.recordSession({
        id: trimmed,
        title: existing?.title ?? "Loaded session",
        cwd: existing?.cwd ?? workspaceRoot(),
      });
      if (!this.messages.length) {
        this.pushSystem(`Loaded session ${shortSessionId(trimmed)}.`);
      } else if (restoredSnapshot) {
        this.postState();
      } else {
        this.postState();
      }
    } catch (error) {
      logError("Load session failed", error);
      this.pushSystem(`Load session failed: ${String(error)}`);
    }
  }

  async sendPrompt(text: string, skipAugment = false): Promise<void> {
    const trimmed = text.trim();
    if (!trimmed) {
      return;
    }

    const composerPrompt = skipAugment
      ? trimmed
      : await this.enrichPromptWithComposerMentions(trimmed);
    const prompt = skipAugment ? trimmed : augmentPrompt(composerPrompt);
    try {
      await this.promptAndRender(trimmed, prompt);
    } catch {
      // promptAndRender already reports the error in the transcript.
    }
  }

  async inlineAssistSelection(): Promise<void> {
    const editor = vscode.window.activeTextEditor;
    if (!editor) {
      await vscode.window.showWarningMessage("Open a file before using Ocean inline assist.");
      return;
    }

    if (this.streamingAssistantIndex !== null) {
      await vscode.window.showWarningMessage("Ocean is already working on a turn.");
      return;
    }

    const hasSelection = !editor.selection.isEmpty;
    const trimmedInstruction = await this.pickInlineInstruction(hasSelection);
    if (!trimmedInstruction) {
      return;
    }

    const document = editor.document;
    const selection = editor.selection;
    const selectedText = document.getText(selection);
    const cursor = selection.active;
    const editRange = hasSelection ? selection : new vscode.Range(cursor, cursor);
    const prompt = hasSelection
      ? buildInlineRewritePrompt({
          file: document.uri.fsPath,
          languageId: document.languageId,
          instruction: trimmedInstruction,
          selectedText,
        })
      : buildInlineInsertPrompt({
          file: document.uri.fsPath,
          languageId: document.languageId,
          instruction: trimmedInstruction,
          context: localWritingContext(document, cursor),
        });

    await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: "Ocean inline assist",
        cancellable: false,
      },
      async () => {
        try {
          const version = document.version;
          const assistantText = await this.promptAndRender(
            hasSelection
              ? `Inline rewrite: ${trimmedInstruction}`
              : `Inline write: ${trimmedInstruction}`,
            prompt,
          );
          const replacement = normalizeInlineReplacement(assistantText);
          if (!replacement.trim()) {
            throw new Error("Ocean returned empty text.");
          }
          if (hasSelection && replacement === selectedText) {
            await vscode.window.showInformationMessage("Ocean returned the same selected text.");
            return;
          }
          const confirmed = await this.previewInlineAssistEdit({
            document,
            cursor,
            hasSelection,
            oldText: selectedText,
            newText: replacement,
          });
          if (!confirmed) {
            return;
          }
          if (document.version !== version) {
            await vscode.window.showWarningMessage(
              "The document changed while Ocean was drafting, so the inline edit was not applied.",
            );
            return;
          }
          const applied = await editor.edit(
            (editBuilder) => {
              editBuilder.replace(editRange, replacement);
            },
            { undoStopBefore: true, undoStopAfter: true },
          );
          if (!applied) {
            throw new Error("VS Code did not apply the inline edit.");
          }
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error);
          await vscode.window.showErrorMessage(`Ocean inline assist failed: ${message}`);
        }
      },
    );
  }

  async sendSelectionPrompt(): Promise<void> {
    const editor = vscode.window.activeTextEditor;
    if (!editor) {
      return;
    }
    const selection = editor.document.getText(editor.selection);
    const file = editor.document.uri.fsPath;
    await this.sendPrompt(
      `I'm looking at \`${file}\`. Explain or help with this selection:\n\n\`\`\`\n${selection}\n\`\`\``,
      true,
    );
  }

  async askCurrentFile(): Promise<void> {
    const editor = vscode.window.activeTextEditor;
    if (!editor) {
      await vscode.window.showWarningMessage("Open a file before asking Ocean.");
      return;
    }

    const instruction = await vscode.window.showInputBox({
      title: "Ocean: Ask About Current File",
      prompt: "What should Ocean do with this file?",
      placeHolder: "Review for bugs, explain the flow, suggest a refactor...",
      ignoreFocusOut: true,
    });
    const trimmedInstruction = instruction?.trim();
    if (!trimmedInstruction) {
      return;
    }

    this.openEditorPanel();
    const document = editor.document;
    try {
      await this.promptAndRender(
        `${path.basename(document.uri.fsPath)}: ${trimmedInstruction}`,
        buildCurrentFilePrompt({
          file: document.uri.fsPath,
          languageId: document.languageId,
          isDirty: document.isDirty,
          instruction: trimmedInstruction,
          text: document.getText(),
        }),
      );
    } catch {
      // promptAndRender already reports the error in the transcript.
    }
  }

  async askWorkspaceFiles(initialUris: vscode.Uri[] = []): Promise<void> {
    if (this.streamingAssistantIndex !== null) {
      await vscode.window.showWarningMessage("Ocean is already working on a turn.");
      return;
    }

    const selectedUris = initialUris.length
      ? await filterFileUris(initialUris)
      : await this.pickWorkspaceFiles();
    if (!selectedUris.length) {
      return;
    }

    const instruction = await vscode.window.showInputBox({
      title: "Ocean: Ask About Files",
      prompt: "What should Ocean do with the selected files?",
      placeHolder: "Review how these fit together, find bugs, refactor safely...",
      ignoreFocusOut: true,
    });
    const trimmedInstruction = instruction?.trim();
    if (!trimmedInstruction) {
      return;
    }

    const { files, skipped } = await collectWorkspaceFileContexts(selectedUris);
    if (!files.length) {
      await vscode.window.showWarningMessage("Ocean could not read text from the selected files.");
      return;
    }

    this.openEditorPanel();
    try {
      await this.promptAndRender(
        `Files: ${singleLine(trimmedInstruction, 64)}`,
        buildWorkspaceFilesPrompt({
          instruction: trimmedInstruction,
          files,
          skipped,
        }),
      );
    } catch {
      // promptAndRender already reports the error in the transcript.
    }
  }

  async fixDiagnostics(target?: DiagnosticFixTarget): Promise<void> {
    const document = await this.documentForDiagnosticTarget(target);
    if (!document) {
      await vscode.window.showWarningMessage("Open a file before asking Ocean to fix diagnostics.");
      return;
    }

    const diagnostics = diagnosticsForTarget(document, target);
    if (!diagnostics.length) {
      await vscode.window.showInformationMessage("Ocean found no diagnostics in the active file.");
      return;
    }

    const picked =
      target?.diagnostics?.length || target?.all
        ? { diagnostics, all: target.all === true }
        : await this.pickDiagnostics(diagnostics);
    if (!picked) {
      return;
    }

    this.openEditorPanel();
    try {
      await this.promptAndRender(
        diagnosticDisplayTitle(document, picked.diagnostics, picked.all === true),
        buildDiagnosticFixPrompt({
          file: document.uri.fsPath,
          languageId: document.languageId,
          isDirty: document.isDirty,
          text: document.getText(),
          diagnostics: picked.diagnostics,
          all: picked.all === true,
        }),
      );
    } catch {
      // promptAndRender already reports the error in the transcript.
    }
  }

  async reviewWorkspaceChanges(): Promise<void> {
    let snapshot;
    try {
      snapshot = await collectWorkspaceChanges(workspaceRoot());
    } catch (error) {
      logError("Workspace changes collection failed", error);
      const message = error instanceof Error ? error.message : String(error);
      await vscode.window.showErrorMessage(`Ocean could not read workspace changes: ${message}`);
      return;
    }

    if (!hasWorkspaceChanges(snapshot)) {
      await vscode.window.showInformationMessage("Ocean found no git changes to review.");
      return;
    }

    this.openEditorPanel();
    try {
      await this.promptAndRender(
        "Review workspace changes",
        formatWorkspaceChangesPrompt(snapshot),
      );
    } catch {
      // promptAndRender already reports the error in the transcript.
    }
  }

  async showLastEdits(): Promise<void> {
    if (!this.editSets.length) {
      await vscode.window.showInformationMessage("Ocean has not applied file edits in this window yet.");
      return;
    }

    const set = await this.pickAppliedEditSet(
      "Ocean: Edit Sets",
      "Pick an Ocean edit set to review",
    );
    if (!set) {
      return;
    }

    const edit = await this.pickAppliedEditFromSet(
      set,
      "Open an Ocean-applied edit as a native diff",
    );
    if (edit) {
      await this.openAppliedEditDiff(edit);
    }
  }

  async revertLastEdit(): Promise<void> {
    if (!this.editSets.length) {
      await vscode.window.showInformationMessage("Ocean has not applied file edits in this window yet.");
      return;
    }

    const set = await this.pickAppliedEditSet(
      "Ocean: Revert Edit Set",
      "Pick the Ocean edit set that contains the edit to revert",
    );
    if (!set) {
      return;
    }

    const edit = await this.pickAppliedEditFromSet(
      set,
      "Pick an Ocean-applied edit to revert",
    );
    if (!edit) {
      return;
    }

    if (edit.oldTextTruncated && !edit.created) {
      await vscode.window.showErrorMessage(
        "Ocean cannot safely revert this edit because the stored before snapshot was truncated.",
      );
      return;
    }
    if (!edit.created && edit.oldText === null) {
      await vscode.window.showErrorMessage(
        "Ocean cannot safely revert this edit because the before snapshot is missing.",
      );
      return;
    }

    const action = edit.created && edit.oldText === null
      ? "delete the created file"
      : "restore the captured before text";
    const confirmed = await vscode.window.showWarningMessage(
      `Revert Ocean edit to ${path.basename(edit.path)}? This will ${action}.`,
      { modal: true },
      "Revert",
    );
    if (confirmed !== "Revert") {
      return;
    }

    await this.revertAppliedEdit(edit);
  }

  async revertEditSet(): Promise<void> {
    if (!this.editSets.length) {
      await vscode.window.showInformationMessage("Ocean has not applied file edits in this window yet.");
      return;
    }

    const set = await this.pickAppliedEditSet(
      "Ocean: Revert Edit Set",
      "Pick an Ocean edit set to revert",
    );
    if (!set) {
      return;
    }

    const unsafe = set.edits
      .map((edit) => unsafeAppliedEditRevertReason(edit))
      .filter((reason): reason is string => Boolean(reason));
    if (unsafe.length) {
      await vscode.window.showErrorMessage(
        `Ocean cannot safely revert this edit set: ${unsafe.slice(0, 3).join("; ")}`,
      );
      return;
    }

    const confirmed = await vscode.window.showWarningMessage(
      `Revert Ocean edit set "${set.title}" (${set.edits.length} ${set.edits.length === 1 ? "file" : "files"})? This will restore captured before text and delete files created by the set.`,
      { modal: true },
      "Revert Set",
    );
    if (confirmed !== "Revert Set") {
      return;
    }

    const revertedIds = new Set<string>();
    try {
      for (const edit of set.edits) {
        await this.applyAppliedEditRevert(edit, false);
        revertedIds.add(edit.id);
      }
    } catch (error) {
      if (revertedIds.size) {
        this.removeAppliedEdits(revertedIds);
      }
      logError(`Revert edit set failed: ${set.id}`, error);
      const message = error instanceof Error ? error.message : String(error);
      await vscode.window.showErrorMessage(`Ocean could not finish reverting "${set.title}": ${message}`);
      return;
    }

    this.removeAppliedEdits(revertedIds);
    await vscode.window.showInformationMessage(
      `Ocean reverted ${set.edits.length} ${set.edits.length === 1 ? "file" : "files"} from "${set.title}".`,
    );
  }

  async openRecentSession(): Promise<void> {
    if (this.streamingAssistantIndex !== null) {
      await vscode.window.showWarningMessage("Wait for the current Ocean turn to finish.");
      return;
    }

    await this.refreshSessionsFromAgent({
      notifyErrors: false,
      notifyUnsupported: false,
    });

    if (!this.sessions.length) {
      const choice = await vscode.window.showInformationMessage(
        "Ocean has no recent sessions in this workspace yet.",
        "New session",
      );
      if (choice === "New session") {
        this.openEditorPanel();
        await this.newSession();
      }
      return;
    }

    const pick = await vscode.window.showQuickPick(
      this.sessions.map((session): SessionQuickPickItem => ({
        label: session.title || shortSessionId(session.id),
        description: shortSessionId(session.id),
        detail: sessionDetail(session),
        session,
      })),
      {
        title: "Ocean: Recent Sessions",
        placeHolder: "Load a previous Ocean session",
        matchOnDescription: true,
        matchOnDetail: true,
        ignoreFocusOut: true,
      },
    );
    if (!pick) {
      return;
    }

    this.openEditorPanel();
    await this.loadSession(pick.session.id);
  }

  async refreshRecentSessions(): Promise<void> {
    if (this.streamingAssistantIndex !== null) {
      await vscode.window.showWarningMessage("Wait for the current Ocean turn to finish.");
      return;
    }

    const count = await this.refreshSessionsFromAgent({
      notifyErrors: true,
      notifyUnsupported: true,
    });
    if (count !== null) {
      await vscode.window.showInformationMessage(
        `Ocean refreshed ${count} ${count === 1 ? "session" : "sessions"}.`,
      );
    }
  }

  async selectModel(): Promise<void> {
    if (!this.connection.connected || !this.connection.currentSessionId) {
      await this.connect();
    }
    const modes = this.connection.modelOptions;
    if (!modes.length) {
      await vscode.window.showInformationMessage("Ocean has no model modes to select.");
      return;
    }
    const current = this.connection.currentModelId;
    const picked = await vscode.window.showQuickPick(
      modes.map((mode) => ({
        label: mode.name || mode.id,
        description: mode.id === current ? "current" : mode.id,
        detail: mode.description ?? undefined,
        modeId: mode.id,
      })),
      {
        title: "Ocean: Select Model",
        placeHolder: "Choose the model for this session",
        matchOnDescription: true,
        matchOnDetail: true,
        ignoreFocusOut: true,
      },
    );
    if (!picked) {
      return;
    }
    await this.setModel(picked.modeId);
  }

  async selectThinkingLevel(): Promise<void> {
    const current = this.connection.settings.thinkingLevel;
    const options = [
      { label: "Daemon default", description: current === "" ? "current" : "", value: "" },
      { label: "Off", description: current === "off" ? "current" : "", value: "off" },
      { label: "Minimal", description: current === "minimal" ? "current" : "", value: "minimal" },
      { label: "Low", description: current === "low" ? "current" : "", value: "low" },
      { label: "Medium", description: current === "medium" ? "current" : "", value: "medium" },
      { label: "High", description: current === "high" ? "current" : "", value: "high" },
      { label: "Extra high", description: current === "xhigh" ? "current" : "", value: "xhigh" },
    ];
    const picked = await vscode.window.showQuickPick(options, {
      title: "Ocean: Thinking Level",
      placeHolder: "Choose per-turn thinking level",
      ignoreFocusOut: true,
    });
    if (!picked) {
      return;
    }
    await this.updateOceanSetting("thinkingLevel", picked.value);
  }

  async toggleEditorContext(): Promise<void> {
    const enabled = !this.connection.settings.injectEditorContext;
    await this.updateOceanSetting("injectEditorContext", enabled);
    await vscode.window.showInformationMessage(
      `Ocean editor context ${enabled ? "enabled" : "disabled"}.`,
    );
  }

  async selectPermissionMode(): Promise<void> {
    const current = this.connection.settings.autoApprovePermissions;
    const picked = await vscode.window.showQuickPick(
      [
        {
          label: "Ask",
          description: current === "ask" ? "current" : "",
          value: "ask",
        },
        {
          label: "Allow all",
          description: current === "allowAll" ? "current" : "",
          value: "allowAll",
        },
      ],
      {
        title: "Ocean: Permission Mode",
        placeHolder: "Choose how Ocean handles tool permissions",
        ignoreFocusOut: true,
      },
    );
    if (!picked) {
      return;
    }
    await this.updateOceanSetting("autoApprovePermissions", picked.value);
  }

  async toggleTrafficLog(): Promise<void> {
    const enabled = !this.connection.settings.logTraffic;
    await this.updateOceanSetting("logTraffic", enabled, true);
    await vscode.window.showInformationMessage(
      `Ocean traffic log ${enabled ? "enabled" : "disabled"}.`,
    );
  }

  async renameCurrentSession(): Promise<void> {
    const sessionId = this.connection.currentSessionId;
    if (!sessionId) {
      await vscode.window.showWarningMessage("Ocean has no active session to rename.");
      return;
    }

    const existing = this.sessions.find((session) => session.id === sessionId);
    const title = await vscode.window.showInputBox({
      title: "Ocean: Rename Current Session",
      value: existing?.title ?? shortSessionId(sessionId),
      ignoreFocusOut: true,
    });
    if (title === undefined) {
      return;
    }
    const trimmed = title.trim();
    if (!trimmed) {
      await vscode.window.showWarningMessage("Session title cannot be empty.");
      return;
    }

    await this.recordSession({
      id: sessionId,
      title: trimmed,
      cwd: existing?.cwd ?? workspaceRoot(),
      updatedAt: new Date().toISOString(),
    });
  }

  async cancelTurn(): Promise<void> {
    if (this.streamingAssistantIndex !== null) {
      this.cancellingTurn = true;
      this.postState();
    }
    await this.connection.cancelTurn();
  }

  async copyLastAssistantMessage(): Promise<void> {
    const message = [...this.messages]
      .reverse()
      .find((item) => item.role === "assistant" && item.content.trim());
    if (!message) {
      await vscode.window.showInformationMessage("Ocean has no assistant response to copy.");
      return;
    }

    await vscode.env.clipboard.writeText(message.content.trim());
    await vscode.window.showInformationMessage("Ocean copied the last response.");
  }

  async copyTranscript(): Promise<void> {
    const text = formatTranscriptForClipboard(this.serializeTranscript());
    if (!text) {
      await vscode.window.showInformationMessage("Ocean has no transcript to copy.");
      return;
    }

    await vscode.env.clipboard.writeText(text);
    await vscode.window.showInformationMessage("Ocean copied the transcript.");
  }

  disconnect(): void {
    const wasConnected = this.connection.connected;
    this.connection.disconnect();
    if (wasConnected) {
      this.pushSystem("Disconnected.");
    } else {
      this.postState();
    }
  }

  dispose(): void {
    this.editorPanel?.dispose();
    this.diffProvider.dispose();
    this.diffDocuments.clear();
    this.connection.disconnect();
  }

  private attachWebview(
    webview: vscode.Webview,
    onDidDispose: vscode.Event<void>,
    options: { surfaceMode?: SurfaceMode; onDispose?: () => void } = {},
  ): void {
    webview.options = {
      enableScripts: true,
      localResourceRoots: [vscode.Uri.joinPath(this.extensionUri, "media")],
    };
    webview.html = this.renderHtml(webview, options.surfaceMode ?? "sidebar");
    this.webviews.add(webview);
    const messageDisposable = webview.onDidReceiveMessage((message) => {
      void this.handleWebviewMessage(webview, message);
    });
    const disposeSubscription = onDidDispose(() => {
      this.webviews.delete(webview);
      messageDisposable.dispose();
      disposeSubscription.dispose();
      options.onDispose?.();
    });
    this.postState();
  }

  private async handleWebviewMessage(
    webview: vscode.Webview,
    message: WebviewMessage,
  ): Promise<void> {
    switch (message.type) {
      case "ready":
        this.postState();
        break;
      case "mentionSearch":
        await this.postMentionSuggestions(webview, message);
        break;
      case "connect":
        await this.connect();
        break;
      case "send":
        await this.sendPrompt(String(message.text ?? ""));
        break;
      case "setModel":
        await this.setModel(String(message.value ?? ""));
        break;
      case "loadSession":
        await this.loadSession(String(message.sessionId ?? message.value ?? ""));
        break;
      case "openFile":
        await this.openTranscriptFile(message.path, message.line);
        break;
      case "openDiff":
        await this.openToolDiff(message.toolId, message.path);
        break;
      case "setThinkingLevel":
        await this.updateOceanSetting("thinkingLevel", String(message.value ?? ""));
        break;
      case "setEditorContext":
        await this.updateOceanSetting("injectEditorContext", Boolean(message.value));
        break;
      case "setPermissionMode":
        await this.updateOceanSetting("autoApprovePermissions", String(message.value ?? "ask"));
        break;
      case "setTrafficLog":
        await this.updateOceanSetting("logTraffic", Boolean(message.value));
        break;
      case "setDaemonUrl":
        await this.updateOceanSetting("daemon.url", String(message.value ?? ""), true);
        break;
      case "setAcpPath":
        await this.updateOceanSetting("acp.path", String(message.value ?? ""), true);
        break;
      case "configureDaemonUrl":
        await this.configureDaemonUrl();
        break;
      case "configureAcpPath":
        await this.configureAcpPath();
        break;
      case "toggleEditorContext":
        await this.updateOceanSetting(
          "injectEditorContext",
          !this.connection.settings.injectEditorContext,
        );
        break;
      case "togglePermissionMode":
        await this.updateOceanSetting(
          "autoApprovePermissions",
          this.connection.settings.autoApprovePermissions === "allowAll"
            ? "ask"
            : "allowAll",
        );
        break;
      case "toggleTrafficLog":
        await this.updateOceanSetting("logTraffic", !this.connection.settings.logTraffic);
        break;
      case "openSettings":
        await vscode.commands.executeCommand(
          "workbench.action.openSettings",
          "@ext:risingtides.ocean-surface",
        );
        break;
      case "inlineAssist":
        await this.inlineAssistSelection();
        break;
      case "newSession":
        await this.newSession();
        break;
      case "openEditor":
        this.openEditorPanel(vscode.ViewColumn.Active);
        break;
      case "openPanel":
        await vscode.commands.executeCommand("workbench.view.extension.oceanPanel");
        break;
      case "openSidebar":
        await vscode.commands.executeCommand("workbench.view.extension.ocean");
        break;
      case "cancel":
        await this.cancelTurn();
        break;
      default:
        break;
    }
  }

  private async postMentionSuggestions(
    webview: vscode.Webview,
    message: WebviewMessage,
  ): Promise<void> {
    const query = typeof message.query === "string" ? message.query.slice(0, 160) : "";
    const suggestions = await workspaceMentionFileSuggestions(query);
    await webview.postMessage({
      type: "mentionSuggestions",
      requestId: message.requestId ?? null,
      query,
      items: suggestions,
    });
  }

  private handleSessionUpdate(notification: SessionNotification): void {
    const update = notification.update;
    const currentSessionId = this.connection.currentSessionId;
    if (currentSessionId && notification.sessionId !== currentSessionId) {
      return;
    }

    if (update.sessionUpdate === "session_info_update") {
      void this.recordSession({
        id: notification.sessionId,
        title: update.title,
        updatedAt: update.updatedAt,
        cwd: workspaceRoot(),
      });
      return;
    }

    if (update.sessionUpdate === "usage_update") {
      this.usage = {
        used: update.used,
        size: update.size,
      };
      this.postState();
      return;
    }

    if (update.sessionUpdate === "user_message_chunk" && update.content.type === "text") {
      void this.titleCurrentSessionFromPrompt(update.content.text);
      this.appendMessageChunk("user", update.content.text, update.messageId);
      this.postState();
      return;
    }

    if (
      update.sessionUpdate === "agent_message_chunk" &&
      update.content.type === "text"
    ) {
      if (this.cancellingTurn && isCancellationText(update.content.text)) {
        return;
      }
      if (this.streamingAssistantIndex !== null && !update.messageId) {
        const message = this.messages[this.streamingAssistantIndex];
        message.content = joinMessageChunk(message.content, update.content.text);
        this.ensureMessageInTranscript(this.streamingAssistantIndex);
      } else {
        this.appendMessageChunk("assistant", update.content.text, update.messageId);
      }
      this.postState();
      return;
    }

    if (update.sessionUpdate === "agent_thought_chunk" && update.content.type === "text") {
      log(`thinking: ${update.content.text.slice(0, 80)}`);
      return;
    }

    if (update.sessionUpdate === "current_mode_update") {
      this.connection.setCurrentMode(update.currentModeId);
      this.postState();
      return;
    }

    if (update.sessionUpdate === "tool_call") {
      const isNewTool = !this.tools.has(update.toolCallId);
      this.upsertTool({
        id: update.toolCallId,
        title: update.title ?? update.toolCallId,
        status: update.status ?? "pending",
        kind: update.kind ?? undefined,
        locations: update.locations ?? [],
        content: update.content ?? [],
      });
      if (isNewTool) {
        this.ensureToolInTranscript(update.toolCallId);
      }
      this.postState();
      return;
    }

    if (update.sessionUpdate === "tool_call_update") {
      const existing = this.tools.get(update.toolCallId);
      const isNewTool = !existing;
      this.upsertTool({
        id: update.toolCallId,
        title: update.title ?? existing?.title ?? update.toolCallId,
        status: update.status ?? existing?.status ?? "in_progress",
        kind: update.kind ?? existing?.kind,
        locations:
          update.locations === undefined
            ? existing?.locations
            : update.locations ?? [],
        content:
          update.content === undefined ? existing?.content : update.content ?? [],
      });
      if (isNewTool) {
        this.ensureToolInTranscript(update.toolCallId);
      }
      this.postState();
    }
  }

  private async promptAndRender(displayText: string, prompt: string): Promise<string> {
    if (this.streamingAssistantIndex !== null) {
      throw new Error("Ocean is already working on a turn.");
    }

    if (!this.connection.connected) {
      await this.connect();
    }
    if (!this.connection.currentSessionId) {
      const session = await this.connection.newSession();
      await this.recordSession({
        id: session.sessionId,
        title: "New session",
        cwd: workspaceRoot(),
      });
    }

    this.cancellingTurn = false;
    await this.titleCurrentSessionFromPrompt(displayText);
    const editSet = this.beginEditSet(displayText);
    this.pushMessage({ role: "user", content: displayText });
    this.messages.push({ role: "assistant", content: "", streaming: true });
    const assistantIndex = this.messages.length - 1;
    this.streamingAssistantIndex = assistantIndex;
    this.postState();

    try {
      const response = await this.connection.prompt(prompt);
      const assistantMessage = this.messages[assistantIndex];
      assistantMessage.streaming = false;
      if (this.cancellingTurn || isCancellationText(assistantMessage.content)) {
        assistantMessage.content = "Cancelled.";
        this.ensureMessageInTranscript(assistantIndex);
        return assistantMessage.content;
      }
      if (!assistantMessage.content.trim()) {
        assistantMessage.content =
          `Turn finished without assistant output (stop: ${response.stopReason}).`;
        this.ensureMessageInTranscript(assistantIndex);
      }
      return assistantMessage.content;
    } catch (error) {
      logError("Prompt failed", error);
      const errorText = error instanceof Error ? error.message : String(error);
      this.messages[assistantIndex] = {
        role: "assistant",
        content:
          this.cancellingTurn || isCancellationText(errorText)
            ? "Cancelled."
            : `Error: ${errorText}`,
      };
      this.ensureMessageInTranscript(assistantIndex);
      throw error;
    } finally {
      if (this.streamingAssistantIndex === assistantIndex) {
        this.streamingAssistantIndex = null;
      }
      this.finishEditSet(editSet.id);
      this.cancellingTurn = false;
      this.persistCurrentSessionSnapshot();
      this.postState();
    }
  }

  private upsertTool(tool: ToolActivity): void {
    this.tools.set(tool.id, tool);
  }

  private pushMessage(message: ChatMessage): number {
    const nextIndex = this.messages.length;
    this.messages.push(message);
    this.ensureMessageInTranscript(nextIndex);
    return nextIndex;
  }

  private ensureMessageInTranscript(messageIndex: number): void {
    if (this.messageTranscriptIndexes.has(messageIndex)) {
      return;
    }
    this.messageTranscriptIndexes.add(messageIndex);
    this.transcript.push({ type: "message", messageIndex });
  }

  private ensureToolInTranscript(toolId: string): void {
    if (this.toolTranscriptIds.has(toolId)) {
      return;
    }
    this.toolTranscriptIds.add(toolId);
    this.transcript.push({ type: "tool", toolId });
  }

  private clearTranscript(): void {
    this.messages = [];
    this.transcript = [];
    this.messageIndexes.clear();
    this.messageTranscriptIndexes.clear();
    this.tools.clear();
    this.toolTranscriptIds.clear();
  }

  private serializeTranscript(): Array<
    | { type: "message"; message: ChatMessage }
    | { type: "tool"; tool: ToolActivity }
  > {
    const items: Array<
      | { type: "message"; message: ChatMessage }
      | { type: "tool"; tool: ToolActivity }
    > = [];

    for (const item of this.transcript) {
      if (item.type === "message") {
        const message = this.messages[item.messageIndex];
        if (message) {
          items.push({ type: "message", message });
        }
        continue;
      }
      const tool = this.tools.get(item.toolId);
      if (tool) {
        items.push({ type: "tool", tool: this.serializeTool(tool) });
      }
    }
    return items;
  }

  private serializeTool(tool: ToolActivity): ToolActivity {
    const terminalOutputs: Record<string, TerminalOutputPreview> = {};
    for (const content of tool.content ?? []) {
      if (content.type !== "terminal" || !content.terminalId) {
        continue;
      }
      const snapshot = this.connection.terminalSnapshot(content.terminalId);
      if (snapshot) {
        const preview = terminalPreview(snapshot);
        this.terminalOutputCache.set(content.terminalId, preview);
        terminalOutputs[content.terminalId] = preview;
        continue;
      }
      const cached = this.terminalOutputCache.get(content.terminalId);
      if (cached) {
        terminalOutputs[content.terminalId] = cached;
      }
    }
    return Object.keys(terminalOutputs).length
      ? { ...tool, terminalOutputs }
      : tool;
  }

  private appendMessageChunk(
    role: "user" | "assistant",
    text: string,
    messageId?: string | null,
  ): void {
    if (!text) {
      return;
    }

    if (messageId) {
      const existingIndex = this.messageIndexes.get(messageId);
      if (existingIndex !== undefined) {
        this.messages[existingIndex].content = joinMessageChunk(
          this.messages[existingIndex].content,
          text,
        );
        this.ensureMessageInTranscript(existingIndex);
        return;
      }
    }

    const lastIndex = this.messages.length - 1;
    const lastMessage = this.messages[lastIndex];
    if (!messageId && lastMessage?.role === role && !lastMessage.streaming) {
      lastMessage.content = joinMessageChunk(lastMessage.content, text);
      this.ensureMessageInTranscript(lastIndex);
      return;
    }

    const nextIndex = this.pushMessage({ role, content: text, id: messageId });
    if (messageId) {
      this.messageIndexes.set(messageId, nextIndex);
    }
  }

  private async recordSession(session: TrackedSession): Promise<void> {
    if (!session.id.trim()) {
      return;
    }
    const existing = this.sessions.find((item) => item.id === session.id);
    const merged: TrackedSession = {
      id: session.id,
      title: session.title ?? existing?.title ?? shortSessionId(session.id),
      updatedAt: session.updatedAt ?? new Date().toISOString(),
      cwd: session.cwd ?? existing?.cwd ?? workspaceRoot(),
    };
    this.sessions = [
      merged,
      ...this.sessions.filter((item) => item.id !== session.id),
    ].slice(0, 30);
    await this.state.update("ocean.sessions", this.sessions);
    this.postState();
  }

  private async recordSessions(sessions: TrackedSession[]): Promise<void> {
    const byId = new Map(this.sessions.map((session) => [session.id, session]));
    for (const session of sessions) {
      if (!session.id.trim()) {
        continue;
      }
      const existing = byId.get(session.id);
      byId.set(session.id, {
        id: session.id,
        title: session.title ?? existing?.title ?? shortSessionId(session.id),
        updatedAt: session.updatedAt ?? existing?.updatedAt ?? new Date().toISOString(),
        cwd: session.cwd ?? existing?.cwd ?? workspaceRoot(),
      });
    }

    this.sessions = [...byId.values()]
      .sort(compareSessionsByActivity)
      .slice(0, 30);
    await this.state.update("ocean.sessions", this.sessions);
    this.postState();
  }

  private async refreshSessionsFromAgent(options: {
    notifyErrors: boolean;
    notifyUnsupported: boolean;
  }): Promise<number | null> {
    try {
      if (!this.connection.connected) {
        await this.connection.connect();
      }
      if (!this.connection.supportsListSessions) {
        if (options.notifyUnsupported) {
          await vscode.window.showInformationMessage(
            "Ocean ACP does not expose session listing yet.",
          );
        }
        return null;
      }

      const sessions: TrackedSession[] = [];
      let cursor: string | null | undefined = null;
      do {
        const page = await this.connection.listSessions(cursor);
        sessions.push(
          ...page.sessions.map((session): TrackedSession => ({
            id: session.sessionId,
            title: session.title ?? null,
            updatedAt: session.updatedAt ?? null,
            cwd: session.cwd ?? null,
          })),
        );
        cursor = page.nextCursor ?? null;
      } while (cursor && sessions.length < 100);

      await this.recordSessions(sessions);
      return sessions.length;
    } catch (error) {
      logError("Session refresh failed", error);
      if (options.notifyErrors) {
        const message = error instanceof Error ? error.message : String(error);
        await vscode.window.showErrorMessage(`Ocean could not refresh sessions: ${message}`);
      }
      return null;
    }
  }

  private restoreSessionSnapshot(sessionId: string): boolean {
    const snapshot = this.sessionSnapshots.find((item) => item.sessionId === sessionId);
    if (!snapshot) {
      return false;
    }

    this.messages = snapshot.messages.map((message) => ({
      ...message,
      streaming: false,
    }));
    this.transcript = snapshot.transcript;
    this.messageTranscriptIndexes.clear();
    this.toolTranscriptIds.clear();
    this.messageIndexes.clear();
    this.tools.clear();

    for (const [index, message] of this.messages.entries()) {
      if (message.id) {
        this.messageIndexes.set(message.id, index);
      }
    }
    for (const ref of this.transcript) {
      if (ref.type === "message") {
        this.messageTranscriptIndexes.add(ref.messageIndex);
      } else {
        this.toolTranscriptIds.add(ref.toolId);
      }
    }
    for (const tool of snapshot.tools) {
      this.tools.set(tool.id, tool);
    }
    this.usage = snapshot.usage ?? null;
    return true;
  }

  private persistCurrentSessionSnapshot(): void {
    const sessionId = this.connection.currentSessionId;
    if (!sessionId || !this.transcript.length) {
      return;
    }
    const snapshot = createSessionSnapshot(
      sessionId,
      this.messages,
      this.transcript,
      this.tools,
      this.usage,
    );
    if (!snapshot) {
      return;
    }
    this.sessionSnapshots = [
      snapshot,
      ...this.sessionSnapshots.filter((item) => item.sessionId !== sessionId),
    ].slice(0, maxPersistedSessionSnapshots);
    void this.state.update("ocean.sessionSnapshots", this.sessionSnapshots);
  }

  private async titleCurrentSessionFromPrompt(promptText: string): Promise<void> {
    const sessionId = this.connection.currentSessionId;
    if (!sessionId) {
      return;
    }
    const existing = this.sessions.find((item) => item.id === sessionId);
    if (existing?.title && !isGenericSessionTitle(existing.title, sessionId)) {
      return;
    }
    const title = sessionTitleFromPrompt(promptText);
    if (!title) {
      return;
    }
    await this.recordSession({
      id: sessionId,
      title,
      updatedAt: new Date().toISOString(),
      cwd: existing?.cwd ?? workspaceRoot(),
    });
  }

  private contextSummary(): {
    enabled: boolean;
    activeFile: string | null;
    openTabs: number;
    diagnostics: number;
  } {
    if (!this.connection.settings.injectEditorContext) {
      return {
        enabled: false,
        activeFile: null,
        openTabs: 0,
        diagnostics: 0,
      };
    }
    const context = collectEditorContext();
    return {
      enabled: true,
      activeFile: context.active_editor
        ? path.basename(context.active_editor.path)
        : null,
      openTabs: context.open_tabs.length,
      diagnostics: context.diagnostics.length,
    };
  }

  private async setModel(modelId: string): Promise<void> {
    if (!modelId.trim()) {
      return;
    }
    try {
      await this.connection.setModel(modelId);
      this.pushSystem(`Model set for this session: ${modelId}`);
    } catch (error) {
      logError("Model change failed", error);
      this.pushSystem(`Model change failed: ${String(error)}`);
    }
  }

  private async openTranscriptFile(
    pathValue: unknown,
    lineValue: unknown,
  ): Promise<void> {
    const targetPath = typeof pathValue === "string" ? pathValue.trim() : "";
    if (!targetPath) {
      return;
    }

    const absolutePath = path.isAbsolute(targetPath)
      ? targetPath
      : path.resolve(workspaceRoot(), targetPath);
    const uri = vscode.Uri.file(absolutePath);
    try {
      const document = await vscode.workspace.openTextDocument(uri);
      const line = typeof lineValue === "number" ? lineValue : Number(lineValue);
      const options: vscode.TextDocumentShowOptions = {
        preview: true,
        preserveFocus: false,
      };
      if (Number.isFinite(line) && line > 0) {
        const lineIndex = Math.min(
          Math.max(Math.floor(line) - 1, 0),
          Math.max(document.lineCount - 1, 0),
        );
        const position = new vscode.Position(lineIndex, 0);
        options.selection = new vscode.Range(position, position);
      }
      await vscode.window.showTextDocument(document, options);
    } catch (error) {
      logError(`Open transcript file failed: ${absolutePath}`, error);
      await vscode.window.showErrorMessage(`Ocean could not open ${targetPath}`);
    }
  }

  private async openToolDiff(toolIdValue: unknown, pathValue: unknown): Promise<void> {
    const toolId = typeof toolIdValue === "string" ? toolIdValue : "";
    const targetPath = typeof pathValue === "string" ? pathValue.trim() : "";
    if (!toolId || !targetPath) {
      return;
    }

    const diff = this.findToolDiff(toolId, targetPath);
    if (!diff) {
      await this.openTranscriptFile(targetPath, undefined);
      return;
    }

    const baseName = path.basename(targetPath);
    const token = encodeURIComponent(`${toolId}:${targetPath}:${Date.now()}`);
    const oldUri = vscode.Uri.parse(
      `ocean-diff:/${encodeURIComponent(baseName)}?side=old&id=${token}`,
    );
    const newUri = vscode.Uri.parse(
      `ocean-diff:/${encodeURIComponent(baseName)}?side=new&id=${token}`,
    );
    this.diffDocuments.set(oldUri.toString(), diff.oldText ?? "");
    this.diffDocuments.set(newUri.toString(), diff.newText);

    await vscode.commands.executeCommand(
      "vscode.diff",
      oldUri,
      newUri,
      `${targetPath} (Ocean)`,
      { preview: true },
    );
  }

  private async openAppliedEditDiff(edit: TrackedAppliedEdit): Promise<void> {
    const baseName = path.basename(edit.path);
    const token = encodeURIComponent(`applied:${edit.id}:${Date.now()}`);
    const oldUri = vscode.Uri.parse(
      `ocean-diff:/${encodeURIComponent(baseName)}?side=before&id=${token}`,
    );
    const newUri = vscode.Uri.parse(
      `ocean-diff:/${encodeURIComponent(baseName)}?side=after&id=${token}`,
    );
    this.diffDocuments.set(oldUri.toString(), edit.oldText ?? "");
    this.diffDocuments.set(newUri.toString(), edit.newText);

    await vscode.commands.executeCommand(
      "vscode.diff",
      oldUri,
      newUri,
      `${edit.created ? "Created" : "Edited"} ${edit.path} ` +
        `(Ocean${editTextWasTruncated(edit) ? ", truncated" : ""})`,
      { preview: true },
    );
  }

  private async previewInlineAssistEdit(input: {
    document: vscode.TextDocument;
    cursor: vscode.Position;
    hasSelection: boolean;
    oldText: string;
    newText: string;
  }): Promise<boolean> {
    const baseName = path.basename(input.document.uri.fsPath || input.document.fileName);
    const token = encodeURIComponent(`inline:${input.document.uri.toString()}:${Date.now()}`);
    const oldUri = vscode.Uri.parse(
      `ocean-diff:/${encodeURIComponent(baseName)}?side=before-inline&id=${token}`,
    );
    const newUri = vscode.Uri.parse(
      `ocean-diff:/${encodeURIComponent(baseName)}?side=after-inline&id=${token}`,
    );
    const before = input.hasSelection
      ? input.oldText
      : inlineInsertPreview(input.document, input.cursor, "");
    const after = input.hasSelection
      ? input.newText
      : inlineInsertPreview(input.document, input.cursor, input.newText);
    this.diffDocuments.set(oldUri.toString(), before);
    this.diffDocuments.set(newUri.toString(), after);

    await vscode.commands.executeCommand(
      "vscode.diff",
      oldUri,
      newUri,
      `${baseName} (Ocean inline preview)`,
      { preview: true },
    );

    const confirmed = await vscode.window.showInformationMessage(
      `Apply Ocean inline edit to ${baseName}?`,
      { modal: true },
      "Apply",
    );
    return confirmed === "Apply";
  }

  private async pickAppliedEditSet(
    title: string,
    placeHolder: string,
  ): Promise<AppliedEditSet | undefined> {
    const setPick = await vscode.window.showQuickPick(
      this.editSets.map((set): AppliedEditSetQuickPickItem => ({
        label: set.title,
        description: `${set.edits.length} ${set.edits.length === 1 ? "file" : "files"}`,
        detail: [
          set.sessionId ? `session ${shortSessionId(set.sessionId)}` : null,
          formatSessionDate(set.startedAt),
        ].filter(Boolean).join(" · "),
        set,
      })),
      {
        title,
        placeHolder,
        matchOnDescription: true,
        matchOnDetail: true,
        ignoreFocusOut: true,
      },
    );
    return setPick?.set;
  }

  private async pickAppliedEditFromSet(
    set: AppliedEditSet,
    placeHolder: string,
  ): Promise<TrackedAppliedEdit | undefined> {
    if (set.edits.length === 1) {
      return set.edits[0];
    }

    const editPick = await vscode.window.showQuickPick(
      set.edits.map((edit): AppliedEditQuickPickItem => ({
        label: path.basename(edit.path),
        description: edit.created ? "created" : "edited",
        detail: `${edit.path} · ${formatSessionDate(edit.timestamp) ?? ""}`,
        edit,
      })),
      {
        title: set.title,
        placeHolder,
        matchOnDescription: true,
        matchOnDetail: true,
        ignoreFocusOut: true,
      },
    );
    return editPick?.edit;
  }

  private async revertAppliedEdit(edit: TrackedAppliedEdit): Promise<void> {
    try {
      await this.applyAppliedEditRevert(edit, true);
    } catch (error) {
      logError(`Revert applied edit failed: ${edit.path}`, error);
      const message = error instanceof Error ? error.message : String(error);
      await vscode.window.showErrorMessage(`Ocean could not revert ${edit.path}: ${message}`);
      return;
    }

    this.removeAppliedEdits(new Set([edit.id]));
    await vscode.window.showInformationMessage(`Ocean reverted ${path.basename(edit.path)}.`);
  }

  private async applyAppliedEditRevert(
    edit: TrackedAppliedEdit,
    revealDocument: boolean,
  ): Promise<void> {
    const unsafeReason = unsafeAppliedEditRevertReason(edit);
    if (unsafeReason) {
      throw new Error(unsafeReason);
    }

    const uri = vscode.Uri.file(edit.path);
    if (edit.created && edit.oldText === null) {
      if (await fileExists(uri)) {
        await vscode.workspace.fs.delete(uri, {
          recursive: false,
          useTrash: false,
        });
      }
      return;
    }

    if (edit.oldText !== null) {
      await this.restoreTextFile(uri, edit.oldText, revealDocument);
      return;
    }

    throw new Error("Missing before snapshot.");
  }

  private async restoreTextFile(
    uri: vscode.Uri,
    content: string,
    revealDocument: boolean,
  ): Promise<void> {
    const openDoc = vscode.workspace.textDocuments.find(
      (document) => document.uri.fsPath === uri.fsPath,
    );
    if (openDoc) {
      const edit = new vscode.WorkspaceEdit();
      edit.replace(uri, fullDocumentRange(openDoc), content);
      const applied = await vscode.workspace.applyEdit(edit);
      if (!applied) {
        throw new Error(`VS Code rejected edit for ${uri.fsPath}`);
      }
      await openDoc.save();
      if (revealDocument) {
        await vscode.window.showTextDocument(openDoc, {
          preview: true,
          preserveFocus: true,
        });
      }
      return;
    }

    await vscode.workspace.fs.writeFile(uri, Buffer.from(content, "utf-8"));
    if (revealDocument) {
      const document = await vscode.workspace.openTextDocument(uri);
      await vscode.window.showTextDocument(document, {
        preview: true,
        preserveFocus: true,
      });
    }
  }

  private findToolDiff(
    toolId: string,
    targetPath: string,
  ): Extract<ToolCallContent, { type: "diff" }> | null {
    const tool = this.tools.get(toolId);
    if (!tool?.content) {
      return null;
    }
    return (
      tool.content.find(
        (content): content is Extract<ToolCallContent, { type: "diff" }> =>
          content.type === "diff" && content.path === targetPath,
      ) ?? null
    );
  }

  async configureDaemonUrl(): Promise<void> {
    const current = this.connection.settings.daemonUrl;
    const value = await vscode.window.showInputBox({
      title: "Ocean daemon URL",
      prompt: "Set the daemon URL used when connecting Ocean ACP.",
      value: current,
      ignoreFocusOut: true,
    });
    if (!value || value.trim() === current) {
      return;
    }
    await this.updateOceanSetting("daemon.url", value.trim(), true);
  }

  async configureAcpPath(): Promise<void> {
    const current = this.connection.settings.acpPath;
    const picked = await vscode.window.showOpenDialog({
      title: "Select ocean-acp binary",
      canSelectFiles: true,
      canSelectFolders: false,
      canSelectMany: false,
    });
    const value = picked?.[0]?.fsPath ?? "";
    if (!value || value === current) {
      return;
    }
    await this.updateOceanSetting("acp.path", value, true);
  }

  private async updateOceanSetting(
    key: string,
    value: string | boolean,
    reconnectRequired = false,
  ): Promise<void> {
    await vscode.workspace
      .getConfiguration("ocean")
      .update(key, value, vscode.ConfigurationTarget.Global);
    if (reconnectRequired && this.connection.connected) {
      this.connection.disconnect();
      this.pushSystem("Setting saved. Reconnect Ocean to use it.");
      return;
    }
    this.postState();
  }

  private recordAppliedEdit(edit: AppliedFileEdit): void {
    if (edit.oldText === edit.newText) {
      return;
    }
    const oldText = truncateAppliedEditText(edit.oldText);
    const newText = truncateAppliedEditText(edit.newText);
    const tracked: TrackedAppliedEdit = {
      ...edit,
      oldText: oldText.text,
      newText: newText.text ?? "",
      oldTextTruncated: oldText.truncated,
      newTextTruncated: newText.truncated,
      id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      timestamp: new Date().toISOString(),
      sessionId: this.connection.currentSessionId,
    };
    const context = this.activeEditSet ?? this.beginEditSet("Ocean file edit");
    const set = this.ensureEditSet(context);
    set.edits = [
      tracked,
      ...set.edits.filter((item) => item.path !== tracked.path),
    ].slice(0, 24);
    set.endedAt = tracked.timestamp;
    this.editSets = [
      set,
      ...this.editSets.filter((item) => item.id !== set.id),
    ].slice(0, 20);
    this.appliedEdits = flattenEditSets(this.editSets);
    void this.persistEditSets();
  }

  private beginEditSet(title: string): ActiveEditSet {
    const active: ActiveEditSet = {
      id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      title: editSetTitle(title),
      sessionId: this.connection.currentSessionId,
      startedAt: new Date().toISOString(),
    };
    this.activeEditSet = active;
    return active;
  }

  private finishEditSet(editSetId: string): void {
    if (this.activeEditSet?.id === editSetId) {
      this.activeEditSet = null;
    }
    const set = this.editSets.find((item) => item.id === editSetId);
    if (!set || set.endedAt) {
      return;
    }
    set.endedAt = new Date().toISOString();
    void this.persistEditSets();
  }

  private ensureEditSet(active: ActiveEditSet): AppliedEditSet {
    const existing = this.editSets.find((set) => set.id === active.id);
    if (existing) {
      return existing;
    }
    return {
      id: active.id,
      title: active.title,
      sessionId: active.sessionId,
      startedAt: active.startedAt,
      endedAt: null,
      edits: [],
    };
  }

  private async persistEditSets(): Promise<void> {
    await this.state.update("ocean.editSets", this.editSets);
  }

  private removeAppliedEdits(editIds: Set<string>): void {
    this.editSets = this.editSets
      .map((set) => ({
        ...set,
        edits: set.edits.filter((edit) => !editIds.has(edit.id)),
      }))
      .filter((set) => set.edits.length > 0);
    this.appliedEdits = flattenEditSets(this.editSets);
    void this.persistEditSets();
  }

  private async documentForDiagnosticTarget(
    target: DiagnosticFixTarget | undefined,
  ): Promise<vscode.TextDocument | null> {
    if (target?.uri) {
      try {
        return await vscode.workspace.openTextDocument(target.uri);
      } catch (error) {
        logError(`Open diagnostic target failed: ${target.uri.toString()}`, error);
        await vscode.window.showErrorMessage("Ocean could not open the diagnostic target.");
        return null;
      }
    }
    return vscode.window.activeTextEditor?.document ?? null;
  }

  private async pickDiagnostics(
    diagnostics: vscode.Diagnostic[],
  ): Promise<DiagnosticQuickPickItem | undefined> {
    if (diagnostics.length === 1) {
      return {
        label: diagnosticLabel(diagnostics[0]),
        detail: diagnosticDetail(diagnostics[0]),
        diagnostics,
      };
    }

    const items: DiagnosticQuickPickItem[] = [
      {
        label: "All diagnostics in current file",
        description: `${diagnostics.length} problems`,
        diagnostics,
        all: true,
      },
      ...diagnostics.map((diagnostic) => ({
        label: diagnosticLabel(diagnostic),
        description: diagnosticSeverityLabel(diagnostic.severity),
        detail: diagnosticDetail(diagnostic),
        diagnostics: [diagnostic],
      })),
    ];

    return vscode.window.showQuickPick(items, {
      title: "Ocean: Fix Diagnostics",
      placeHolder: "Pick a diagnostic to fix",
      matchOnDescription: true,
      matchOnDetail: true,
      ignoreFocusOut: true,
    });
  }

  private async pickInlineInstruction(hasSelection: boolean): Promise<string | undefined> {
    const rewriteChoices: InlineAssistChoice[] = [
      {
        label: "Rewrite...",
        description: "custom",
        custom: true,
      },
      {
        label: "Tighten",
        description: "shorter, sharper",
        instruction: "Rewrite this to be tighter, sharper, and less wordy.",
      },
      {
        label: "Polish",
        description: "clean final copy",
        instruction: "Polish this into clean final copy without changing the meaning.",
      },
      {
        label: "Direct",
        description: "less soft, more decisive",
        instruction: "Make this more direct, confident, and concrete.",
      },
      {
        label: "Fix",
        description: "grammar and clarity",
        instruction: "Fix grammar, punctuation, and clarity while preserving the voice.",
      },
    ];

    const insertChoices: InlineAssistChoice[] = [
      {
        label: "Write...",
        description: "custom",
        custom: true,
      },
      {
        label: "Continue",
        description: "next paragraph",
        instruction: "Continue from the cursor with the next natural paragraph.",
      },
      {
        label: "Draft",
        description: "usable first pass",
        instruction: "Draft the next section in the same voice and structure.",
      },
      {
        label: "Bridge",
        description: "connect ideas",
        instruction: "Write a concise transition that connects the surrounding ideas.",
      },
    ];

    const pick = await vscode.window.showQuickPick(
      hasSelection ? rewriteChoices : insertChoices,
      {
        title: hasSelection ? "Ocean Inline Rewrite" : "Ocean Inline Write",
        placeHolder: hasSelection ? "Pick a rewrite mode" : "Pick what to insert",
        ignoreFocusOut: true,
      },
    );
    if (!pick) {
      return undefined;
    }
    if (!pick.custom) {
      return pick.instruction;
    }

    const instruction = await vscode.window.showInputBox({
      title: hasSelection ? "Ocean Inline Rewrite" : "Ocean Inline Write",
      prompt: hasSelection
        ? "How should Ocean rewrite the selected text?"
        : "What should Ocean write at the cursor?",
      placeHolder: hasSelection
        ? "Make this tighter, warmer, more direct..."
        : "Draft a short intro, add a stronger close...",
      ignoreFocusOut: true,
    });
    return instruction?.trim() || undefined;
  }

  private async pickWorkspaceFiles(): Promise<vscode.Uri[]> {
    if (!vscode.workspace.workspaceFolders?.length) {
      await vscode.window.showWarningMessage("Open a workspace before selecting Ocean file context.");
      return [];
    }

    const files = await vscode.workspace.findFiles(
      "**/*",
      workspaceSearchExcludeGlob,
      1200,
    );
    if (!files.length) {
      await vscode.window.showInformationMessage("Ocean found no workspace files to attach.");
      return [];
    }

    const root = workspaceRoot();
    const items = files
      .map((uri): WorkspaceFileQuickPickItem => {
        const relativePath = path.relative(root, uri.fsPath) || path.basename(uri.fsPath);
        return {
          label: path.basename(uri.fsPath),
          description: relativePath,
          detail: path.dirname(relativePath),
          uri,
          relativePath,
        };
      })
      .sort((a, b) => a.relativePath.localeCompare(b.relativePath));

    const picked = await vscode.window.showQuickPick(items, {
      title: "Ocean: Ask About Files",
      placeHolder: `Pick up to ${maxWorkspaceContextFiles} files to send as context`,
      canPickMany: true,
      matchOnDescription: true,
      matchOnDetail: true,
      ignoreFocusOut: true,
    });
    return picked?.map((item) => item.uri) ?? [];
  }

  private async enrichPromptWithComposerMentions(text: string): Promise<string> {
    const fileMentions = extractComposerFileMentions(text);
    const wantsChanges = hasWorkspaceChangesMention(text);
    const wantsDiagnostics = hasWorkspaceDiagnosticsMention(text);
    const wantsActiveFile = hasActiveFileMention(text);
    const wantsSelection = hasSelectionMention(text);
    const wantsTerminal = hasTerminalMention(text);
    const wantsOpenTabs = hasOpenTabsMention(text);
    const wantsWorkspaceTree = hasWorkspaceTreeMention(text);
    if (
      !fileMentions.length &&
      !wantsChanges &&
      !wantsDiagnostics &&
      !wantsActiveFile &&
      !wantsSelection &&
      !wantsTerminal &&
      !wantsOpenTabs &&
      !wantsWorkspaceTree
    ) {
      return text;
    }

    const { uris, skipped: unresolved } = await resolveComposerFileMentions(fileMentions);
    const allSkipped = [...unresolved];

    if (wantsActiveFile) {
      const activeFile = activeEditorFileUri();
      if (activeFile.uri) {
        uris.push(activeFile.uri);
      } else {
        allSkipped.push(`@current (${activeFile.reason})`);
      }
    }

    if (wantsOpenTabs) {
      const openTabs = openTabFileUris();
      uris.push(...openTabs.uris);
      allSkipped.push(...openTabs.skipped);
    }

    const { files, skipped } = await collectWorkspaceFileContexts(uris);
    allSkipped.push(...skipped);
    const attachments: ComposerContextAttachment[] = [];

    if (wantsChanges) {
      try {
        const snapshot = await collectWorkspaceChanges(workspaceRoot());
        attachments.push({
          label: "Workspace changes",
          content: hasWorkspaceChanges(snapshot)
            ? formatWorkspaceChangesContext(snapshot)
            : "Git status is clean.",
        });
      } catch (error) {
        logError("Composer workspace changes collection failed", error);
        const message = error instanceof Error ? error.message : String(error);
        allSkipped.push(`@changes (${message})`);
      }
    }

    if (wantsDiagnostics) {
      attachments.push({
        label: "Workspace diagnostics",
        content: formatWorkspaceDiagnosticsContext(),
      });
    }

    if (wantsSelection) {
      const selection = formatActiveSelectionContext();
      if (selection.content) {
        attachments.push({
          label: "Active selection",
          content: selection.content,
        });
      } else {
        allSkipped.push(`@selection (${selection.reason})`);
      }
    }

    if (wantsTerminal) {
      const terminal = this.formatRecentTerminalContext();
      if (terminal.content) {
        attachments.push({
          label: "Recent terminal output",
          content: terminal.content,
        });
      } else {
        allSkipped.push(`@terminal (${terminal.reason})`);
      }
    }

    if (wantsWorkspaceTree) {
      attachments.push({
        label: "Workspace file map",
        content: await formatWorkspaceTreeContext(),
      });
    }

    if (!files.length && !attachments.length && !allSkipped.length) {
      return text;
    }

    return buildComposerContextPrompt({
      instruction: text,
      files,
      attachments,
      skipped: allSkipped,
    });
  }

  private formatRecentTerminalContext(): { content: string | null; reason: string } {
    const terminals: Array<{
      terminalId: string;
      toolTitle: string;
      toolStatus: ToolActivity["status"];
      preview: TerminalOutputPreview;
    }> = [];
    const seen = new Set<string>();

    for (let index = this.transcript.length - 1; index >= 0; index -= 1) {
      const ref = this.transcript[index];
      if (ref.type !== "tool") {
        continue;
      }
      const tool = this.tools.get(ref.toolId);
      if (!tool?.content?.length) {
        continue;
      }
      for (const content of [...tool.content].reverse()) {
        if (content.type !== "terminal" || !content.terminalId || seen.has(content.terminalId)) {
          continue;
        }
        seen.add(content.terminalId);
        const preview = this.terminalOutputPreviewForId(content.terminalId);
        if (!preview?.output.trim()) {
          continue;
        }
        terminals.push({
          terminalId: content.terminalId,
          toolTitle: tool.title,
          toolStatus: tool.status,
          preview,
        });
        if (terminals.length >= maxComposerTerminalPromptItems) {
          break;
        }
      }
      if (terminals.length >= maxComposerTerminalPromptItems) {
        break;
      }
    }

    if (!seen.size) {
      return { content: null, reason: "no terminal output in current session" };
    }
    if (!terminals.length) {
      return { content: null, reason: "terminal output is empty or unavailable" };
    }

    const body = terminals.flatMap((terminal) => [
      `--- terminal: ${terminal.terminalId}`,
      `tool: ${terminal.toolTitle}`,
      `status: ${terminal.toolStatus}`,
      `exit: ${formatTerminalExitForPrompt(terminal.preview)}`,
      `truncated: ${terminal.preview.truncated}`,
      "",
      terminal.preview.output,
      `--- end terminal: ${terminal.terminalId}`,
      "",
    ]).join("\n");
    const trimmed = trimFileForPrompt(body, maxComposerTerminalPromptChars);
    return {
      content: trimmed.truncated
        ? `${trimmed.text}\n\n[terminal context truncated at ${maxComposerTerminalPromptChars} chars]`
        : trimmed.text,
      reason: "",
    };
  }

  private terminalOutputPreviewForId(terminalId: string): TerminalOutputPreview | null {
    const snapshot = this.connection.terminalSnapshot(terminalId);
    if (snapshot) {
      const preview = terminalPreview(snapshot);
      this.terminalOutputCache.set(terminalId, preview);
      return preview;
    }
    return this.terminalOutputCache.get(terminalId) ?? null;
  }

  private pushSystem(content: string): void {
    this.pushMessage({ role: "system", content });
    this.postState();
  }

  private postState(): void {
    const currentSessionId = this.connection.currentSessionId;
    const state = {
      type: "state",
      connected: this.connection.connected,
      sessionId: currentSessionId,
      turnInProgress: this.streamingAssistantIndex !== null,
      cancelling: this.cancellingTurn,
      tools: [...this.tools.values()],
      messages: this.messages,
      transcript: this.serializeTranscript(),
      agent: this.connection.agentInfo,
      currentModelId: this.connection.currentModelId,
      modelOptions: this.connection.modelOptions,
      sessionOptions: this.sessions,
      usage: this.usage,
      contextSummary: this.contextSummary(),
      supportsLoadSession: this.connection.supportsLoadSession,
      supportsListSessions: this.connection.supportsListSessions,
      thinkingLevel: this.connection.settings.thinkingLevel,
      settings: this.connection.settings,
    };
    this.onStatusChange?.({
      connected: state.connected,
      turnInProgress: state.turnInProgress,
      cancelling: state.cancelling,
      currentModelId: state.currentModelId,
      sessionId: currentSessionId,
      sessionTitle: this.currentSessionTitle(currentSessionId),
    });
    for (const webview of this.webviews) {
      void webview.postMessage(state).then(
        () => undefined,
        () => this.webviews.delete(webview),
      );
    }
  }

  private currentSessionTitle(sessionId: string | null): string | null {
    if (!sessionId) {
      return null;
    }
    const session = this.sessions.find((item) => item.id === sessionId);
    return session?.title || shortSessionId(sessionId);
  }

  private renderHtml(webview: vscode.Webview, surfaceMode: SurfaceMode): string {
    const scriptUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this.extensionUri, "media", "chat.js"),
    );
    const styleUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this.extensionUri, "media", "chat.css"),
    );
    const nonce = String(Date.now());
    const assetVersion = encodeURIComponent(`${extensionVersion}-${surfaceMode}-${nonce}`);

    return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource}; script-src 'nonce-${nonce}';" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <link rel="stylesheet" href="${styleUri}?v=${assetVersion}" />
  <title>Ocean</title>
</head>
<body class="surface-${surfaceMode}">
  <div id="shell">
    <header id="topbar">
      <strong>Ocean</strong>
      <span id="status">Disconnected</span>
    </header>
    <section id="runtime-bar" aria-label="Ocean runtime">
      <label class="model-control">
        <span class="sr-only">Model</span>
        <select id="modelSelect" title="Set the model for this Ocean session">
          <option value="">Connect to load models</option>
        </select>
      </label>
      <label class="session-control">
        <span class="sr-only">Session</span>
        <select id="sessionSelect" title="Load a previous Ocean session">
          <option value="">No session</option>
        </select>
      </label>
      <button type="button" id="newSession" class="text-button" title="Start a new Ocean session">New</button>
      <label class="thinking-control">
        <span class="sr-only">Thinking level</span>
        <select id="thinkingSelect" title="Per-turn thinking level">
          <option value="">Think default</option>
          <option value="off">Think off</option>
          <option value="minimal">Think minimal</option>
          <option value="low">Think low</option>
          <option value="medium">Think medium</option>
          <option value="high">Think high</option>
          <option value="xhigh">Think xhigh</option>
        </select>
      </label>
      <span id="contextValue" class="runtime-stat" title="Context window usage">ctx</span>
      <details id="settingsMenu" class="runtime-menu">
        <summary title="Ocean settings">Settings</summary>
        <div class="settings-grid">
          <label>
            <span>Daemon</span>
            <input id="daemonUrlInput" type="text" autocomplete="off" />
          </label>
          <label>
            <span>ACP</span>
            <input id="acpPathInput" type="text" autocomplete="off" />
          </label>
          <label>
            <span>Permissions</span>
            <select id="permissionSelect">
              <option value="ask">Ask</option>
              <option value="allowAll">Allow all</option>
            </select>
          </label>
          <label class="check-row">
            <input id="contextToggle" type="checkbox" />
            <span>Editor context</span>
          </label>
          <label class="check-row">
            <input id="trafficToggle" type="checkbox" />
            <span>Traffic log</span>
          </label>
        </div>
      </details>
    </section>
    <div id="activity" hidden></div>
    <div id="messages"></div>
    <form id="composer">
      <div id="mentionMenu" class="mention-menu" role="listbox" hidden></div>
      <textarea id="input" rows="2" placeholder="Ask Ocean about this workspace" title="Enter sends. Shift+Enter inserts a line."></textarea>
      <div class="actions">
        <button type="button" id="connect" class="connect-button" title="Connect to Ocean">Connect</button>
        <button type="submit" class="send-button" title="Send with Enter">Send</button>
        <button type="button" id="cancel" class="quiet-button">Stop</button>
      </div>
    </form>
  </div>
  <script nonce="${nonce}" src="${scriptUri}?v=${assetVersion}"></script>
</body>
</html>`;
  }
}

function buildInlineRewritePrompt(input: {
  file: string;
  languageId: string;
  instruction: string;
  selectedText: string;
}): string {
  return [
    "You are editing selected text inside VS Code/Cursor.",
    "Apply the instruction and return only the exact replacement text.",
    "Do not explain, summarize, add Markdown fences, or mention Ocean.",
    "Preserve formatting and indentation unless the instruction asks to change it.",
    "",
    `File: ${input.file}`,
    `Language: ${input.languageId}`,
    "",
    "Instruction:",
    input.instruction,
    "",
    "Selected text:",
    "<selected_text>",
    input.selectedText,
    "</selected_text>",
  ].join("\n");
}

function buildInlineInsertPrompt(input: {
  file: string;
  languageId: string;
  instruction: string;
  context: string;
}): string {
  return [
    "You are writing at the cursor inside VS Code/Cursor.",
    "Apply the instruction and return only the exact text to insert at the cursor.",
    "Do not explain, summarize, add Markdown fences, or mention Ocean.",
    "Match the surrounding voice, tense, formatting, and indentation.",
    "",
    `File: ${input.file}`,
    `Language: ${input.languageId}`,
    "",
    "Instruction:",
    input.instruction,
    "",
    "Local context around cursor:",
    "<cursor_context>",
    input.context,
    "</cursor_context>",
  ].join("\n");
}

function buildCurrentFilePrompt(input: {
  file: string;
  languageId: string;
  isDirty: boolean;
  instruction: string;
  text: string;
}): string {
  const fileText = trimFileForPrompt(input.text, 60000);
  return [
    "You are helping with the active file inside VS Code/Cursor.",
    "Use the exact file content below as the source of truth.",
    "If you propose edits, reference the specific locations and keep the answer actionable.",
    "",
    `File: ${input.file}`,
    `Language: ${input.languageId}`,
    `Unsaved changes: ${input.isDirty}`,
    fileText.truncated ? "File content: truncated to fit the prompt." : "File content:",
    "<current_file>",
    fileText.text,
    "</current_file>",
    "",
    "Request:",
    input.instruction,
  ].join("\n");
}

function buildWorkspaceFilesPrompt(input: {
  instruction: string;
  files: WorkspaceFilePromptContext[];
  skipped: string[];
}): string {
  return [
    "You are helping with explicitly selected workspace files inside VS Code/Cursor.",
    "Use the selected file contents below as the source of truth.",
    "If a fix is clear, prefer direct file edits through the available filesystem tools.",
    "Keep changes scoped to the selected files unless the user asks for a broader workspace change.",
    "",
    "Selected files:",
    ...input.files.map((file) =>
      `- ${file.relativePath}${file.isDirty ? " (unsaved buffer)" : ""}${file.truncated ? " (truncated)" : ""}`,
    ),
    input.skipped.length ? "" : null,
    input.skipped.length ? "Skipped files:" : null,
    ...input.skipped.map((item) => `- ${item}`),
    "",
    "File contents:",
    ...input.files.flatMap((file) => [
      `--- file: ${file.relativePath}`,
      `unsaved: ${file.isDirty}`,
      `truncated: ${file.truncated}`,
      file.text,
      `--- end: ${file.relativePath}`,
      "",
    ]),
    "Request:",
    input.instruction,
  ].filter((line): line is string => line !== null).join("\n");
}

function buildComposerContextPrompt(input: {
  instruction: string;
  files: WorkspaceFilePromptContext[];
  attachments: ComposerContextAttachment[];
  skipped: string[];
}): string {
  return [
    "The user typed context mentions in the VS Code/Cursor chat composer.",
    "Use the attached context below as source of truth for those mentions.",
    "If a mentioned context item was skipped or unresolved, say so only when it affects the answer.",
    "If a fix is clear, prefer direct file edits through the available filesystem tools.",
    "",
    input.files.length ? "Mentioned files:" : null,
    ...input.files.map((file) =>
      `- ${file.relativePath}${file.isDirty ? " (unsaved buffer)" : ""}${file.truncated ? " (truncated)" : ""}`,
    ),
    input.skipped.length ? "" : null,
    input.skipped.length ? "Skipped mentions:" : null,
    ...input.skipped.map((item) => `- ${item}`),
    "",
    input.files.length ? "File contents:" : null,
    ...input.files.flatMap((file) => [
      `--- file: ${file.relativePath}`,
      `unsaved: ${file.isDirty}`,
      `truncated: ${file.truncated}`,
      file.text,
      `--- end: ${file.relativePath}`,
      "",
    ]),
    ...input.attachments.flatMap((attachment) => [
      `--- context: ${attachment.label}`,
      attachment.content,
      `--- end: ${attachment.label}`,
      "",
    ]),
    "User request:",
    input.instruction,
  ].filter((line): line is string => line !== null).join("\n");
}

function buildDiagnosticFixPrompt(input: {
  file: string;
  languageId: string;
  isDirty: boolean;
  text: string;
  diagnostics: vscode.Diagnostic[];
  all: boolean;
}): string {
  const fileText = trimFileForPrompt(input.text, 60000);
  return [
    "You are fixing VS Code/Cursor diagnostics in the active file.",
    "Use the exact unsaved buffer content below as source of truth.",
    "Prefer direct file edits through the available filesystem tools when a fix is clear.",
    "Keep the change focused on the selected diagnostics and preserve existing behavior.",
    "After editing, summarize the fix and mention any diagnostics that still need human review.",
    "",
    `File: ${input.file}`,
    `Language: ${input.languageId}`,
    `Unsaved changes: ${input.isDirty}`,
    `Scope: ${input.all ? "all selected current-file diagnostics" : "one selected diagnostic"}`,
    "",
    "Diagnostics:",
    ...input.diagnostics.map(formatDiagnosticForPrompt),
    "",
    fileText.truncated ? "File content: truncated to fit the prompt." : "File content:",
    "<current_file>",
    fileText.text,
    "</current_file>",
  ].join("\n");
}

function localWritingContext(document: vscode.TextDocument, cursor: vscode.Position): string {
  const start = Math.max(0, cursor.line - 30);
  const end = Math.min(document.lineCount - 1, cursor.line + 30);
  const range = new vscode.Range(new vscode.Position(start, 0), document.lineAt(end).range.end);
  const text = document.getText(range);
  const cursorLine = cursor.line - start + 1;
  const cursorColumn = cursor.character + 1;
  return [
    `Cursor: local line ${cursorLine}, column ${cursorColumn}`,
    "",
    text,
  ].join("\n");
}

function inlineInsertPreview(
  document: vscode.TextDocument,
  cursor: vscode.Position,
  insertion: string,
): string {
  const start = Math.max(0, cursor.line - 10);
  const end = Math.min(document.lineCount - 1, cursor.line + 10);
  const before = document.getText(
    new vscode.Range(new vscode.Position(start, 0), cursor),
  );
  const after = document.getText(
    new vscode.Range(cursor, document.lineAt(end).range.end),
  );
  const marker = insertion
    ? ["<<< Ocean insert >>>", insertion, "<<< end Ocean insert >>>"].join("\n")
    : "<<< cursor >>>";
  return [before.replace(/\s+$/g, ""), marker, after.replace(/^\s+/g, "")]
    .filter(Boolean)
    .join("\n");
}

function normalizeInlineReplacement(text: string): string {
  const value = text.replace(/\r\n/g, "\n");
  const fenced = value.trim().match(/^```(?:[a-zA-Z0-9_-]+)?\n([\s\S]*?)\n```$/);
  if (fenced) {
    return fenced[1];
  }
  return value.replace(/^\n+/, "").replace(/\n+$/, "");
}

function trimFileForPrompt(text: string, maxChars: number): { text: string; truncated: boolean } {
  if (text.length <= maxChars) {
    return { text, truncated: false };
  }
  return {
    text: text.slice(0, maxChars).replace(/\s+$/g, ""),
    truncated: true,
  };
}

async function filterFileUris(uris: vscode.Uri[]): Promise<vscode.Uri[]> {
  const unique = new Map<string, vscode.Uri>();
  for (const uri of uris) {
    if (uri.scheme !== "file") {
      continue;
    }
    try {
      const stat = await vscode.workspace.fs.stat(uri);
      if ((stat.type & vscode.FileType.File) === vscode.FileType.File) {
        unique.set(uri.fsPath, uri);
      }
    } catch {
      continue;
    }
  }
  return [...unique.values()];
}

async function workspaceMentionFileSuggestions(
  query: string,
): Promise<ComposerMentionSuggestion[]> {
  const normalizedQuery = normalizeMentionSearchQuery(query);
  if (!shouldSearchWorkspaceFiles(normalizedQuery)) {
    return [];
  }

  const folders = vscode.workspace.workspaceFolders ?? [];
  if (!folders.length) {
    return [];
  }

  const roots = workspaceRoots();
  const matches = await vscode.workspace.findFiles(
    "**/*",
    workspaceSearchExcludeGlob,
    maxComposerMentionFileSearch,
  );
  const files = await filterFileUris(matches);
  const queryParts = normalizedQuery
    .split(/[\/._-]+/g)
    .map((part) => part.trim())
    .filter(Boolean);

  return files
    .map((uri) => {
      const relativePath = workspaceRelativePath(uri.fsPath, roots).replace(/\\/g, "/");
      return {
        relativePath,
        basename: path.basename(relativePath),
        score: mentionSuggestionScore(relativePath, normalizedQuery, queryParts),
      };
    })
    .filter((item) => item.score < Number.POSITIVE_INFINITY)
    .sort((left, right) =>
      left.score - right.score ||
      left.relativePath.split("/").length - right.relativePath.split("/").length ||
      left.relativePath.localeCompare(right.relativePath),
    )
    .slice(0, maxComposerMentionSuggestions)
    .map((item) => ({
      kind: "file",
      label: item.basename,
      detail: item.relativePath,
      insert: composerMentionInsertForPath(item.relativePath),
    }));
}

function normalizeMentionSearchQuery(query: string): string {
  return query
    .replace(/^@/, "")
    .replace(/^["'`]/, "")
    .trim()
    .toLowerCase();
}

function shouldSearchWorkspaceFiles(query: string): boolean {
  return query.length >= 2 || /[./_-]/.test(query);
}

function mentionSuggestionScore(
  relativePath: string,
  query: string,
  queryParts: string[],
): number {
  const haystack = relativePath.toLowerCase();
  const basename = path.basename(relativePath).toLowerCase();
  if (!query) {
    return Number.POSITIVE_INFINITY;
  }
  if (basename === query) {
    return 0;
  }
  if (basename.startsWith(query)) {
    return 1;
  }
  if (haystack.startsWith(query)) {
    return 2;
  }
  if (basename.includes(query)) {
    return 3;
  }
  if (haystack.includes(query)) {
    return 4;
  }
  if (queryParts.length && queryParts.every((part) => haystack.includes(part))) {
    return 5;
  }
  return Number.POSITIVE_INFINITY;
}

function composerMentionInsertForPath(relativePath: string): string {
  const normalized = relativePath.replace(/\\/g, "/");
  const needsQuotes =
    /[\s)\]}>,;]/.test(normalized) ||
    (!normalized.includes("/") && !normalized.includes("."));
  if (!needsQuotes) {
    return `@${normalized}`;
  }
  if (!normalized.includes("\"")) {
    return `@"${normalized}"`;
  }
  if (!normalized.includes("`")) {
    return `@\`${normalized}\``;
  }
  return `@'${normalized.replace(/'/g, "")}'`;
}

function extractComposerFileMentions(text: string): ComposerFileMention[] {
  const mentions: ComposerFileMention[] = [];
  const pattern =
    /(^|[\s([{])@(?:"([^"\n]+)"|'([^'\n]+)'|`([^`\n]+)`|([^\s)\]}>,;]+))/g;
  for (const match of text.matchAll(pattern)) {
    const quotedPath = match[2] ?? match[3] ?? match[4] ?? "";
    const barePath = match[5] ?? "";
    const quoted = Boolean(quotedPath);
    const raw = quotedPath || barePath;
    const mentionPath = normalizeMentionPath(raw);
    if (!mentionPath || (!quoted && !isFileLikeMention(mentionPath))) {
      continue;
    }
    mentions.push({
      raw,
      path: mentionPath,
      quoted,
    });
  }

  const seen = new Set<string>();
  return mentions.filter((mention) => {
    const key = mention.path.toLowerCase();
    if (seen.has(key)) {
      return false;
    }
    seen.add(key);
    return true;
  });
}

function hasWorkspaceChangesMention(text: string): boolean {
  return /(^|[\s([{])@(changes|git|diff)(?=$|[\s)\]}>,;.!?])/i.test(text);
}

function hasWorkspaceDiagnosticsMention(text: string): boolean {
  return /(^|[\s([{])@(problems|diagnostics)(?=$|[\s)\]}>,;.!?])/i.test(text);
}

function hasActiveFileMention(text: string): boolean {
  return /(^|[\s([{])@(current|active|file)(?=$|[\s)\]}>,;.!?])/i.test(text);
}

function hasSelectionMention(text: string): boolean {
  return /(^|[\s([{])@(selection|sel)(?=$|[\s)\]}>,;.!?])/i.test(text);
}

function hasTerminalMention(text: string): boolean {
  return /(^|[\s([{])@(terminal|term|last-terminal|last-command|cmd)(?=$|[\s)\]}>,;.!?])/i.test(text);
}

function hasOpenTabsMention(text: string): boolean {
  return /(^|[\s([{])@(tabs|open-tabs|open|editors|buffers)(?=$|[\s)\]}>,;.!?])/i.test(text);
}

function hasWorkspaceTreeMention(text: string): boolean {
  return /(^|[\s([{])@(workspace|codebase|tree|project)(?=$|[\s)\]}>,;.!?])/i.test(text);
}

function activeEditorFileUri(): { uri: vscode.Uri | null; reason: string } {
  const editor = vscode.window.activeTextEditor;
  if (!editor) {
    return { uri: null, reason: "no active editor" };
  }
  const uri = editor.document.uri;
  if (uri.scheme !== "file") {
    return { uri: null, reason: "active editor is not a file" };
  }
  if (!isInsideWorkspace(uri.fsPath)) {
    return { uri: null, reason: "active file is outside workspace" };
  }
  return { uri, reason: "" };
}

function formatActiveSelectionContext(): { content: string | null; reason: string } {
  const editor = vscode.window.activeTextEditor;
  if (!editor) {
    return { content: null, reason: "no active editor" };
  }
  if (editor.selection.isEmpty) {
    return { content: null, reason: "no selected text" };
  }

  const document = editor.document;
  const selectionText = document.getText(editor.selection);
  if (!selectionText.trim()) {
    return { content: null, reason: "selected text is empty" };
  }

  const trimmed = trimFileForPrompt(selectionText, maxComposerSelectionPromptChars);
  const label =
    document.uri.scheme === "file"
      ? workspaceRelativePath(document.uri.fsPath, workspaceRoots())
      : document.uri.toString();
  return {
    content: [
      `file: ${label}`,
      `language: ${document.languageId}`,
      `dirty: ${document.isDirty}`,
      `range: ${formatRangeForPrompt(editor.selection)}`,
      `truncated: ${trimmed.truncated}`,
      "",
      trimmed.text,
    ].join("\n"),
    reason: "",
  };
}

function formatRangeForPrompt(range: vscode.Range): string {
  return [
    `${range.start.line + 1}:${range.start.character + 1}`,
    `${range.end.line + 1}:${range.end.character + 1}`,
  ].join("-");
}

function openTabFileUris(): { uris: vscode.Uri[]; skipped: string[] } {
  const roots = workspaceRoots();
  const uris: vscode.Uri[] = [];
  const skipped: string[] = [];
  const seen = new Set<string>();

  for (const group of vscode.window.tabGroups.all) {
    for (const tab of group.tabs) {
      const input = tab.input;
      if (!(input instanceof vscode.TabInputText)) {
        continue;
      }
      const uri = input.uri;
      if (uri.scheme !== "file") {
        skipped.push(`${uri.toString()} (not a file tab)`);
        continue;
      }
      if (!isInsideWorkspace(uri.fsPath, roots)) {
        skipped.push(`${path.basename(uri.fsPath)} (outside workspace)`);
        continue;
      }
      if (seen.has(uri.fsPath)) {
        continue;
      }
      seen.add(uri.fsPath);
      uris.push(uri);
    }
  }

  if (!uris.length && !skipped.length) {
    skipped.push("@tabs (no open workspace file tabs)");
  }
  return { uris, skipped };
}

async function resolveComposerFileMentions(
  mentions: ComposerFileMention[],
): Promise<{ uris: vscode.Uri[]; skipped: string[] }> {
  const uris: vscode.Uri[] = [];
  const skipped: string[] = [];
  const seen = new Set<string>();

  for (const mention of mentions) {
    const resolved = await resolveComposerFileMention(mention);
    if (resolved.uri) {
      if (!seen.has(resolved.uri.fsPath)) {
        seen.add(resolved.uri.fsPath);
        uris.push(resolved.uri);
      }
      continue;
    }
    skipped.push(`@${mention.raw} (${resolved.reason})`);
  }

  return { uris, skipped };
}

async function resolveComposerFileMention(
  mention: ComposerFileMention,
): Promise<{ uri: vscode.Uri | null; reason: string }> {
  const candidates = mentionPathCandidates(mention.path);
  const roots = workspaceRoots();

  for (const candidate of candidates) {
    const expanded = resolveHomePath(candidate);
    const directPaths = path.isAbsolute(expanded)
      ? [expanded]
      : roots.map((root) => path.resolve(root, expanded));
    for (const candidatePath of directPaths) {
      const resolvedPath = candidatePath;
      if (!isInsideWorkspace(resolvedPath, roots)) {
        continue;
      }
      const uri = await fileUriIfReadableFile(resolvedPath);
      if (uri) {
        return { uri, reason: "" };
      }
    }
  }

  if (!mention.path.includes("/") && safeBareMentionGlob(mention.path)) {
    const matches = await vscode.workspace.findFiles(
      `**/${mention.path}`,
      workspaceSearchExcludeGlob,
      20,
    );
    const fileMatches = await filterFileUris(matches);
    if (fileMatches.length === 1) {
      return { uri: fileMatches[0], reason: "" };
    }
    if (fileMatches.length > 1) {
      return { uri: null, reason: `${fileMatches.length} matching files` };
    }
  }

  const expandedMentionPath = resolveHomePath(mention.path);
  if (path.isAbsolute(expandedMentionPath) && !isInsideWorkspace(expandedMentionPath, roots)) {
    return { uri: null, reason: "outside workspace" };
  }
  return { uri: null, reason: "file not found" };
}

function normalizeMentionPath(rawPath: string): string {
  return rawPath
    .trim()
    .replace(/:(\d+)(:\d+)?$/, "")
    .replace(/[.!?]+$/g, "");
}

function isFileLikeMention(mentionPath: string): boolean {
  return (
    mentionPath.startsWith("./") ||
    mentionPath.startsWith("../") ||
    mentionPath.startsWith("/") ||
    mentionPath.startsWith("~/") ||
    mentionPath.includes("/") ||
    mentionPath.includes(".")
  );
}

function mentionPathCandidates(mentionPath: string): string[] {
  const candidates = [mentionPath];
  const withoutTrailingPunctuation = mentionPath.replace(/[.!?]+$/g, "");
  if (withoutTrailingPunctuation && withoutTrailingPunctuation !== mentionPath) {
    candidates.push(withoutTrailingPunctuation);
  }
  return candidates;
}

function workspaceRoots(): string[] {
  const folders = vscode.workspace.workspaceFolders?.map((folder) => folder.uri.fsPath) ?? [];
  return folders.length ? folders : [workspaceRoot()];
}

function resolveHomePath(value: string): string {
  if (value === "~") {
    return homedir();
  }
  if (value.startsWith(`~${path.sep}`)) {
    return path.join(homedir(), value.slice(2));
  }
  return value;
}

function isInsideWorkspace(fsPath: string, roots = workspaceRoots()): boolean {
  const resolved = path.resolve(resolveHomePath(fsPath));
  return roots.some((root) => {
    const resolvedRoot = path.resolve(root);
    return resolved === resolvedRoot || resolved.startsWith(`${resolvedRoot}${path.sep}`);
  });
}

async function fileUriIfReadableFile(fsPath: string): Promise<vscode.Uri | null> {
  try {
    const uri = vscode.Uri.file(fsPath);
    const stat = await vscode.workspace.fs.stat(uri);
    return (stat.type & vscode.FileType.File) === vscode.FileType.File ? uri : null;
  } catch {
    return null;
  }
}

function safeBareMentionGlob(value: string): boolean {
  return /^[A-Za-z0-9._-]+$/.test(value);
}

async function collectWorkspaceFileContexts(
  selectedUris: vscode.Uri[],
): Promise<{ files: WorkspaceFilePromptContext[]; skipped: string[] }> {
  const root = workspaceRoot();
  const uniqueUris = await filterFileUris(selectedUris);
  const skipped: string[] = [];
  if (uniqueUris.length > maxWorkspaceContextFiles) {
    skipped.push(
      `${uniqueUris.length - maxWorkspaceContextFiles} files over the ${maxWorkspaceContextFiles}-file context limit`,
    );
  }

  let remainingChars = maxWorkspaceFilesPromptChars;
  const files: WorkspaceFilePromptContext[] = [];
  for (const uri of uniqueUris.slice(0, maxWorkspaceContextFiles)) {
    const relativePath = path.relative(root, uri.fsPath) || path.basename(uri.fsPath);
    if (remainingChars <= 0) {
      skipped.push(`${relativePath} (prompt file-context budget exhausted)`);
      continue;
    }

    const text = await readWorkspaceText(uri);
    if (text === null) {
      skipped.push(`${relativePath} (not readable as text)`);
      continue;
    }

    const trimmed = trimFileForPrompt(
      text,
      Math.min(maxWorkspaceFilePromptChars, remainingChars),
    );
    files.push({
      uri,
      relativePath,
      text: trimmed.text,
      truncated: trimmed.truncated,
      isDirty: isOpenDocumentDirty(uri),
    });
    remainingChars -= trimmed.text.length;
  }
  return { files, skipped };
}

async function formatWorkspaceTreeContext(): Promise<string> {
  const folders = vscode.workspace.workspaceFolders ?? [];
  if (!folders.length) {
    return "No workspace folder is open.";
  }

  const roots = workspaceRoots();
  const matches = await vscode.workspace.findFiles(
    "**/*",
    workspaceSearchExcludeGlob,
    maxComposerWorkspaceTreeFiles + 1,
  );
  const truncated = matches.length > maxComposerWorkspaceTreeFiles;
  const relativePaths = matches
    .slice(0, maxComposerWorkspaceTreeFiles)
    .filter((uri) => uri.scheme === "file" && isInsideWorkspace(uri.fsPath, roots))
    .map((uri) => workspaceRelativePath(uri.fsPath, roots))
    .sort((left, right) => left.localeCompare(right));

  const lines = [
    "Workspace roots:",
    ...folders.map((folder) => `- ${folder.uri.fsPath}`),
    "",
    `shown_files: ${relativePaths.length}`,
    `truncated: ${truncated}`,
    `excluded: .git, node_modules, dist, build, target, .next, out, coverage`,
    "",
    "Files:",
    ...relativePaths.map((relativePath) => `- ${relativePath}`),
  ];
  const trimmed = trimFileForPrompt(lines.join("\n"), maxComposerWorkspaceTreeChars);
  return trimmed.truncated
    ? `${trimmed.text}\n\n[workspace file map truncated at ${maxComposerWorkspaceTreeChars} chars]`
    : trimmed.text;
}

async function readWorkspaceText(uri: vscode.Uri): Promise<string | null> {
  const openDoc = vscode.workspace.textDocuments.find(
    (document) => document.uri.fsPath === uri.fsPath,
  );
  if (openDoc) {
    return openDoc.getText();
  }
  try {
    const raw = await vscode.workspace.fs.readFile(uri);
    if (looksBinary(raw)) {
      return null;
    }
    return Buffer.from(raw).toString("utf-8");
  } catch {
    return null;
  }
}

function isOpenDocumentDirty(uri: vscode.Uri): boolean {
  return Boolean(
    vscode.workspace.textDocuments.find(
      (document) => document.uri.fsPath === uri.fsPath && document.isDirty,
    ),
  );
}

function looksBinary(data: Uint8Array): boolean {
  const limit = Math.min(data.length, 4096);
  for (let index = 0; index < limit; index += 1) {
    if (data[index] === 0) {
      return true;
    }
  }
  return false;
}

function formatWorkspaceDiagnosticsContext(): string {
  const { diagnostics, total } = collectWorkspaceDiagnosticsForPrompt();
  if (!diagnostics.length) {
    return "No workspace diagnostics reported.";
  }

  const omitted = total - diagnostics.length;
  const lines = [
    `count: ${total}`,
    omitted > 0
      ? `shown: ${diagnostics.length} (${omitted} over the ${maxWorkspaceDiagnosticsPromptItems}-diagnostic limit omitted)`
      : null,
    "",
    ...diagnostics.map(formatWorkspaceDiagnosticForPrompt),
  ].filter((line): line is string => line !== null);
  const trimmed = trimFileForPrompt(lines.join("\n"), maxWorkspaceDiagnosticsPromptChars);
  return trimmed.truncated
    ? `${trimmed.text}\n\n[diagnostics context truncated at ${maxWorkspaceDiagnosticsPromptChars} chars]`
    : trimmed.text;
}

function collectWorkspaceDiagnosticsForPrompt(): {
  diagnostics: WorkspaceDiagnosticPromptContext[];
  total: number;
} {
  const roots = workspaceRoots();
  const collected: WorkspaceDiagnosticPromptContext[] = [];
  for (const [uri, diagnostics] of vscode.languages.getDiagnostics()) {
    if (uri.scheme !== "file" || !isInsideWorkspace(uri.fsPath, roots)) {
      continue;
    }
    const relativePath = workspaceRelativePath(uri.fsPath, roots);
    for (const diagnostic of diagnostics) {
      if (diagnostic.severity === vscode.DiagnosticSeverity.Hint) {
        continue;
      }
      collected.push({ relativePath, diagnostic });
    }
  }

  collected.sort(compareWorkspaceDiagnostics);
  return {
    diagnostics: collected.slice(0, maxWorkspaceDiagnosticsPromptItems),
    total: collected.length,
  };
}

function compareWorkspaceDiagnostics(
  left: WorkspaceDiagnosticPromptContext,
  right: WorkspaceDiagnosticPromptContext,
): number {
  return (
    left.diagnostic.severity - right.diagnostic.severity ||
    left.relativePath.localeCompare(right.relativePath) ||
    left.diagnostic.range.start.line - right.diagnostic.range.start.line ||
    left.diagnostic.range.start.character - right.diagnostic.range.start.character
  );
}

function workspaceRelativePath(fsPath: string, roots: string[]): string {
  const resolved = path.resolve(fsPath);
  const root = roots.find((candidate) => {
    const resolvedRoot = path.resolve(candidate);
    return resolved === resolvedRoot || resolved.startsWith(`${resolvedRoot}${path.sep}`);
  });
  if (!root) {
    return fsPath;
  }
  return path.relative(root, fsPath) || path.basename(fsPath);
}

function formatWorkspaceDiagnosticForPrompt(
  item: WorkspaceDiagnosticPromptContext,
): string {
  const diagnostic = item.diagnostic;
  const start = diagnostic.range.start;
  const end = diagnostic.range.end;
  const source = diagnostic.source ? ` source=${diagnostic.source}` : "";
  const code = diagnostic.code !== undefined ? ` code=${diagnosticCodeValue(diagnostic.code)}` : "";
  return [
    `- ${diagnosticSeverityLabel(diagnostic.severity)} ${item.relativePath}:${start.line + 1}:${start.character + 1}-${end.line + 1}:${end.character + 1}${source}${code}`,
    `  ${diagnostic.message.replace(/\r?\n/g, "\n  ")}`,
  ].join("\n");
}

function diagnosticCodeValue(code: vscode.Diagnostic["code"]): string {
  if (typeof code === "object" && code !== null && "value" in code) {
    return String(code.value);
  }
  return String(code);
}

function diagnosticsForTarget(
  document: vscode.TextDocument,
  target: DiagnosticFixTarget | undefined,
): vscode.Diagnostic[] {
  const source = target?.diagnostics?.length
    ? target.diagnostics
    : vscode.languages.getDiagnostics(document.uri);
  return source.filter(
    (diagnostic) => diagnostic.severity !== vscode.DiagnosticSeverity.Hint,
  );
}

function diagnosticDisplayTitle(
  document: vscode.TextDocument,
  diagnostics: vscode.Diagnostic[],
  all: boolean,
): string {
  const baseName = path.basename(document.uri.fsPath);
  if (all) {
    return `${baseName}: fix ${diagnostics.length} diagnostics`;
  }
  return `${baseName}:${diagnostics[0].range.start.line + 1} fix diagnostic`;
}

function diagnosticLabel(diagnostic: vscode.Diagnostic): string {
  const line = diagnostic.range.start.line + 1;
  const col = diagnostic.range.start.character + 1;
  return `L${line}:${col} ${singleLine(diagnostic.message, 72)}`;
}

function diagnosticDetail(diagnostic: vscode.Diagnostic): string {
  return [
    diagnostic.source ? `source: ${diagnostic.source}` : null,
    diagnostic.code !== undefined ? `code: ${String(diagnostic.code)}` : null,
    diagnosticSeverityLabel(diagnostic.severity),
  ]
    .filter(Boolean)
    .join(" · ");
}

function formatDiagnosticForPrompt(diagnostic: vscode.Diagnostic): string {
  const start = diagnostic.range.start;
  const end = diagnostic.range.end;
  const source = diagnostic.source ? ` source=${diagnostic.source}` : "";
  const code = diagnostic.code !== undefined ? ` code=${String(diagnostic.code)}` : "";
  return [
    `- ${diagnosticSeverityLabel(diagnostic.severity)} ${start.line + 1}:${start.character + 1}-${end.line + 1}:${end.character + 1}${source}${code}`,
    `  ${diagnostic.message.replace(/\r?\n/g, "\n  ")}`,
  ].join("\n");
}

function diagnosticSeverityLabel(severity: vscode.DiagnosticSeverity): string {
  switch (severity) {
    case vscode.DiagnosticSeverity.Error:
      return "Error";
    case vscode.DiagnosticSeverity.Warning:
      return "Warning";
    case vscode.DiagnosticSeverity.Information:
      return "Information";
    case vscode.DiagnosticSeverity.Hint:
      return "Hint";
    default:
      return "Diagnostic";
  }
}

function singleLine(value: string, maxChars: number): string {
  const normalized = value.replace(/\s+/g, " ").trim();
  if (normalized.length <= maxChars) {
    return normalized;
  }
  return `${normalized.slice(0, maxChars - 3).trim()}...`;
}

function terminalPreview(snapshot: TerminalOutputResponse): TerminalOutputPreview {
  const output = trimPreviewText(snapshot.output, 6000);
  return {
    output: output.text,
    truncated: snapshot.truncated || output.truncated,
    exitCode: snapshot.exitStatus?.exitCode ?? null,
    signal: snapshot.exitStatus?.signal ?? null,
  };
}

function formatTerminalExitForPrompt(preview: TerminalOutputPreview): string {
  if (preview.signal) {
    return `signal ${preview.signal}`;
  }
  if (preview.exitCode !== undefined && preview.exitCode !== null) {
    return `code ${preview.exitCode}`;
  }
  return "running or unknown";
}

function trimPreviewText(text: string, maxChars: number): { text: string; truncated: boolean } {
  if (text.length <= maxChars) {
    return { text, truncated: false };
  }
  return {
    text: text.slice(text.length - maxChars),
    truncated: true,
  };
}

function isCancellationText(text: string): boolean {
  const normalized = text.toLowerCase();
  return (
    normalized.includes("agent cancelled") ||
    normalized.includes("agent canceled") ||
    normalized.includes("request cancelled") ||
    normalized.includes("request canceled")
  );
}

function formatTranscriptForClipboard(
  items: Array<
    | { type: "message"; message: ChatMessage }
    | { type: "tool"; tool: ToolActivity }
  >,
): string {
  return items
    .map((item) => {
      if (item.type === "message") {
        const content = item.message.content.trim();
        if (!content) {
          return "";
        }
        return `${roleLabelForClipboard(item.message.role)}\n${content}`;
      }
      return [
        `TOOL ${item.tool.status ?? "pending"}`,
        item.tool.title || item.tool.id || "",
      ]
        .filter(Boolean)
        .join("\n");
    })
    .filter(Boolean)
    .join("\n\n---\n\n");
}

function roleLabelForClipboard(role: ChatMessage["role"]): string {
  switch (role) {
    case "assistant":
      return "OCEAN";
    case "user":
      return "YOU";
    case "system":
      return "SYS";
    case "tool":
      return "TOOL";
    default:
      return String(role).toUpperCase();
  }
}

function joinMessageChunk(existing: string, next: string): string {
  if (!existing) {
    return next;
  }
  if (!next) {
    return existing;
  }
  if (/\s$/.test(existing) || /^\s/.test(next)) {
    return existing + next;
  }

  const last = existing.at(-1) ?? "";
  const first = next.at(0) ?? "";
  if (/^[,.;:!?)}\]'"]/.test(first) || /[([{`'"]$/.test(last)) {
    return existing + next;
  }
  if (/[A-Za-z0-9_]$/.test(existing) && /^[a-z0-9_]/.test(first)) {
    return existing + next;
  }
  return `${existing} ${next}`;
}

function shortSessionId(sessionId: string): string {
  return sessionId.length > 8 ? sessionId.slice(0, 8) : sessionId;
}

function sessionDetail(session: TrackedSession): string {
  return [
    session.cwd || null,
    session.updatedAt ? formatSessionDate(session.updatedAt) : null,
  ]
    .filter(Boolean)
    .join(" · ");
}

function flattenEditSets(sets: AppliedEditSet[]): TrackedAppliedEdit[] {
  return sets
    .flatMap((set) => set.edits)
    .sort((a, b) => b.timestamp.localeCompare(a.timestamp))
    .slice(0, 30);
}

function createSessionSnapshot(
  sessionId: string,
  messages: ChatMessage[],
  transcript: TranscriptRef[],
  tools: Map<string, ToolActivity>,
  usage: UsageSnapshot | null,
): SessionTranscriptSnapshot | null {
  const clippedTranscript = transcript.slice(-maxPersistedTranscriptItems);
  const messageIndexMap = new Map<number, number>();
  const nextMessages: ChatMessage[] = [];
  const nextTranscript: TranscriptRef[] = [];
  const nextTools = new Map<string, ToolActivity>();

  for (const ref of clippedTranscript) {
    if (ref.type === "message") {
      const message = messages[ref.messageIndex];
      if (!message) {
        continue;
      }
      let nextIndex = messageIndexMap.get(ref.messageIndex);
      if (nextIndex === undefined) {
        nextIndex = nextMessages.length;
        messageIndexMap.set(ref.messageIndex, nextIndex);
        nextMessages.push(cloneMessageForSnapshot(message));
      }
      nextTranscript.push({ type: "message", messageIndex: nextIndex });
      continue;
    }

    const tool = tools.get(ref.toolId);
    if (!tool) {
      continue;
    }
    if (!nextTools.has(ref.toolId)) {
      nextTools.set(ref.toolId, cloneToolForSnapshot(tool));
    }
    nextTranscript.push({ type: "tool", toolId: ref.toolId });
  }

  if (!nextTranscript.length) {
    return null;
  }
  return {
    sessionId,
    updatedAt: new Date().toISOString(),
    messages: nextMessages,
    transcript: nextTranscript,
    tools: [...nextTools.values()],
    usage,
  };
}

function cloneMessageForSnapshot(message: ChatMessage): ChatMessage {
  return {
    role: message.role,
    content: truncatePersistedText(message.content, maxPersistedMessageChars),
    id: message.id ?? null,
    streaming: false,
  };
}

function cloneToolForSnapshot(tool: ToolActivity): ToolActivity {
  const clone = JSON.parse(JSON.stringify(tool)) as ToolActivity;
  clone.title = truncatePersistedText(clone.title ?? clone.id, 512);
  clone.content = clone.content?.map((content) => compactToolContent(content));
  if (clone.terminalOutputs) {
    for (const output of Object.values(clone.terminalOutputs)) {
      output.output = truncatePersistedText(output.output, maxPersistedToolTextChars);
    }
  }
  return clone;
}

function compactToolContent(content: ToolCallContent): ToolCallContent {
  const clone = JSON.parse(JSON.stringify(content)) as ToolCallContent;
  if (clone.type === "diff") {
    clone.oldText =
      typeof clone.oldText === "string"
        ? truncatePersistedText(clone.oldText, maxPersistedToolTextChars)
        : null;
    clone.newText =
      typeof clone.newText === "string"
        ? truncatePersistedText(clone.newText, maxPersistedToolTextChars)
        : "";
    return clone;
  }
  if (clone.type === "content" && clone.content?.type === "text") {
    clone.content.text = truncatePersistedText(
      clone.content.text,
      maxPersistedToolTextChars,
    );
    return clone;
  }
  if (clone.type === "content" && clone.content?.type === "resource") {
    const resource = clone.content.resource;
    if (resource && "text" in resource && typeof resource.text === "string") {
      resource.text = truncatePersistedText(resource.text, maxPersistedToolTextChars);
    }
  }
  return clone;
}

function truncatePersistedText(value: string, limit: number): string {
  if (value.length <= limit) {
    return value;
  }
  const headLength = Math.floor(limit * 0.65);
  const tailLength = limit - headLength;
  return [
    value.slice(0, headLength).replace(/\s+$/g, ""),
    `\n\n[... Ocean transcript snapshot truncated: ${value.length - limit} chars omitted ...]\n\n`,
    value.slice(value.length - tailLength).replace(/^\s+/g, ""),
  ].join("");
}

async function fileExists(uri: vscode.Uri): Promise<boolean> {
  try {
    await vscode.workspace.fs.stat(uri);
    return true;
  } catch {
    return false;
  }
}

function fullDocumentRange(document: vscode.TextDocument): vscode.Range {
  const lastLine = document.lineAt(document.lineCount - 1);
  return new vscode.Range(0, 0, document.lineCount - 1, lastLine.text.length);
}

function truncateAppliedEditText(
  value: string | null,
): { text: string | null; truncated: boolean } {
  if (value === null) {
    return { text: null, truncated: false };
  }
  if (value.length <= maxPersistedEditTextChars) {
    return { text: value, truncated: false };
  }
  const headLength = Math.floor(maxPersistedEditTextChars * 0.6);
  const tailLength = maxPersistedEditTextChars - headLength;
  return {
    text: [
      value.slice(0, headLength).replace(/\s+$/g, ""),
      `\n\n[... Ocean edit snapshot truncated: ${value.length - maxPersistedEditTextChars} chars omitted ...]\n\n`,
      value.slice(value.length - tailLength).replace(/^\s+/g, ""),
    ].join(""),
    truncated: true,
  };
}

function editTextWasTruncated(edit: TrackedAppliedEdit): boolean {
  return Boolean(edit.oldTextTruncated || edit.newTextTruncated);
}

function unsafeAppliedEditRevertReason(edit: TrackedAppliedEdit): string | null {
  const baseName = path.basename(edit.path);
  if (edit.oldTextTruncated && !edit.created) {
    return `${baseName} has a truncated before snapshot`;
  }
  if (!edit.created && edit.oldText === null) {
    return `${baseName} is missing a before snapshot`;
  }
  return null;
}

function editSetTitle(value: string): string {
  const title = singleLine(value, 72);
  return title || "Ocean edit set";
}

function formatSessionDate(value: string): string | null {
  const date = new Date(value);
  if (Number.isNaN(date.valueOf())) {
    return null;
  }
  return date.toLocaleString([], {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

function isGenericSessionTitle(title: string, sessionId: string): boolean {
  const normalized = title.trim().toLowerCase();
  return (
    !normalized ||
    normalized === "new session" ||
    normalized === "loaded session" ||
    normalized === shortSessionId(sessionId).toLowerCase()
  );
}

function sessionTitleFromPrompt(promptText: string): string | null {
  const cleaned = promptText
    .replace(/```[\s\S]*?```/g, "code")
    .replace(/`([^`]+)`/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
  if (!cleaned) {
    return null;
  }

  const limit = 56;
  if (cleaned.length <= limit) {
    return cleaned;
  }

  const slice = cleaned.slice(0, limit + 1);
  const boundary = slice.lastIndexOf(" ");
  const title = (boundary >= 28 ? slice.slice(0, boundary) : slice.slice(0, limit)).trim();
  return title ? `${title}...` : null;
}

function sanitizeEditSets(value: unknown): AppliedEditSet[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const sets: AppliedEditSet[] = [];
  for (const item of value) {
    if (!item || typeof item !== "object") {
      continue;
    }
    const candidate = item as Record<string, unknown>;
    const id = typeof candidate.id === "string" ? candidate.id : "";
    const title = typeof candidate.title === "string" ? candidate.title : "";
    const startedAt =
      typeof candidate.startedAt === "string" ? candidate.startedAt : "";
    const edits = sanitizeAppliedEdits(candidate.edits);
    if (!id || !title || !startedAt || !edits.length) {
      continue;
    }
    sets.push({
      id,
      title,
      sessionId:
        typeof candidate.sessionId === "string" ? candidate.sessionId : null,
      startedAt,
      endedAt: typeof candidate.endedAt === "string" ? candidate.endedAt : null,
      edits,
    });
  }
  return sets
    .sort((a, b) => b.startedAt.localeCompare(a.startedAt))
    .slice(0, 20);
}

function sanitizeAppliedEdits(value: unknown): TrackedAppliedEdit[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const edits: TrackedAppliedEdit[] = [];
  for (const item of value) {
    if (!item || typeof item !== "object") {
      continue;
    }
    const candidate = item as Record<string, unknown>;
    const id = typeof candidate.id === "string" ? candidate.id : "";
    const editPath = typeof candidate.path === "string" ? candidate.path : "";
    const newText = typeof candidate.newText === "string" ? candidate.newText : "";
    const timestamp =
      typeof candidate.timestamp === "string" ? candidate.timestamp : "";
    if (!id || !editPath || !timestamp) {
      continue;
    }
    edits.push({
      id,
      path: editPath,
      oldText:
        typeof candidate.oldText === "string" ? candidate.oldText : null,
      newText,
      created: Boolean(candidate.created),
      saved: candidate.saved !== false,
      timestamp,
      sessionId:
        typeof candidate.sessionId === "string" ? candidate.sessionId : null,
      oldTextTruncated: Boolean(candidate.oldTextTruncated),
      newTextTruncated: Boolean(candidate.newTextTruncated),
    });
  }
  return edits
    .sort((a, b) => b.timestamp.localeCompare(a.timestamp))
    .slice(0, 24);
}

function sanitizeSessions(value: unknown): TrackedSession[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const sessions: TrackedSession[] = [];
  for (const item of value) {
    if (!item || typeof item !== "object") {
      continue;
    }
    const candidate = item as Record<string, unknown>;
    const id = typeof candidate.id === "string" ? candidate.id.trim() : "";
    if (!id) {
      continue;
    }
    sessions.push({
      id,
      title: typeof candidate.title === "string" ? candidate.title : null,
      updatedAt:
        typeof candidate.updatedAt === "string" ? candidate.updatedAt : null,
      cwd: typeof candidate.cwd === "string" ? candidate.cwd : null,
    });
  }
  return sessions.sort(compareSessionsByActivity).slice(0, 30);
}

function compareSessionsByActivity(a: TrackedSession, b: TrackedSession): number {
  return sessionActivityTime(b) - sessionActivityTime(a);
}

function sessionActivityTime(session: TrackedSession): number {
  const timestamp = Date.parse(session.updatedAt ?? "");
  return Number.isFinite(timestamp) ? timestamp : 0;
}

function sanitizeSessionSnapshots(value: unknown): SessionTranscriptSnapshot[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const snapshots: SessionTranscriptSnapshot[] = [];
  for (const item of value) {
    if (!item || typeof item !== "object") {
      continue;
    }
    const candidate = item as Record<string, unknown>;
    const sessionId =
      typeof candidate.sessionId === "string" ? candidate.sessionId.trim() : "";
    const updatedAt =
      typeof candidate.updatedAt === "string" ? candidate.updatedAt : "";
    const messages = sanitizeSnapshotMessages(candidate.messages);
    const tools = sanitizeSnapshotTools(candidate.tools);
    const toolIds = new Set(tools.map((tool) => tool.id));
    const transcript = sanitizeSnapshotTranscript(
      candidate.transcript,
      messages.length,
      toolIds,
    );
    if (!sessionId || !updatedAt || !transcript.length) {
      continue;
    }
    snapshots.push({
      sessionId,
      updatedAt,
      messages,
      transcript,
      tools,
      usage: sanitizeUsageSnapshot(candidate.usage),
    });
  }
  return snapshots
    .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))
    .slice(0, maxPersistedSessionSnapshots);
}

function sanitizeSnapshotMessages(value: unknown): ChatMessage[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const messages: ChatMessage[] = [];
  for (const item of value) {
    if (!item || typeof item !== "object") {
      continue;
    }
    const candidate = item as Record<string, unknown>;
    const role = candidate.role;
    const content = typeof candidate.content === "string" ? candidate.content : "";
    if (
      role !== "user" &&
      role !== "assistant" &&
      role !== "system" &&
      role !== "tool"
    ) {
      continue;
    }
    messages.push({
      role,
      content,
      id: typeof candidate.id === "string" ? candidate.id : null,
      streaming: false,
    });
  }
  return messages;
}

function sanitizeSnapshotTools(value: unknown): ToolActivity[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const tools: ToolActivity[] = [];
  for (const item of value) {
    if (!item || typeof item !== "object") {
      continue;
    }
    const candidate = item as ToolActivity;
    if (typeof candidate.id !== "string" || !candidate.id) {
      continue;
    }
    tools.push({
      ...candidate,
      title:
        typeof candidate.title === "string" && candidate.title
          ? candidate.title
          : candidate.id,
      status: sanitizeToolStatus(candidate.status),
    });
  }
  return tools;
}

function sanitizeToolStatus(value: unknown): ToolActivity["status"] {
  return value === "pending" ||
    value === "in_progress" ||
    value === "completed" ||
    value === "failed"
    ? value
    : "completed";
}

function sanitizeSnapshotTranscript(
  value: unknown,
  messageCount: number,
  toolIds: Set<string>,
): TranscriptRef[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const refs: TranscriptRef[] = [];
  for (const item of value.slice(-maxPersistedTranscriptItems)) {
    if (!item || typeof item !== "object") {
      continue;
    }
    const candidate = item as Record<string, unknown>;
    if (candidate.type === "message") {
      const messageIndex = Number(candidate.messageIndex);
      if (Number.isInteger(messageIndex) && messageIndex >= 0 && messageIndex < messageCount) {
        refs.push({ type: "message", messageIndex });
      }
      continue;
    }
    if (candidate.type === "tool" && typeof candidate.toolId === "string") {
      if (toolIds.has(candidate.toolId)) {
        refs.push({ type: "tool", toolId: candidate.toolId });
      }
    }
  }
  return refs;
}

function sanitizeUsageSnapshot(value: unknown): UsageSnapshot | null {
  if (!value || typeof value !== "object") {
    return null;
  }
  const candidate = value as Record<string, unknown>;
  const used = Number(candidate.used);
  const size = Number(candidate.size);
  if (!Number.isFinite(used) || !Number.isFinite(size) || size <= 0) {
    return null;
  }
  return { used, size };
}
