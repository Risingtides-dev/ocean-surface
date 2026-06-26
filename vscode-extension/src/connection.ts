import { spawn, type ChildProcess } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { Readable, Writable } from "node:stream";
import {
  ClientSideConnection,
  ndJsonStream,
  PROTOCOL_VERSION,
  type InitializeResponse,
  type NewSessionResponse,
  type PromptResponse,
  type Stream,
} from "@agentclientprotocol/sdk";
import * as vscode from "vscode";
import { OceanAcpClient, type SessionUpdateListener } from "./acpClient";
import { FileSystemHandler } from "./handlers/filesystem";
import { TerminalHandler } from "./handlers/terminal";
import { PermissionHandler } from "./handlers/permissions";
import { log, logError, logTraffic } from "./logger";
import { workspaceRoot } from "./editorContext";
import { version as extensionVersion } from "../package.json";

export type ConnectionStateListener = (connected: boolean) => void;

export class OceanConnection {
  private process: ChildProcess | null = null;
  private connection: ClientSideConnection | null = null;
  private client: OceanAcpClient | null = null;
  private initResponse: InitializeResponse | null = null;
  private sessionId: string | null = null;
  private readonly fsHandler = new FileSystemHandler();
  private readonly terminalHandler = new TerminalHandler();
  private readonly permissionHandler = new PermissionHandler();

  constructor(
    private readonly onSessionUpdate: SessionUpdateListener,
    private readonly onConnectionStateChange?: ConnectionStateListener,
  ) {}

  get connected(): boolean {
    return this.connection !== null;
  }

  get currentSessionId(): string | null {
    return this.sessionId;
  }

  get agentInfo(): InitializeResponse["agentInfo"] | undefined {
    return this.initResponse?.agentInfo;
  }

  async connect(): Promise<void> {
    if (this.connection) {
      return;
    }

    await this.ensureDaemonHealthy();

    const binary = resolveOceanAcpPath();
    const config = vscode.workspace.getConfiguration("ocean");
    const daemonUrl = config.get<string>("daemon.url", "http://127.0.0.1:4780");

    log(`Spawning ocean-acp: ${binary}`);
    this.process = spawn(binary, ["--daemon-url", daemonUrl], {
      stdio: ["pipe", "pipe", "pipe"],
      env: {
        ...process.env,
        OCEAN_ACP_CLIENT_TYPE: "acp-vscode",
        OCEAN_ACP_LOG: process.env.OCEAN_ACP_LOG ?? "info",
      },
    });

    this.process.stderr?.on("data", (chunk: Buffer) => {
      log(`[ocean-acp stderr] ${chunk.toString().trim()}`);
    });

    this.process.on("exit", (code, signal) => {
      log(`ocean-acp exited (code=${code}, signal=${signal ?? "none"})`);
      this.connection = null;
      this.sessionId = null;
      void vscode.commands.executeCommand("setContext", "ocean.connected", false);
      this.onConnectionStateChange?.(false);
    });

    if (!this.process.stdout || !this.process.stdin) {
      throw new Error("ocean-acp missing stdio streams");
    }

    const readable = Readable.toWeb(this.process.stdout) as ReadableStream;
    const writable = Writable.toWeb(this.process.stdin) as WritableStream;
    const stream = this.tapStream(ndJsonStream(writable, readable));

    this.client = new OceanAcpClient(
      this.fsHandler,
      this.terminalHandler,
      this.permissionHandler,
      this.onSessionUpdate,
    );

    this.connection = new ClientSideConnection((agent) => {
      this.client?.setAgent(agent);
      return this.client!;
    }, stream);

    this.initResponse = await this.connection.initialize({
      protocolVersion: PROTOCOL_VERSION,
      clientInfo: {
        name: "ocean-vscode",
        version: extensionVersion,
      },
      clientCapabilities: {
        fs: { readTextFile: true, writeTextFile: true },
        terminal: true,
      },
    });

    log(
      `Connected to ${this.initResponse.agentInfo?.name ?? "Ocean"} ` +
        `v${this.initResponse.agentInfo?.version ?? "?"}`,
    );
    await vscode.commands.executeCommand("setContext", "ocean.connected", true);
    this.onConnectionStateChange?.(true);
  }

  async newSession(): Promise<NewSessionResponse> {
    if (!this.connection) {
      throw new Error("Not connected");
    }
    const cwd = workspaceRoot();
    const response = await this.connection.newSession({ cwd, mcpServers: [] });
    this.sessionId = response.sessionId;
    log(`New session: ${this.sessionId} (cwd=${cwd})`);
    return response;
  }

  async prompt(text: string): Promise<PromptResponse> {
    if (!this.connection || !this.sessionId) {
      throw new Error("No active session");
    }
    await vscode.commands.executeCommand("setContext", "ocean.turnInProgress", true);
    try {
      return await this.connection.prompt({
        sessionId: this.sessionId,
        prompt: [{ type: "text", text }],
      });
    } finally {
      await vscode.commands.executeCommand("setContext", "ocean.turnInProgress", false);
    }
  }

  async cancelTurn(): Promise<void> {
    if (!this.connection || !this.sessionId) {
      return;
    }
    await this.connection.cancel({ sessionId: this.sessionId });
  }

  disconnect(): void {
    this.connection = null;
    this.sessionId = null;
    this.initResponse = null;
    this.terminalHandler.dispose();
    if (this.process) {
      this.process.kill();
      this.process = null;
    }
    void vscode.commands.executeCommand("setContext", "ocean.connected", false);
    void vscode.commands.executeCommand("setContext", "ocean.turnInProgress", false);
    this.onConnectionStateChange?.(false);
  }

  private tapStream(stream: Stream): Stream {
    const config = vscode.workspace.getConfiguration("ocean");
    if (!config.get<boolean>("logTraffic", false)) {
      return stream;
    }

    const sendTap = new TransformStream({
      transform(chunk, controller) {
        logTraffic("send", chunk);
        controller.enqueue(chunk);
      },
    });
    const recvTap = new TransformStream({
      transform(chunk, controller) {
        logTraffic("recv", chunk);
        controller.enqueue(chunk);
      },
    });

    void sendTap.readable
      .pipeTo(stream.writable)
      .catch((error) => logError("ACP send tap error", error));
    void stream.readable
      .pipeTo(recvTap.writable)
      .catch((error) => logError("ACP recv tap error", error));

    return { writable: sendTap.writable, readable: recvTap.readable };
  }

  private async ensureDaemonHealthy(): Promise<void> {
    const config = vscode.workspace.getConfiguration("ocean");
    const daemonUrl = config.get<string>("daemon.url", "http://127.0.0.1:4780");
    const healthUrl = `${daemonUrl.replace(/\/$/, "")}/health`;

    try {
      const response = await fetch(healthUrl, { signal: AbortSignal.timeout(3000) });
      if (!response.ok) {
        throw new Error(`HTTP ${response.status}`);
      }
    } catch (error) {
      const message =
        "Ocean daemon is not reachable. Start it from ocean-os: `./target/release/ocean-daemon`";
      const pick = await vscode.window.showErrorMessage(message, "Copy command");
      if (pick === "Copy command") {
        await vscode.env.clipboard.writeText("cargo run -p ocean-daemon --release");
      }
      throw new Error(`${message} (${String(error)})`);
    }
  }
}

function resolveOceanAcpPath(): string {
  const config = vscode.workspace.getConfiguration("ocean");
  const configured = config.get<string>("acp.path", "").trim();
  if (configured && existsSync(configured)) {
    return configured;
  }

  const candidates = [
    join(homedir(), ".cargo", "bin", "ocean-acp"),
    join(homedir(), "dev", "ocean-os", "target", "release", "ocean-acp"),
    join(homedir(), "dev", "ocean-os", "target", "debug", "ocean-acp"),
    join(homedir(), "ocean-os", "target", "release", "ocean-acp"),
    join(homedir(), "ocean-os", "target", "debug", "ocean-acp"),
    join(homedir(), "ocean-os-repo", "target", "release", "ocean-acp"),
    join(homedir(), "ocean-os-repo", "target", "debug", "ocean-acp"),
    resolve(__dirname, "..", "..", "..", "ocean-os", "target", "release", "ocean-acp"),
    resolve(__dirname, "..", "..", "..", "ocean-os", "target", "debug", "ocean-acp"),
    "ocean-acp",
  ];

  for (const candidate of candidates) {
    if (candidate === "ocean-acp" || existsSync(candidate)) {
      return candidate;
    }
  }

  throw new Error(
    "ocean-acp binary not found. Build it: `cargo build -p ocean-acp --release` in ocean-os, " +
      "then set ocean.acp.path in settings.",
  );
}
