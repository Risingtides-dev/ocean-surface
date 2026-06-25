import type {
  Agent,
  Client,
  CreateTerminalRequest,
  CreateTerminalResponse,
  KillTerminalRequest,
  KillTerminalResponse,
  ReadTextFileRequest,
  ReadTextFileResponse,
  ReleaseTerminalRequest,
  ReleaseTerminalResponse,
  RequestPermissionRequest,
  RequestPermissionResponse,
  SessionNotification,
  TerminalOutputRequest,
  TerminalOutputResponse,
  WaitForTerminalExitRequest,
  WaitForTerminalExitResponse,
  WriteTextFileRequest,
  WriteTextFileResponse,
} from "@agentclientprotocol/sdk";
import { FileSystemHandler } from "./handlers/filesystem";
import { TerminalHandler } from "./handlers/terminal";
import { PermissionHandler } from "./handlers/permissions";
import { log } from "./logger";

export type SessionUpdateListener = (update: SessionNotification) => void;

export class OceanAcpClient implements Client {
  private agent: Agent | null = null;

  constructor(
    private readonly fsHandler: FileSystemHandler,
    private readonly terminalHandler: TerminalHandler,
    private readonly permissionHandler: PermissionHandler,
    private readonly onSessionUpdate: SessionUpdateListener,
  ) {}

  setAgent(agent: Agent): void {
    this.agent = agent;
  }

  getAgent(): Agent | null {
    return this.agent;
  }

  async requestPermission(
    params: RequestPermissionRequest,
  ): Promise<RequestPermissionResponse> {
    return this.permissionHandler.requestPermission(params);
  }

  async sessionUpdate(params: SessionNotification): Promise<void> {
    this.onSessionUpdate(params);
  }

  async readTextFile(params: ReadTextFileRequest): Promise<ReadTextFileResponse> {
    log(`Client.readTextFile: ${params.path}`);
    return this.fsHandler.readTextFile(params);
  }

  async writeTextFile(params: WriteTextFileRequest): Promise<WriteTextFileResponse> {
    log(`Client.writeTextFile: ${params.path}`);
    return this.fsHandler.writeTextFile(params);
  }

  async createTerminal(params: CreateTerminalRequest): Promise<CreateTerminalResponse> {
    return this.terminalHandler.createTerminal(params);
  }

  async terminalOutput(params: TerminalOutputRequest): Promise<TerminalOutputResponse> {
    return this.terminalHandler.terminalOutput(params);
  }

  async waitForTerminalExit(
    params: WaitForTerminalExitRequest,
  ): Promise<WaitForTerminalExitResponse> {
    return this.terminalHandler.waitForTerminalExit(params);
  }

  async killTerminal(params: KillTerminalRequest): Promise<KillTerminalResponse> {
    return this.terminalHandler.killTerminal(params);
  }

  async releaseTerminal(params: ReleaseTerminalRequest): Promise<ReleaseTerminalResponse> {
    return this.terminalHandler.releaseTerminal(params);
  }
}
