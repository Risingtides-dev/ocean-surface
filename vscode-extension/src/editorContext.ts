import * as vscode from "vscode";
import * as path from "node:path";

export interface EditorContextPayload {
  surface: "vscode_extension";
  workspace_folders: string[];
  active_editor: {
    path: string;
    language_id: string;
    is_dirty: boolean;
    line_count: number;
    cursor: { line: number; character: number };
    selection: string;
    visible_range: { start_line: number; end_line: number };
  } | null;
  open_tabs: Array<{ path: string; language_id: string; is_dirty: boolean }>;
  diagnostics: Array<{
    path: string;
    severity: string;
    message: string;
    line: number;
  }>;
  git_branch: string | null;
}

export function workspaceRoot(): string {
  const folder = vscode.workspace.workspaceFolders?.[0];
  return folder?.uri.fsPath ?? process.cwd();
}

export function collectEditorContext(): EditorContextPayload {
  const editor = vscode.window.activeTextEditor;
  const folders =
    vscode.workspace.workspaceFolders?.map((f) => f.uri.fsPath) ?? [];

  let activeEditor: EditorContextPayload["active_editor"] = null;
  if (editor) {
    const doc = editor.document;
    const selectionText = doc.getText(editor.selection);
    const visible = editor.visibleRanges[0];
    activeEditor = {
      path: doc.uri.fsPath,
      language_id: doc.languageId,
      is_dirty: doc.isDirty,
      line_count: doc.lineCount,
      cursor: {
        line: editor.selection.active.line + 1,
        character: editor.selection.active.character + 1,
      },
      selection: selectionText,
      visible_range: visible
        ? {
            start_line: visible.start.line + 1,
            end_line: visible.end.line + 1,
          }
        : { start_line: 1, end_line: 1 },
    };
  }

  const openTabs = vscode.window.tabGroups.all.flatMap((group) =>
    group.tabs
      .map((tab) => {
        const input = tab.input;
        if (!(input instanceof vscode.TabInputText)) {
          return null;
        }
        return {
          path: input.uri.fsPath,
          language_id: "",
          is_dirty: tab.isDirty,
        };
      })
      .filter((tab): tab is NonNullable<typeof tab> => tab !== null),
  );

  const diagnostics: EditorContextPayload["diagnostics"] = [];
  for (const [uri, diags] of vscode.languages.getDiagnostics()) {
    for (const diag of diags.slice(0, 5)) {
      diagnostics.push({
        path: uri.fsPath,
        severity: vscode.DiagnosticSeverity[diag.severity] ?? "Unknown",
        message: diag.message,
        line: diag.range.start.line + 1,
      });
    }
  }

  return {
    surface: "vscode_extension",
    workspace_folders: folders,
    active_editor: activeEditor,
    open_tabs: openTabs.slice(0, 20),
    diagnostics: diagnostics.slice(0, 15),
    git_branch: readGitBranch(folders[0]),
  };
}

function readGitBranch(root: string | undefined): string | null {
  if (!root) {
    return null;
  }
  const gitExt = vscode.extensions.getExtension("vscode.git");
  if (!gitExt?.isActive) {
    return null;
  }
  try {
    const git = gitExt.exports.getAPI(1);
    const repo = git.repositories.find(
      (r: { rootUri: vscode.Uri }) => r.rootUri.fsPath === root,
    );
    return repo?.state?.HEAD?.name ?? null;
  } catch {
    return null;
  }
}

export function formatEditorContextBlock(context: EditorContextPayload): string {
  const lines: string[] = [
    "<editor_context>",
    `surface: ${context.surface}`,
    `workspace: ${context.workspace_folders.join(", ") || "(none)"}`,
  ];

  if (context.git_branch) {
    lines.push(`git_branch: ${context.git_branch}`);
  }

  if (context.active_editor) {
    const e = context.active_editor;
    lines.push(
      `active_file: ${e.path}`,
      `language: ${e.language_id}`,
      `cursor: line ${e.cursor.line}, col ${e.cursor.character}`,
      `dirty: ${e.is_dirty}`,
    );
    if (e.selection.trim()) {
      lines.push("selection:", "```", e.selection, "```");
    }
  }

  if (context.open_tabs.length > 0) {
    lines.push("open_tabs:");
    for (const tab of context.open_tabs) {
      lines.push(`- ${tab.path}${tab.is_dirty ? " (unsaved)" : ""}`);
    }
  }

  if (context.diagnostics.length > 0) {
    lines.push("diagnostics:");
    for (const diag of context.diagnostics) {
      lines.push(
        `- [${diag.severity}] ${path.basename(diag.path)}:${diag.line} ${diag.message}`,
      );
    }
  }

  lines.push("</editor_context>");
  return lines.join("\n");
}

export function augmentPrompt(userText: string): string {
  const config = vscode.workspace.getConfiguration("ocean");
  if (!config.get<boolean>("injectEditorContext", true)) {
    return userText;
  }
  const block = formatEditorContextBlock(collectEditorContext());
  return `${block}\n\n${userText}`;
}
