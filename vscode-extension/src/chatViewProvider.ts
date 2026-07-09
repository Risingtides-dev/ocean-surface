import * as vscode from "vscode";
import type { SessionNotification } from "@agentclientprotocol/sdk";
import { OceanConnection } from "./connection";
import { augmentPrompt } from "./editorContext";
import { log, logError } from "./logger";

interface ChatMessage {
  role: "user" | "assistant" | "system" | "tool";
  content: string;
  streaming?: boolean;
}

interface ToolActivity {
  id: string;
  title: string;
  status: "pending" | "in_progress" | "completed" | "failed";
  kind?: string;
}

interface InlineAssistChoice extends vscode.QuickPickItem {
  instruction?: string;
  custom?: boolean;
}

export class ChatViewProvider implements vscode.WebviewViewProvider {
  public static readonly primaryViewType = "ocean.chat";
  public static readonly panelViewType = "ocean.panelChat";
  public static readonly auxiliaryViewType = "ocean.auxChat";
  public static readonly editorViewType = "ocean.chatEditor";

  private editorPanel: vscode.WebviewPanel | undefined;
  private readonly webviews = new Set<vscode.Webview>();
  private readonly connection: OceanConnection;
  private messages: ChatMessage[] = [];
  private streamingAssistantIndex: number | null = null;
  private cancellingTurn = false;
  private readonly tools = new Map<string, ToolActivity>();

  constructor(private readonly extensionUri: vscode.Uri) {
    this.connection = new OceanConnection(
      (update) => this.handleSessionUpdate(update),
      () => this.postState(),
    );
  }

  resolveWebviewView(
    webviewView: vscode.WebviewView,
    _context: vscode.WebviewViewResolveContext,
    _token: vscode.CancellationToken,
  ): void {
    this.attachWebview(webviewView.webview, webviewView.onDidDispose);
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
    this.attachWebview(panel.webview, panel.onDidDispose, () => {
      if (this.editorPanel === panel) {
        this.editorPanel = undefined;
      }
    });
  }

  async connect(): Promise<void> {
    try {
      this.pushSystem("Connecting to Ocean…");
      await this.connection.connect();
      await this.connection.newSession();
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
      await this.connection.newSession();
      this.messages = [];
      this.pushSystem("New session started.");
    } catch (error) {
      logError("New session failed", error);
      this.pushSystem(`New session failed: ${String(error)}`);
    }
  }

  async sendPrompt(text: string, skipAugment = false): Promise<void> {
    const trimmed = text.trim();
    if (!trimmed) {
      return;
    }

    const prompt = skipAugment ? trimmed : augmentPrompt(trimmed);
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

  async cancelTurn(): Promise<void> {
    if (this.streamingAssistantIndex !== null) {
      this.cancellingTurn = true;
      this.postState();
    }
    await this.connection.cancelTurn();
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
    this.connection.disconnect();
  }

  private attachWebview(
    webview: vscode.Webview,
    onDidDispose: vscode.Event<void>,
    onDispose?: () => void,
  ): void {
    webview.options = {
      enableScripts: true,
      localResourceRoots: [vscode.Uri.joinPath(this.extensionUri, "media")],
    };
    webview.html = this.renderHtml(webview);
    this.webviews.add(webview);
    const messageDisposable = webview.onDidReceiveMessage((message) => {
      void this.handleWebviewMessage(message);
    });
    const disposeSubscription = onDidDispose(() => {
      this.webviews.delete(webview);
      messageDisposable.dispose();
      disposeSubscription.dispose();
      onDispose?.();
    });
    this.postState();
  }

  private async handleWebviewMessage(message: { type?: unknown; text?: unknown }): Promise<void> {
    switch (message.type) {
      case "ready":
        this.postState();
        break;
      case "connect":
        await this.connect();
        break;
      case "send":
        await this.sendPrompt(String(message.text ?? ""));
        break;
      case "inlineAssist":
        await this.inlineAssistSelection();
        break;
      case "newSession":
        await this.newSession();
        break;
      case "cancel":
        await this.cancelTurn();
        break;
      default:
        break;
    }
  }

  private handleSessionUpdate(notification: SessionNotification): void {
    const update = notification.update;
    if (this.streamingAssistantIndex === null) {
      return;
    }

    if (
      update.sessionUpdate === "agent_message_chunk" &&
      update.content.type === "text"
    ) {
      if (this.cancellingTurn && isCancellationText(update.content.text)) {
        return;
      }
      this.messages[this.streamingAssistantIndex].content += update.content.text;
      this.postState();
      return;
    }

    if (update.sessionUpdate === "agent_thought_chunk" && update.content.type === "text") {
      log(`thinking: ${update.content.text.slice(0, 80)}`);
      return;
    }

    if (update.sessionUpdate === "tool_call") {
      this.upsertTool({
        id: update.toolCallId,
        title: update.title ?? update.toolCallId,
        status: update.status ?? "pending",
        kind: update.kind ? String(update.kind) : undefined,
      });
      this.postState();
      return;
    }

    if (update.sessionUpdate === "tool_call_update") {
      const existing = this.tools.get(update.toolCallId);
      this.upsertTool({
        id: update.toolCallId,
        title: update.title ?? existing?.title ?? update.toolCallId,
        status: update.status ?? existing?.status ?? "in_progress",
        kind: update.kind ? String(update.kind) : existing?.kind,
      });
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
      await this.connection.newSession();
    }

    this.cancellingTurn = false;
    this.tools.clear();
    this.messages.push({ role: "user", content: displayText });
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
        return assistantMessage.content;
      }
      if (!assistantMessage.content.trim()) {
        assistantMessage.content =
          `Turn finished without assistant output (stop: ${response.stopReason}).`;
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
      throw error;
    } finally {
      if (this.streamingAssistantIndex === assistantIndex) {
        this.streamingAssistantIndex = null;
      }
      this.cancellingTurn = false;
      this.tools.clear();
      this.postState();
    }
  }

  private upsertTool(tool: ToolActivity): void {
    this.tools.set(tool.id, tool);
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

  private pushSystem(content: string): void {
    this.messages.push({ role: "system", content });
    this.postState();
  }

  private postState(): void {
    const state = {
      type: "state",
      connected: this.connection.connected,
      sessionId: this.connection.currentSessionId,
      turnInProgress: this.streamingAssistantIndex !== null,
      cancelling: this.cancellingTurn,
      tools: [...this.tools.values()],
      messages: this.messages,
    };
    for (const webview of this.webviews) {
      void webview.postMessage(state).then(
        () => undefined,
        () => this.webviews.delete(webview),
      );
    }
  }

  private renderHtml(webview: vscode.Webview): string {
    const scriptUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this.extensionUri, "media", "chat.js"),
    );
    const styleUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this.extensionUri, "media", "chat.css"),
    );
    const nonce = String(Date.now());

    return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource}; script-src 'nonce-${nonce}';" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <link rel="stylesheet" href="${styleUri}" />
  <title>Ocean</title>
</head>
<body>
  <header id="topbar">
    <div class="brand">
      <span class="mark" aria-hidden="true"></span>
      <div class="brand-copy">
        <span class="brand-kicker">LOCAL ACP</span>
        <strong>Ocean</strong>
        <span id="status">Disconnected</span>
      </div>
    </div>
    <div class="top-actions">
      <button type="button" id="connect" class="secondary" title="Connect to Ocean">Connect</button>
      <button type="button" id="newSession" class="ghost" title="Start a new Ocean session">New</button>
    </div>
  </header>
  <div id="activity" hidden></div>
  <div id="messages"></div>
  <form id="composer">
    <textarea id="input" rows="1" placeholder="Ask Ocean" title="Enter sends. Shift+Enter inserts a line."></textarea>
    <div class="actions">
      <button type="submit" title="Send with Enter">Send</button>
      <button type="button" id="inlineAssist" class="secondary" title="Run inline assist on the active editor">Inline</button>
      <button type="button" id="cancel" class="ghost">Stop</button>
    </div>
  </form>
  <script nonce="${nonce}" src="${scriptUri}"></script>
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

function normalizeInlineReplacement(text: string): string {
  const value = text.replace(/\r\n/g, "\n");
  const fenced = value.trim().match(/^```(?:[a-zA-Z0-9_-]+)?\n([\s\S]*?)\n```$/);
  if (fenced) {
    return fenced[1];
  }
  return value.replace(/^\n+/, "").replace(/\n+$/, "");
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
