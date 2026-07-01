import * as vscode from "vscode";
import type { DiagnosticFixTarget } from "./chatViewProvider";

export class OceanDiagnosticCodeActionProvider implements vscode.CodeActionProvider {
  static readonly providedCodeActionKinds = [vscode.CodeActionKind.QuickFix];

  provideCodeActions(
    document: vscode.TextDocument,
    _range: vscode.Range | vscode.Selection,
    context: vscode.CodeActionContext,
  ): vscode.CodeAction[] {
    const diagnostics = relevantDiagnostics(context.diagnostics);
    if (!diagnostics.length) {
      return [];
    }

    const actions = diagnostics.map((diagnostic) =>
      diagnosticCodeAction(document.uri, diagnostic),
    );

    const fileDiagnostics = relevantDiagnostics(
      vscode.languages.getDiagnostics(document.uri),
    );
    if (fileDiagnostics.length > 1) {
      actions.push(allDiagnosticsCodeAction(document.uri, fileDiagnostics));
    }

    return actions;
  }
}

function diagnosticCodeAction(
  uri: vscode.Uri,
  diagnostic: vscode.Diagnostic,
): vscode.CodeAction {
  const title = `Ocean: Fix ${singleLine(diagnostic.message, 56)}`;
  const action = new vscode.CodeAction(title, vscode.CodeActionKind.QuickFix);
  action.diagnostics = [diagnostic];
  action.command = {
    command: "ocean.fixDiagnostics",
    title,
    arguments: [
      {
        uri,
        diagnostics: [diagnostic],
      } satisfies DiagnosticFixTarget,
    ],
  };
  return action;
}

function allDiagnosticsCodeAction(
  uri: vscode.Uri,
  diagnostics: vscode.Diagnostic[],
): vscode.CodeAction {
  const title = `Ocean: Fix all ${diagnostics.length} diagnostics in file`;
  const action = new vscode.CodeAction(title, vscode.CodeActionKind.QuickFix);
  action.diagnostics = diagnostics;
  action.command = {
    command: "ocean.fixDiagnostics",
    title,
    arguments: [
      {
        uri,
        diagnostics,
        all: true,
      } satisfies DiagnosticFixTarget,
    ],
  };
  return action;
}

function relevantDiagnostics(diagnostics: readonly vscode.Diagnostic[]): vscode.Diagnostic[] {
  return diagnostics.filter(
    (diagnostic) => diagnostic.severity !== vscode.DiagnosticSeverity.Hint,
  );
}

function singleLine(value: string, maxChars: number): string {
  const normalized = value.replace(/\s+/g, " ").trim();
  if (normalized.length <= maxChars) {
    return normalized;
  }
  return `${normalized.slice(0, maxChars - 3).trim()}...`;
}
