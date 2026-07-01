import * as vscode from "vscode";
import { ChatViewProvider, type OceanStatusSnapshot } from "./chatViewProvider";
import { OceanDiagnosticCodeActionProvider } from "./diagnosticCodeActions";
import { log } from "./logger";

let chatProvider: ChatViewProvider | undefined;
let statusBarItem: vscode.StatusBarItem | undefined;

interface OceanCommandMenuItem extends vscode.QuickPickItem {
  command?: string;
  args?: unknown[];
}

export function activate(context: vscode.ExtensionContext): void {
  log("Ocean extension activating");

  statusBarItem = vscode.window.createStatusBarItem(
    vscode.StatusBarAlignment.Left,
    95,
  );
  chatProvider = new ChatViewProvider(
    context.extensionUri,
    context.globalState,
    updateStatusBar,
  );
  updateStatusBar({
    connected: false,
    turnInProgress: false,
    cancelling: false,
    currentModelId: null,
    sessionId: null,
    sessionTitle: null,
  });
  statusBarItem.command = "ocean.commandMenu";
  statusBarItem.show();

  context.subscriptions.push(
    statusBarItem,
    vscode.window.registerWebviewViewProvider(
      ChatViewProvider.primaryViewType,
      chatProvider,
      { webviewOptions: { retainContextWhenHidden: true } },
    ),
    vscode.window.registerWebviewViewProvider(
      ChatViewProvider.panelViewType,
      chatProvider,
      { webviewOptions: { retainContextWhenHidden: true } },
    ),
    vscode.languages.registerCodeActionsProvider(
      { scheme: "file" },
      new OceanDiagnosticCodeActionProvider(),
      {
        providedCodeActionKinds:
          OceanDiagnosticCodeActionProvider.providedCodeActionKinds,
      },
    ),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("ocean.commandMenu", async () => {
      await showOceanCommandMenu();
    }),
    vscode.commands.registerCommand("ocean.openSidebar", async () => {
      await focusView(ChatViewProvider.primaryViewType, "ocean");
    }),
    vscode.commands.registerCommand("ocean.openPanel", async () => {
      await focusView(ChatViewProvider.panelViewType, "oceanPanel");
    }),
    vscode.commands.registerCommand("ocean.openEditor", () => {
      chatProvider?.openEditorPanel();
    }),
    vscode.commands.registerCommand("ocean.connect", async () => {
      await chatProvider?.connect();
    }),
    vscode.commands.registerCommand("ocean.newSession", async () => {
      await chatProvider?.newSession();
    }),
    vscode.commands.registerCommand("ocean.sendSelection", async () => {
      await chatProvider?.sendSelectionPrompt();
    }),
    vscode.commands.registerCommand("ocean.askCurrentFile", async () => {
      await chatProvider?.askCurrentFile();
    }),
    vscode.commands.registerCommand("ocean.askWorkspaceFiles", async (...args) => {
      await chatProvider?.askWorkspaceFiles(collectCommandUris(args));
    }),
    vscode.commands.registerCommand("ocean.fixDiagnostics", async (target) => {
      await chatProvider?.fixDiagnostics(target);
    }),
    vscode.commands.registerCommand("ocean.reviewWorkspaceChanges", async () => {
      await chatProvider?.reviewWorkspaceChanges();
    }),
    vscode.commands.registerCommand("ocean.showLastEdits", async () => {
      await chatProvider?.showLastEdits();
    }),
    vscode.commands.registerCommand("ocean.revertLastEdit", async () => {
      await chatProvider?.revertLastEdit();
    }),
    vscode.commands.registerCommand("ocean.revertEditSet", async () => {
      await chatProvider?.revertEditSet();
    }),
    vscode.commands.registerCommand("ocean.inlineAssist", async () => {
      await chatProvider?.inlineAssistSelection();
    }),
    vscode.commands.registerCommand("ocean.openRecentSession", async () => {
      await chatProvider?.openRecentSession();
    }),
    vscode.commands.registerCommand("ocean.refreshSessions", async () => {
      await chatProvider?.refreshRecentSessions();
    }),
    vscode.commands.registerCommand("ocean.renameSession", async () => {
      await chatProvider?.renameCurrentSession();
    }),
    vscode.commands.registerCommand("ocean.copyLastResponse", async () => {
      await chatProvider?.copyLastAssistantMessage();
    }),
    vscode.commands.registerCommand("ocean.copyTranscript", async () => {
      await chatProvider?.copyTranscript();
    }),
    vscode.commands.registerCommand("ocean.selectModel", async () => {
      await chatProvider?.selectModel();
    }),
    vscode.commands.registerCommand("ocean.selectThinkingLevel", async () => {
      await chatProvider?.selectThinkingLevel();
    }),
    vscode.commands.registerCommand("ocean.toggleEditorContext", async () => {
      await chatProvider?.toggleEditorContext();
    }),
    vscode.commands.registerCommand("ocean.setPermissionMode", async () => {
      await chatProvider?.selectPermissionMode();
    }),
    vscode.commands.registerCommand("ocean.toggleTrafficLog", async () => {
      await chatProvider?.toggleTrafficLog();
    }),
    vscode.commands.registerCommand("ocean.configureDaemonUrl", async () => {
      await chatProvider?.configureDaemonUrl();
    }),
    vscode.commands.registerCommand("ocean.configureAcpPath", async () => {
      await chatProvider?.configureAcpPath();
    }),
    vscode.commands.registerCommand("ocean.cancelTurn", async () => {
      await chatProvider?.cancelTurn();
    }),
    vscode.commands.registerCommand("ocean.disconnect", () => {
      chatProvider?.disconnect();
    }),
  );

  void vscode.commands.executeCommand("setContext", "ocean.connected", false);
  void vscode.commands.executeCommand("setContext", "ocean.turnInProgress", false);
}

export function deactivate(): void {
  chatProvider?.dispose();
  chatProvider = undefined;
  statusBarItem = undefined;
}

function updateStatusBar(status: OceanStatusSnapshot): void {
  if (!statusBarItem) {
    return;
  }
  if (status.cancelling) {
    statusBarItem.text = "Ocean: Cancelling";
  } else if (status.turnInProgress) {
    statusBarItem.text = "Ocean: Working";
  } else if (status.connected) {
    statusBarItem.text = status.currentModelId
      ? `Ocean: ${status.currentModelId}`
      : "Ocean: Live";
  } else {
    statusBarItem.text = "Ocean";
  }

  const detail = [
    status.connected ? "Connected" : "Offline",
    status.sessionTitle ? `Session: ${status.sessionTitle}` : null,
    status.currentModelId ? `Model: ${status.currentModelId}` : null,
  ]
    .filter(Boolean)
    .join("\n");
  statusBarItem.tooltip = `${detail || "Open Ocean"}\n\nClick for Ocean actions.`;
}

async function showOceanCommandMenu(): Promise<void> {
  const items: OceanCommandMenuItem[] = [
    { label: "Surface", kind: vscode.QuickPickItemKind.Separator },
    { label: "Open Editor", command: "ocean.openEditor" },
    { label: "Open Sidebar", command: "ocean.openSidebar" },
    { label: "Open Bottom Panel", command: "ocean.openPanel" },
    { label: "Session", kind: vscode.QuickPickItemKind.Separator },
    { label: "New Session", command: "ocean.newSession" },
    { label: "Open Recent Session", command: "ocean.openRecentSession" },
    { label: "Refresh Sessions", command: "ocean.refreshSessions" },
    { label: "Rename Current Session", command: "ocean.renameSession" },
    { label: "Copy Last Response", command: "ocean.copyLastResponse" },
    { label: "Copy Transcript", command: "ocean.copyTranscript" },
    { label: "Context", kind: vscode.QuickPickItemKind.Separator },
    { label: "Ask About Current File", command: "ocean.askCurrentFile" },
    { label: "Ask About Files", command: "ocean.askWorkspaceFiles" },
    { label: "Review Workspace Changes", command: "ocean.reviewWorkspaceChanges" },
    { label: "Fix Diagnostics", command: "ocean.fixDiagnostics" },
    { label: "Inline Assist", command: "ocean.inlineAssist" },
    { label: "Edits", kind: vscode.QuickPickItemKind.Separator },
    { label: "Show Last Edits", command: "ocean.showLastEdits" },
    { label: "Revert Last Edit", command: "ocean.revertLastEdit" },
    { label: "Revert Edit Set", command: "ocean.revertEditSet" },
    { label: "Runtime", kind: vscode.QuickPickItemKind.Separator },
    { label: "Select Model", command: "ocean.selectModel" },
    { label: "Set Thinking Level", command: "ocean.selectThinkingLevel" },
    { label: "Toggle Editor Context", command: "ocean.toggleEditorContext" },
    { label: "Set Permission Mode", command: "ocean.setPermissionMode" },
    { label: "Toggle Traffic Log", command: "ocean.toggleTrafficLog" },
    { label: "Connect", command: "ocean.connect" },
    { label: "Cancel Turn", command: "ocean.cancelTurn" },
    { label: "Disconnect", command: "ocean.disconnect" },
    { label: "Configure Daemon URL", command: "ocean.configureDaemonUrl" },
    { label: "Configure ACP Path", command: "ocean.configureAcpPath" },
    {
      label: "Settings",
      command: "workbench.action.openSettings",
      args: ["@ext:risingtides.ocean-surface"],
    },
  ];

  const picked = await vscode.window.showQuickPick(items, {
    title: "Ocean",
    placeHolder: "Choose an action",
    matchOnDescription: false,
    matchOnDetail: false,
    ignoreFocusOut: true,
  });
  if (!picked?.command) {
    return;
  }
  await vscode.commands.executeCommand(picked.command, ...(picked.args ?? []));
}

async function focusView(viewId: string, containerId: string): Promise<void> {
  try {
    await vscode.commands.executeCommand(`${viewId}.focus`);
    return;
  } catch {
    // Fall through to the container command. Cursor/VS Code exposes this for
    // contributed view containers even when the view focus command is absent.
  }
  await vscode.commands.executeCommand(`workbench.view.extension.${containerId}`);
}

function collectCommandUris(values: unknown[]): vscode.Uri[] {
  const uris: vscode.Uri[] = [];
  for (const value of values) {
    if (value instanceof vscode.Uri) {
      uris.push(value);
      continue;
    }
    if (Array.isArray(value)) {
      uris.push(...collectCommandUris(value));
    }
  }
  const seen = new Set<string>();
  return uris.filter((uri) => {
    if (seen.has(uri.toString())) {
      return false;
    }
    seen.add(uri.toString());
    return true;
  });
}
