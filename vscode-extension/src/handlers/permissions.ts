import * as vscode from "vscode";
import type {
  RequestPermissionRequest,
  RequestPermissionResponse,
} from "@agentclientprotocol/sdk";
import { log } from "../logger";

export class PermissionHandler {
  async requestPermission(
    params: RequestPermissionRequest,
  ): Promise<RequestPermissionResponse> {
    const config = vscode.workspace.getConfiguration("ocean");
    const autoApprove = config.get<string>("autoApprovePermissions", "ask");
    const title = params.toolCall?.title ?? "Permission request";
    log(`requestPermission: ${title} (autoApprove=${autoApprove})`);

    if (autoApprove === "allowAll") {
      const allowOption = params.options.find(
        (option) => option.kind === "allow_once" || option.kind === "allow_always",
      );
      if (allowOption) {
        return {
          outcome: { outcome: "selected", optionId: allowOption.optionId },
        };
      }
    }

    const items = params.options.map((option) => ({
      label: `${option.kind.startsWith("allow") ? "$(check)" : "$(x)"} ${option.name}`,
      description: option.kind,
      optionId: option.optionId,
    }));

    const selection = await vscode.window.showQuickPick(items, {
      placeHolder: title,
      title: "Ocean agent permission",
      ignoreFocusOut: true,
    });

    if (!selection) {
      return { outcome: { outcome: "cancelled" } };
    }

    return {
      outcome: { outcome: "selected", optionId: selection.optionId },
    };
  }
}
