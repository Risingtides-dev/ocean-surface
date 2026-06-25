import * as vscode from "vscode";

let channel: vscode.OutputChannel | undefined;

export function getOutputChannel(): vscode.OutputChannel {
  if (!channel) {
    channel = vscode.window.createOutputChannel("Ocean");
  }
  return channel;
}

export function log(message: string): void {
  getOutputChannel().appendLine(`[${new Date().toISOString()}] ${message}`);
}

export function logError(message: string, error?: unknown): void {
  const detail =
    error instanceof Error ? error.stack ?? error.message : String(error ?? "");
  getOutputChannel().appendLine(
    `[${new Date().toISOString()}] ERROR ${message}${detail ? `: ${detail}` : ""}`,
  );
}

export function logTraffic(direction: "send" | "recv", payload: unknown): void {
  const config = vscode.workspace.getConfiguration("ocean");
  if (!config.get<boolean>("logTraffic", false)) {
    return;
  }
  getOutputChannel().appendLine(
    `[ACP ${direction}] ${JSON.stringify(payload)}`,
  );
}
