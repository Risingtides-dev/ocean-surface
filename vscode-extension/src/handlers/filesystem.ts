import * as vscode from "vscode";
import type {
  ReadTextFileRequest,
  ReadTextFileResponse,
  WriteTextFileRequest,
  WriteTextFileResponse,
} from "@agentclientprotocol/sdk";
import { log, logError } from "../logger";

export interface AppliedFileEdit {
  path: string;
  oldText: string | null;
  newText: string;
  created: boolean;
  saved: boolean;
}

export type AppliedFileEditListener = (edit: AppliedFileEdit) => void;

/** ACP filesystem ops via VS Code — includes unsaved buffer content. */
export class FileSystemHandler {
  constructor(private readonly onAppliedEdit?: AppliedFileEditListener) {}

  async readTextFile(params: ReadTextFileRequest): Promise<ReadTextFileResponse> {
    log(`readTextFile: ${params.path}`);
    try {
      const uri = vscode.Uri.file(params.path);
      const openDoc = vscode.workspace.textDocuments.find(
        (doc) => doc.uri.fsPath === uri.fsPath,
      );

      let content: string;
      if (openDoc) {
        content = openDoc.getText();
      } else {
        const raw = await vscode.workspace.fs.readFile(uri);
        content = Buffer.from(raw).toString("utf-8");
      }

      if (
        (params.line !== undefined && params.line !== null) ||
        (params.limit !== undefined && params.limit !== null)
      ) {
        const lines = content.split("\n");
        const startLine = (params.line ?? 1) - 1;
        const endLine = params.limit ? startLine + params.limit : lines.length;
        content = lines.slice(startLine, endLine).join("\n");
      }

      return { content };
    } catch (error) {
      logError(`readTextFile failed: ${params.path}`, error);
      throw error;
    }
  }

  async writeTextFile(params: WriteTextFileRequest): Promise<WriteTextFileResponse> {
    log(`writeTextFile: ${params.path}`);
    try {
      const uri = vscode.Uri.file(params.path);
      const openDoc = vscode.workspace.textDocuments.find(
        (doc) => doc.uri.fsPath === uri.fsPath,
      );
      let doc: vscode.TextDocument;
      let oldText: string | null = null;

      if (openDoc) {
        oldText = openDoc.getText();
        const edit = new vscode.WorkspaceEdit();
        edit.replace(uri, fullDocumentRange(openDoc), params.content);
        const applied = await vscode.workspace.applyEdit(edit);
        if (!applied) {
          throw new Error(`VS Code rejected edit for ${params.path}`);
        }
        await openDoc.save();
        doc = openDoc;
      } else {
        oldText = await readExistingText(uri);
        await vscode.workspace.fs.writeFile(
          uri,
          Buffer.from(params.content, "utf-8"),
        );
        doc = await vscode.workspace.openTextDocument(uri);
      }

      await vscode.window.showTextDocument(doc, {
        preview: true,
        preserveFocus: true,
      });
      this.onAppliedEdit?.({
        path: uri.fsPath,
        oldText,
        newText: params.content,
        created: oldText === null,
        saved: true,
      });
      return {};
    } catch (error) {
      logError(`writeTextFile failed: ${params.path}`, error);
      throw error;
    }
  }
}

async function readExistingText(uri: vscode.Uri): Promise<string | null> {
  try {
    const raw = await vscode.workspace.fs.readFile(uri);
    return Buffer.from(raw).toString("utf-8");
  } catch {
    return null;
  }
}

function fullDocumentRange(document: vscode.TextDocument): vscode.Range {
  const lastLine = document.lineAt(document.lineCount - 1);
  return new vscode.Range(0, 0, document.lineCount - 1, lastLine.text.length);
}
