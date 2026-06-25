import * as vscode from "vscode";
import { ChatViewProvider } from "./chatViewProvider";
import { log } from "./logger";

let chatProvider: ChatViewProvider | undefined;

export function activate(context: vscode.ExtensionContext): void {
  log("Ocean extension activating");

  chatProvider = new ChatViewProvider(context.extensionUri);
  context.subscriptions.push(
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
    vscode.window.registerWebviewViewProvider(
      ChatViewProvider.auxiliaryViewType,
      chatProvider,
      { webviewOptions: { retainContextWhenHidden: true } },
    ),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("ocean.openSidebar", async () => {
      await focusView(ChatViewProvider.primaryViewType, "ocean");
    }),
    vscode.commands.registerCommand("ocean.openPanel", async () => {
      await focusView(ChatViewProvider.panelViewType, "oceanPanel");
    }),
    vscode.commands.registerCommand("ocean.openAuxiliary", async () => {
      await focusView(ChatViewProvider.auxiliaryViewType, "oceanAux");
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
    vscode.commands.registerCommand("ocean.inlineAssist", async () => {
      await chatProvider?.inlineAssistSelection();
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
