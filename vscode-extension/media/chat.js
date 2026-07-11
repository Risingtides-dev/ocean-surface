(function () {
  const vscode = acquireVsCodeApi();

  const activityEl = document.getElementById("activity");
  const messagesEl = document.getElementById("messages");
  const form = document.getElementById("composer");
  const input = document.getElementById("input");
  const mentionMenu = document.getElementById("mentionMenu");
  const connectButton = document.getElementById("connect");
  const newSessionButton = document.getElementById("newSession");
  const cancelButton = document.getElementById("cancel");
  const statusEl = document.getElementById("status");
  const modelSelect = document.getElementById("modelSelect");
  const sessionSelect = document.getElementById("sessionSelect");
  const thinkingSelect = document.getElementById("thinkingSelect");
  const contextValue = document.getElementById("contextValue");
  const daemonUrlInput = document.getElementById("daemonUrlInput");
  const acpPathInput = document.getElementById("acpPathInput");
  const permissionSelect = document.getElementById("permissionSelect");
  const contextToggle = document.getElementById("contextToggle");
  const trafficToggle = document.getElementById("trafficToggle");

  const roleLabels = {
    user: "YOU",
    assistant: "OCEAN",
    system: "SYS",
    tool: "TOOL",
  };

  const maxComposerHistory = 30;
  const mentionOptions = [
    { token: "@current", label: "Current file", detail: "active buffer", aliases: ["@active", "@file"] },
    { token: "@selection", label: "Selection", detail: "selected text", aliases: ["@sel"] },
    { token: "@tabs", label: "Open tabs", detail: "visible workspace files", aliases: ["@open", "@editors"] },
    { token: "@workspace", label: "Workspace map", detail: "bounded file tree", aliases: ["@codebase", "@tree", "@project"] },
    { token: "@changes", label: "Workspace changes", detail: "git status and diff", aliases: ["@git", "@diff"] },
    { token: "@problems", label: "Problems", detail: "diagnostics", aliases: ["@diagnostics"] },
    { token: "@terminal", label: "Terminal", detail: "recent output", aliases: ["@term", "@cmd"] },
  ];

  const slashOptions = [
    {
      token: "/review",
      label: "/review",
      detail: "review current file",
      insert: "Review @current for bugs, edge cases, and missing tests.",
      aliases: ["audit", "bugs"],
    },
    {
      token: "/changes",
      label: "/changes",
      detail: "review git diff",
      insert: "Review @changes for regressions, risky edits, and missing tests.",
      aliases: ["diff", "git"],
    },
    {
      token: "/fix",
      label: "/fix",
      detail: "fix diagnostics",
      insert: "Fix @problems in the current workspace.",
      aliases: ["problems", "diagnostics"],
    },
    {
      token: "/explain",
      label: "/explain",
      detail: "explain current file",
      insert: "Explain @current and call out the important control flow.",
      aliases: ["current", "file"],
    },
    {
      token: "/workspace",
      label: "/workspace",
      detail: "map codebase",
      insert: "Use @workspace to map the project structure and identify the main entry points.",
      aliases: ["tree", "codebase", "project"],
    },
    {
      token: "/tabs",
      label: "/tabs",
      detail: "review open files",
      insert: "Review @tabs as the current working set.",
      aliases: ["open", "buffers"],
    },
    {
      token: "/terminal",
      label: "/terminal",
      detail: "inspect output",
      insert: "Use @terminal to explain the latest command output and the next fix.",
      aliases: ["term", "cmd"],
    },
    {
      token: "/selection",
      label: "/selection",
      detail: "work on selection",
      insert: "Rewrite @selection to be clearer and more direct.",
      aliases: ["sel", "rewrite"],
    },
  ];

  let latestState = {};
  let webviewState = normalizeWebviewState(vscode.getState());
  let composerHistory = webviewState.composerHistory;
  let composerHistoryIndex = null;
  let composerDraftBeforeHistory = "";
  let forceTranscriptScroll = true;
  let mentionState = {
    open: false,
    start: -1,
    active: 0,
    options: [],
  };
  let mentionRequestSeq = 0;
  let mentionSearchTimer = 0;
  let mentionFileQuery = "";
  let mentionFileOptions = [];
  let slashState = {
    open: false,
    start: -1,
    active: 0,
    options: [],
  };

  input.value = webviewState.composerDraft;

  function normalizeWebviewState(value) {
    const state = value && typeof value === "object" ? value : {};
    return {
      ...state,
      composerDraft: typeof state.composerDraft === "string" ? state.composerDraft : "",
      composerHistory: normalizeComposerHistory(state.composerHistory),
    };
  }

  function normalizeComposerHistory(value) {
    if (!Array.isArray(value)) {
      return [];
    }
    const seen = new Set();
    const history = [];
    for (const item of value) {
      const text = String(item || "").trim();
      if (!text || seen.has(text)) {
        continue;
      }
      seen.add(text);
      history.push(text);
      if (history.length >= maxComposerHistory) {
        break;
      }
    }
    return history;
  }

  function persistComposerState() {
    webviewState = {
      ...webviewState,
      composerDraft: input.value,
      composerHistory,
    };
    vscode.setState(webviewState);
  }

  function recordComposerHistory(text) {
    const trimmed = String(text || "").trim();
    if (!trimmed) {
      return;
    }
    composerHistory = [
      trimmed,
      ...composerHistory.filter((item) => item !== trimmed),
    ].slice(0, maxComposerHistory);
  }

  function resetComposerHistoryNavigation() {
    composerHistoryIndex = null;
    composerDraftBeforeHistory = "";
  }

  function renderConnectionState(state) {
    const connected = Boolean(state.connected);
    const turnInProgress = Boolean(state.turnInProgress);
    connectButton.hidden = connected;
    connectButton.disabled = connected || turnInProgress;
    newSessionButton.disabled = !connected || turnInProgress;
    cancelButton.hidden = !turnInProgress;
    cancelButton.disabled = !turnInProgress;
    statusEl.textContent = state.cancelling
      ? "Cancelling"
      : turnInProgress
        ? "Working"
        : connected
          ? state.currentModelId || "Live"
          : "Offline";
  }

  function renderRuntime(state) {
    const sessionId = state.sessionId || "";
    const models = state.modelOptions || [];

    renderModelSelect(models, state.currentModelId, Boolean(state.connected));
    renderSessionSelect(state.sessionOptions || [], sessionId, Boolean(state.connected));
    renderThinkingSelect(state.thinkingLevel || "");
    renderContextValue(state);
    renderSettings(state.settings || {});
  }

  function renderModelSelect(models, currentModelId, connected) {
    const previous = modelSelect.value;
    modelSelect.innerHTML = "";

    if (!connected) {
      modelSelect.append(new Option("Connect to load models", ""));
      modelSelect.disabled = true;
      return;
    }

    if (!models.length) {
      modelSelect.append(new Option("No model roster", ""));
      modelSelect.disabled = true;
      return;
    }

    for (const model of models) {
      const option = new Option(model.name || model.id, model.id);
      option.title = model.description || model.id;
      modelSelect.append(option);
    }

    modelSelect.disabled = Boolean(latestState.turnInProgress);
    modelSelect.value = currentModelId || previous || models[0].id;
  }

  function renderSessionSelect(sessions, currentSessionId, connected) {
    const previous = sessionSelect.value;
    sessionSelect.innerHTML = "";

    const normalized = Array.isArray(sessions) ? sessions.slice() : [];
    if (currentSessionId && !normalized.some((session) => session.id === currentSessionId)) {
      normalized.unshift({ id: currentSessionId, title: shortSession(currentSessionId) });
    }

    if (!normalized.length) {
      sessionSelect.append(new Option(connected ? "No session" : "Connect to load session", ""));
      sessionSelect.disabled = true;
      return;
    }

    for (const session of normalized) {
      const title = session.title || shortSession(session.id);
      const suffix = session.updatedAt ? ` · ${formatSessionTime(session.updatedAt)}` : "";
      const option = new Option(`${title}${suffix}`, session.id);
      option.title = session.id;
      sessionSelect.append(option);
    }

    sessionSelect.disabled = !connected || Boolean(latestState.turnInProgress);
    sessionSelect.value = currentSessionId || previous || normalized[0].id;
  }

  function renderThinkingSelect(value) {
    const next = value || "";
    if (thinkingSelect.value !== next) {
      thinkingSelect.value = next;
    }
    thinkingSelect.disabled = Boolean(latestState.turnInProgress);
  }

  function renderContextValue(state) {
    const usage = state.usage;
    const summary = state.contextSummary || {};
    if (usage && usage.size > 0) {
      const pct = Math.min(100, Math.round((usage.used / usage.size) * 100));
      contextValue.textContent = `ctx ${pct}%`;
      contextValue.title = `${usage.used.toLocaleString()} / ${usage.size.toLocaleString()} tokens`;
      return;
    }
    if (!summary.enabled) {
      contextValue.textContent = "ctx off";
      contextValue.title = "Editor context is off";
      return;
    }
    contextValue.textContent = "ctx on";
    contextValue.title = [
      summary.activeFile ? `Active: ${summary.activeFile}` : null,
      `${summary.openTabs || 0} tabs`,
      `${summary.diagnostics || 0} diagnostics`,
    ].filter(Boolean).join(" · ");
  }

  function renderSettings(settings) {
    if (document.activeElement !== daemonUrlInput) {
      daemonUrlInput.value = settings.daemonUrl || "";
    }
    if (document.activeElement !== acpPathInput) {
      acpPathInput.value = settings.acpPath || settings.resolvedAcpPath || "";
    }
    permissionSelect.value = settings.autoApprovePermissions || "ask";
    contextToggle.checked = settings.injectEditorContext !== false;
    trafficToggle.checked = Boolean(settings.logTraffic);
  }

  function renderActivity(state) {
    activityEl.hidden = true;
    activityEl.textContent = "";
  }

  function formatStatus(status) {
    switch (status) {
      case "in_progress":
        return "running";
      case "completed":
        return "done";
      default:
        return status || "pending";
    }
  }

  function renderTranscript(items) {
    const previousBottom =
      messagesEl.scrollHeight - messagesEl.scrollTop - messagesEl.clientHeight;
    const stickToBottom = forceTranscriptScroll || previousBottom < 80;
    messagesEl.innerHTML = "";
    if (!items.length) {
      const empty = document.createElement("div");
      empty.className = "empty";
      const detail = document.createElement("span");
      detail.textContent = "No turns yet.";
      empty.append(detail);
      messagesEl.appendChild(empty);
      forceTranscriptScroll = false;
      return;
    }

    for (const item of compactTranscriptItems(items)) {
      if (item.type === "tool") {
        messagesEl.appendChild(renderToolRow(item.tool));
        continue;
      }
      if (item.type === "toolGroup") {
        messagesEl.appendChild(renderToolGroup(item.tools));
        continue;
      }
      messagesEl.appendChild(renderMessageRow(item.message));
    }
    if (stickToBottom) {
      messagesEl.scrollTop = messagesEl.scrollHeight;
    } else {
      messagesEl.scrollTop = Math.max(
        0,
        messagesEl.scrollHeight - messagesEl.clientHeight - previousBottom,
      );
    }
    forceTranscriptScroll = false;
  }

  function compactTranscriptItems(items) {
    const compacted = [];
    let completedTools = [];

    function flushCompletedTools() {
      if (!completedTools.length) {
        return;
      }
      if (completedTools.length === 1) {
        compacted.push({ type: "tool", tool: completedTools[0] });
      } else {
        compacted.push({ type: "toolGroup", tools: completedTools });
      }
      completedTools = [];
    }

    for (const item of items) {
      if (item.type === "tool" && isCompletedTool(item.tool)) {
        completedTools.push(item.tool);
        continue;
      }
      flushCompletedTools();
      compacted.push(item);
    }
    flushCompletedTools();
    return compacted;
  }

  function isCompletedTool(tool) {
    return (tool.status || "pending") === "completed";
  }

  function buildLegacyTranscript(messages, tools) {
    const items = [];
    for (const message of messages || []) {
      items.push({ type: "message", message });
    }
    for (const tool of tools || []) {
      items.push({ type: "tool", tool });
    }
    return items;
  }

  function renderMessageRow(msg) {
    const div = document.createElement("article");
    div.className = `msg ${msg.role}${msg.streaming ? " streaming" : ""}`;

    const label = document.createElement("span");
    label.className = "msg-label";
    label.textContent = roleLabels[msg.role] || msg.role.toUpperCase();

    const body = document.createElement("div");
    body.className = "msg-body";
    body.appendChild(renderMarkdown(msg.content || ""));

    div.append(label, body);
    return div;
  }

  function renderMarkdown(value) {
    const root = document.createElement("div");
    root.className = "markdown";
    const lines = String(value).replace(/\r\n/g, "\n").split("\n");
    let index = 0;

    while (index < lines.length) {
      const line = lines[index];
      if (!line.trim()) {
        index += 1;
        continue;
      }

      const fence = line.match(/^```([A-Za-z0-9_-]*)\s*$/);
      if (fence) {
        const codeLines = [];
        index += 1;
        while (index < lines.length && !lines[index].startsWith("```")) {
          codeLines.push(lines[index]);
          index += 1;
        }
        if (index < lines.length) {
          index += 1;
        }
        const pre = document.createElement("pre");
        const code = document.createElement("code");
        if (fence[1]) {
          code.dataset.language = fence[1];
        }
        code.textContent = codeLines.join("\n");
        pre.appendChild(code);
        root.appendChild(pre);
        continue;
      }

      const heading = line.match(/^(#{1,3})\s+(.+)$/);
      if (heading) {
        const level = Math.min(heading[1].length + 1, 4);
        const element = document.createElement(`h${level}`);
        appendInline(element, heading[2]);
        root.appendChild(element);
        index += 1;
        continue;
      }

      if (isTableStart(lines, index)) {
        const tableBlock = renderTable(lines, index);
        root.appendChild(tableBlock.element);
        index = tableBlock.nextIndex;
        continue;
      }

      if (isListLine(line)) {
        const ordered = isOrderedListLine(line);
        const list = document.createElement(ordered ? "ol" : "ul");
        while (
          index < lines.length &&
          isListLine(lines[index]) &&
          isOrderedListLine(lines[index]) === ordered
        ) {
          const item = document.createElement("li");
          appendInline(item, lines[index].replace(ordered ? /^\d+[.)]\s+/ : /^[-*]\s+/, ""));
          list.appendChild(item);
          index += 1;
        }
        root.appendChild(list);
        continue;
      }

      if (/^>\s?/.test(line)) {
        const blockquote = document.createElement("blockquote");
        const paragraph = document.createElement("p");
        const quoteLines = [];
        while (index < lines.length && /^>\s?/.test(lines[index])) {
          quoteLines.push(lines[index].replace(/^>\s?/, ""));
          index += 1;
        }
        appendInline(paragraph, quoteLines.join("\n"));
        blockquote.appendChild(paragraph);
        root.appendChild(blockquote);
        continue;
      }

      const paragraphLines = [];
      while (
        index < lines.length &&
        lines[index].trim() &&
        !lines[index].startsWith("```") &&
        !/^(#{1,3})\s+/.test(lines[index]) &&
        !isListLine(lines[index]) &&
        !/^>\s?/.test(lines[index])
      ) {
        paragraphLines.push(lines[index]);
        index += 1;
      }
      const paragraph = document.createElement("p");
      appendInline(paragraph, paragraphLines.join(" "));
      root.appendChild(paragraph);
    }

    if (!root.childElementCount) {
      const paragraph = document.createElement("p");
      paragraph.textContent = "";
      root.appendChild(paragraph);
    }
    return root;
  }

  function isListLine(line) {
    return /^[-*]\s+/.test(line) || isOrderedListLine(line);
  }

  function isOrderedListLine(line) {
    return /^\d+[.)]\s+/.test(line);
  }

  function isTableStart(lines, index) {
    return (
      index + 1 < lines.length &&
      isTableRow(lines[index]) &&
      isTableSeparator(lines[index + 1])
    );
  }

  function isTableRow(line) {
    return /^\s*\|?.+\|.+\|?\s*$/.test(line || "");
  }

  function isTableSeparator(line) {
    const cells = splitTableRow(line);
    return (
      cells.length > 1 &&
      cells.every((cell) => /^:?-{3,}:?$/.test(cell.replace(/\s/g, "")))
    );
  }

  function splitTableRow(line) {
    let trimmed = String(line || "").trim();
    if (trimmed.startsWith("|")) {
      trimmed = trimmed.slice(1);
    }
    if (trimmed.endsWith("|")) {
      trimmed = trimmed.slice(0, -1);
    }
    return trimmed.split("|").map((cell) => cell.trim());
  }

  function renderTable(lines, startIndex) {
    const headers = splitTableRow(lines[startIndex]);
    const aligns = splitTableRow(lines[startIndex + 1]).map((cell) => {
      const trimmed = cell.trim();
      if (trimmed.startsWith(":") && trimmed.endsWith(":")) {
        return "center";
      }
      return trimmed.endsWith(":") ? "right" : "";
    });
    const table = document.createElement("table");
    const thead = document.createElement("thead");
    const headerRow = document.createElement("tr");
    for (const [index, header] of headers.entries()) {
      const cell = document.createElement("th");
      if (aligns[index]) {
        cell.style.textAlign = aligns[index];
      }
      appendInline(cell, header);
      headerRow.appendChild(cell);
    }
    thead.appendChild(headerRow);
    table.appendChild(thead);

    const tbody = document.createElement("tbody");
    let index = startIndex + 2;
    while (index < lines.length && lines[index].trim() && isTableRow(lines[index])) {
      const row = document.createElement("tr");
      const cells = splitTableRow(lines[index]);
      for (let cellIndex = 0; cellIndex < headers.length; cellIndex += 1) {
        const cell = document.createElement("td");
        if (aligns[cellIndex]) {
          cell.style.textAlign = aligns[cellIndex];
        }
        appendInline(cell, cells[cellIndex] || "");
        row.appendChild(cell);
      }
      tbody.appendChild(row);
      index += 1;
    }
    table.appendChild(tbody);

    const wrap = document.createElement("div");
    wrap.className = "table-wrap";
    wrap.appendChild(table);
    return { element: wrap, nextIndex: index };
  }

  function appendInline(parent, value) {
    const pattern = /(`[^`\n]+`|\*\*[^*\n]+?\*\*)/g;
    let lastIndex = 0;
    const text = String(value);
    for (const match of text.matchAll(pattern)) {
      appendText(parent, text.slice(lastIndex, match.index));
      const token = match[0];
      if (token.startsWith("`")) {
        appendInlineCode(parent, token.slice(1, -1));
      } else {
        const strong = document.createElement("strong");
        strong.textContent = token.slice(2, -2);
        parent.appendChild(strong);
      }
      lastIndex = match.index + token.length;
    }
    appendText(parent, text.slice(lastIndex));
  }

  function appendInlineCode(parent, value) {
    const fileRef = parseExactFileReference(value);
    if (fileRef) {
      parent.appendChild(renderFileReferenceLink(value, fileRef, true));
      return;
    }

    const code = document.createElement("code");
    code.textContent = value;
    parent.appendChild(code);
  }

  function appendText(parent, value) {
    const parts = String(value).split("\n");
    for (const [index, part] of parts.entries()) {
      if (index > 0) {
        parent.appendChild(document.createElement("br"));
      }
      if (part) {
        appendLinkedText(parent, part);
      }
    }
  }

  function appendLinkedText(parent, value) {
    const pattern = /(^|[\s([{])((?:~\/|\.{1,2}\/|\/|[A-Za-z0-9_.-]+\/)[^\s`'"<>{}\[\](),;:]+|[A-Za-z0-9_.-]+\.[A-Za-z][A-Za-z0-9_-]{0,7})(?:(?::|#L?)(\d{1,7})(?::\d{1,5})?)?/gi;
    let lastIndex = 0;
    const text = String(value);

    for (const match of text.matchAll(pattern)) {
      const prefix = match[1] || "";
      const refStart = (match.index || 0) + prefix.length;
      const matchEnd = (match.index || 0) + match[0].length;
      const rawPath = match[2] || "";
      const line = match[3] ? Number(match[3]) : null;
      const fileRef = parseFileReferenceParts(rawPath, line);
      if (!fileRef) {
        continue;
      }

      if (refStart > lastIndex) {
        parent.appendChild(document.createTextNode(text.slice(lastIndex, refStart)));
      }
      parent.appendChild(
        renderFileReferenceLink(
          text.slice(refStart, matchEnd),
          fileRef,
          false,
        ),
      );
      lastIndex = matchEnd;
    }

    if (lastIndex < text.length) {
      parent.appendChild(document.createTextNode(text.slice(lastIndex)));
    }
  }

  function parseExactFileReference(value) {
    const text = String(value || "").trim();
    const match = text.match(/^((?:~\/|\.{1,2}\/|\/|[A-Za-z0-9_.-]+\/)[^\s`'"<>{}\[\](),;:]+|[A-Za-z0-9_.-]+\.[A-Za-z][A-Za-z0-9_-]{0,7})(?:(?::|#L?)(\d{1,7})(?::\d{1,5})?)?$/i);
    if (!match) {
      return null;
    }
    return parseFileReferenceParts(match[1], match[2] ? Number(match[2]) : null);
  }

  function parseFileReferenceParts(rawPath, line) {
    const pathValue = normalizeFileReferencePath(rawPath);
    if (!pathValue || !isLikelyTranscriptFilePath(pathValue)) {
      return null;
    }
    return {
      path: pathValue,
      line: Number.isFinite(line) && line > 0 ? Math.floor(line) : null,
    };
  }

  function normalizeFileReferencePath(value) {
    return String(value || "")
      .trim()
      .replace(/[.!?]+$/g, "");
  }

  function isLikelyTranscriptFilePath(value) {
    if (/^[a-z][a-z0-9+.-]*:\/\//i.test(value)) {
      return false;
    }
    if (/^(?:~\/|\.{1,2}\/|\/)/.test(value)) {
      return true;
    }
    if (value.includes("/")) {
      return true;
    }
    if (/^\d+(?:\.\d+)+$/.test(value)) {
      return false;
    }
    return /\.(?:[cm]?[jt]sx?|jsonc?|mdx?|ya?ml|toml|rs|py|go|java|kt|swift|c|cc|cpp|h|hpp|css|scss|html?|svelte|vue|sh|bash|zsh|fish|sql|graphql|proto|txt|lock|env|gitignore|dockerfile)$/i.test(value);
  }

  function renderFileReferenceLink(label, fileRef, codeStyle) {
    const link = document.createElement("a");
    link.className = `file-ref${codeStyle ? " code-ref" : ""}`;
    link.href = "#";
    link.title = fileRef.line ? `${fileRef.path}:${fileRef.line}` : fileRef.path;
    link.addEventListener("click", (event) => {
      event.preventDefault();
      vscode.postMessage({
        type: "openFile",
        path: fileRef.path,
        line: fileRef.line || undefined,
      });
    });

    if (codeStyle) {
      const code = document.createElement("code");
      code.textContent = label;
      link.appendChild(code);
    } else {
      link.textContent = label;
    }
    return link;
  }

  function renderToolRow(tool, options = {}) {
    const row = document.createElement("article");
    row.className = `tool-row ${tool.status || "pending"}`;

    const status = document.createElement("span");
    status.className = "tool-state";
    status.textContent = formatStatus(tool.status);

    const body = document.createElement("div");
    body.className = "tool-body";

    const title = document.createElement("div");
    title.className = "tool-title";
    title.textContent = tool.title || tool.id || "Tool call";

    const meta = document.createElement("div");
    meta.className = "tool-meta";
    meta.textContent = [options.summary, tool.kind || "tool"].filter(Boolean).join(" · ");

    body.append(title, meta);

    const files = collectToolFiles(tool);
    if (files.length) {
      body.appendChild(renderToolFiles(files));
    }

    const output = collectToolOutput(tool);
    if (output.items.length) {
      body.appendChild(renderToolOutput(output));
    }

    row.append(status, body);
    return row;
  }

  function renderToolGroup(tools) {
    const details = document.createElement("details");
    details.className = "tool-details compact";

    const summary = document.createElement("summary");
    summary.className = "tool-summary";

    const status = document.createElement("span");
    status.className = "tool-state";
    status.textContent = "done";

    const body = document.createElement("span");
    body.className = "tool-summary-body";

    const title = document.createElement("span");
    title.className = "tool-title";
    title.textContent = `${tools.length} tool calls`;

    const meta = document.createElement("span");
    meta.className = "tool-meta";
    meta.textContent = summarizeToolKinds(tools);

    body.append(title, meta);
    summary.append(status, body);

    const list = document.createElement("div");
    list.className = "tool-list";
    for (const tool of tools) {
      list.appendChild(renderToolRow(tool));
    }

    details.append(summary, list);
    return details;
  }

  function summarizeToolKinds(tools) {
    const counts = new Map();
    for (const tool of tools) {
      const label = tool.kind || tool.title || "tool";
      counts.set(label, (counts.get(label) || 0) + 1);
    }
    return [...counts.entries()]
      .slice(0, 4)
      .map(([label, count]) => count > 1 ? `${label} x${count}` : label)
      .join(" · ");
  }

  function renderToolFiles(files) {
    const fileList = document.createElement("div");
    fileList.className = "tool-files";
    for (const file of files.slice(0, 4)) {
      const fileRow = document.createElement("div");
      fileRow.className = `tool-file ${file.diff ? "changed" : ""}`;
      fileRow.tabIndex = 0;
      fileRow.setAttribute("role", "button");
      fileRow.dataset.path = file.path;
      fileRow.title = file.line ? `${file.path}:${file.line}` : file.path;
      fileRow.addEventListener("click", () => {
        openToolFile(file);
      });
      fileRow.addEventListener("keydown", (event) => {
        if (event.key !== "Enter" && event.key !== " ") {
          return;
        }
        event.preventDefault();
        openToolFile(file);
      });

      const name = document.createElement("span");
      name.className = "tool-file-name";
      name.textContent = compactPath(file.path);

      const tag = document.createElement("span");
      tag.className = "tool-file-tag";
      tag.textContent = file.diff ? file.diffLabel : file.line ? `L${file.line}` : "file";

      fileRow.append(name, tag);
      fileList.appendChild(fileRow);
    }
    if (files.length > 4) {
      const more = document.createElement("div");
      more.className = "tool-file more";
      more.textContent = `${files.length - 4} more`;
      fileList.appendChild(more);
    }
    return fileList;
  }

  function openToolFile(file) {
    if (file.diff && file.toolId) {
      vscode.postMessage({
        type: "openDiff",
        toolId: file.toolId,
        path: file.path,
      });
      return;
    }
    vscode.postMessage({
      type: "openFile",
      path: file.path,
      line: file.line || undefined,
    });
  }

  function collectToolFiles(tool) {
    const seen = new Map();
    for (const location of tool.locations || []) {
      if (!location || !location.path) {
        continue;
      }
      seen.set(location.path, {
        path: location.path,
        toolId: tool.id || null,
        line: location.line || null,
        diff: false,
        diffLabel: "file",
      });
    }
    for (const content of tool.content || []) {
      if (!content || content.type !== "diff" || !content.path) {
        continue;
      }
      seen.set(content.path, {
        path: content.path,
        toolId: tool.id || null,
        line: null,
        diff: true,
        diffLabel: content.oldText == null ? "created" : "edited",
      });
    }
    return [...seen.values()];
  }

  function collectToolOutput(tool) {
    const items = [];
    for (const content of tool.content || []) {
      const item = toolOutputItem(content, tool);
      if (item) {
        items.push(item);
      }
    }
    return {
      items: items.slice(0, 2),
      hidden: Math.max(0, items.length - 2),
    };
  }

  function toolOutputItem(content, tool) {
    if (!content || content.type === "diff") {
      return null;
    }

    if (content.type === "terminal") {
      const terminalId = content.terminalId || "";
      const preview = terminalId ? tool.terminalOutputs?.[terminalId] : null;
      const text = compactOutputText(preview?.output || "");
      if (!text) {
        return null;
      }
      const suffix = terminalStatus(preview);
      return { label: "term", text, suffix };
    }

    if (content.type !== "content" || !content.content) {
      return null;
    }

    const block = content.content;
    if (block.type === "text") {
      const text = compactOutputText(block.text || "");
      return text ? { label: "text", text } : null;
    }

    if (block.type === "resource_link") {
      const title = block.title || block.name || block.uri || "resource";
      const lines = uniqueTextLines([title, block.uri, block.mimeType]);
      const text = compactOutputText(lines.join("\n"));
      return text ? { label: "res", text } : null;
    }

    if (block.type === "resource" && block.resource) {
      const resource = block.resource;
      const text = compactOutputText(
        resource.text || uniqueTextLines([resource.uri, resource.mimeType]).join("\n"),
      );
      return text ? { label: "res", text } : null;
    }

    return null;
  }

  function renderToolOutput(output) {
    const wrap = document.createElement("div");
    wrap.className = "tool-output";
    for (const item of output.items) {
      const row = document.createElement("div");
      row.className = "tool-output-item";

      const label = document.createElement("span");
      label.className = "tool-output-label";
      label.textContent = item.suffix ? `${item.label} ${item.suffix}` : item.label;

      const text = document.createElement("pre");
      text.className = "tool-output-text";
      appendText(text, item.text);

      row.append(label, text);
      wrap.appendChild(row);
    }
    if (output.hidden) {
      const more = document.createElement("div");
      more.className = "tool-output-more";
      more.textContent = `${output.hidden} more outputs`;
      wrap.appendChild(more);
    }
    return wrap;
  }

  function compactOutputText(value) {
    const normalized = String(value || "")
      .replace(/\r\n/g, "\n")
      .replace(/\t/g, "  ")
      .trim();
    if (!normalized) {
      return "";
    }

    const lines = normalized.split("\n");
    let truncated = lines.length > 8;
    let text = lines.slice(0, 8).join("\n");
    if (text.length > 640) {
      text = text.slice(0, 640).replace(/\s+$/g, "");
      truncated = true;
    }
    return truncated ? `${text}\n...` : text;
  }

  function uniqueTextLines(values) {
    const seen = new Set();
    const lines = [];
    for (const value of values) {
      const text = String(value || "").trim();
      if (!text || seen.has(text)) {
        continue;
      }
      seen.add(text);
      lines.push(text);
    }
    return lines;
  }

  function terminalStatus(preview) {
    if (!preview) {
      return "";
    }
    if (preview.signal) {
      return preview.truncated ? "sig" : `sig ${preview.signal}`;
    }
    if (preview.exitCode !== undefined && preview.exitCode !== null) {
      return preview.truncated ? `x${preview.exitCode}+` : `x${preview.exitCode}`;
    }
    return preview.truncated ? "+" : "";
  }

  function compactPath(path) {
    const normalized = String(path).replace(/\\/g, "/");
    const parts = normalized.split("/").filter(Boolean);
    if (parts.length <= 3) {
      return normalized;
    }
    return `.../${parts.slice(-3).join("/")}`;
  }

  function shortSession(sessionId) {
    return sessionId ? sessionId.slice(0, 8) : "no session";
  }

  function formatSessionTime(value) {
    const date = new Date(value);
    if (Number.isNaN(date.valueOf())) {
      return "";
    }
    return date.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  }

  function pushSetting(type, value) {
    vscode.postMessage({ type, value });
  }

  function updateMentionMenu(options = {}) {
    if (document.activeElement !== input) {
      closeMentionMenu();
      return;
    }
    const trigger = activeMentionTrigger();
    if (!trigger) {
      closeMentionMenu();
      return;
    }
    closeSlashMenu(false);

    const query = trigger.query.toLowerCase();
    if (options.requestFiles !== false) {
      queueMentionFileSearch(query);
    }

    const staticOptions = mentionOptions
      .filter((option) => mentionMatches(option, query))
      .slice(0, 7);
    const fileOptions = mentionFileQuery === query ? mentionFileOptions : [];
    const resolvedOptions = [...staticOptions, ...fileOptions].slice(0, 9);
    if (!resolvedOptions.length) {
      closeMentionMenu(false);
      return;
    }

    mentionState = {
      open: true,
      start: trigger.start,
      active: Math.min(mentionState.active, resolvedOptions.length - 1),
      options: resolvedOptions,
    };
    renderMentionMenu();
  }

  function updateSlashMenu() {
    if (document.activeElement !== input) {
      closeSlashMenu();
      return false;
    }
    const trigger = activeSlashTrigger();
    if (!trigger) {
      closeSlashMenu(false);
      return false;
    }

    closeMentionMenu(false);
    const query = trigger.query.toLowerCase();
    const options = slashOptions
      .filter((option) => slashMatches(option, query))
      .slice(0, 8);
    if (!options.length) {
      closeSlashMenu();
      return true;
    }

    slashState = {
      open: true,
      start: trigger.start,
      active: Math.min(slashState.active, options.length - 1),
      options,
    };
    renderSlashMenu();
    return true;
  }

  function activeMentionTrigger() {
    if (input.selectionStart !== input.selectionEnd) {
      return null;
    }
    const cursor = input.selectionStart ?? input.value.length;
    const prefix = input.value.slice(0, cursor);
    const match = prefix.match(/(^|\s)@([^\s@]*)$/);
    if (!match) {
      return null;
    }
    return {
      start: prefix.lastIndexOf("@"),
      query: match[2] || "",
    };
  }

  function activeSlashTrigger() {
    if (input.selectionStart !== input.selectionEnd) {
      return null;
    }
    const cursor = input.selectionStart ?? input.value.length;
    const prefix = input.value.slice(0, cursor);
    const match = prefix.match(/(^|\n)\s*\/([A-Za-z-]*)$/);
    if (!match) {
      return null;
    }
    return {
      start: prefix.lastIndexOf("/"),
      query: match[2] || "",
    };
  }

  function queueMentionFileSearch(query) {
    if (!shouldSearchFilesForMention(query)) {
      mentionFileQuery = "";
      mentionFileOptions = [];
      return;
    }
    if (mentionFileQuery === query) {
      return;
    }

    window.clearTimeout(mentionSearchTimer);
    const requestId = ++mentionRequestSeq;
    mentionSearchTimer = window.setTimeout(() => {
      vscode.postMessage({
        type: "mentionSearch",
        requestId,
        query,
      });
    }, 120);
  }

  function shouldSearchFilesForMention(query) {
    return query.length >= 2 || /[./_-]/.test(query);
  }

  function handleMentionSuggestions(message) {
    if (Number(message.requestId) !== mentionRequestSeq) {
      return;
    }
    if (document.activeElement !== input) {
      return;
    }
    const query = String(message.query || "").toLowerCase();
    const trigger = activeMentionTrigger();
    if (!trigger || trigger.query.toLowerCase() !== query) {
      return;
    }

    mentionFileQuery = query;
    mentionFileOptions = Array.isArray(message.items)
      ? message.items.map((item) => ({
        kind: "file",
        label: String(item.label || ""),
        detail: String(item.detail || ""),
        insert: String(item.insert || ""),
      })).filter((item) => item.label && item.insert)
      : [];
    updateMentionMenu({ requestFiles: false });
  }

  function mentionMatches(option, query) {
    if (!query) {
      return true;
    }
    const haystack = [
      option.token || option.label,
      option.label,
      option.detail,
      ...(option.aliases || []),
    ]
      .join(" ")
      .toLowerCase();
    return haystack.includes(query);
  }

  function slashMatches(option, query) {
    if (!query) {
      return true;
    }
    const haystack = [
      option.token,
      option.label,
      option.detail,
      ...(option.aliases || []),
    ]
      .join(" ")
      .toLowerCase();
    return haystack.includes(query);
  }

  function renderMentionMenu() {
    mentionMenu.innerHTML = "";
    for (const [index, option] of mentionState.options.entries()) {
      const item = document.createElement("div");
      item.className =
        `mention-item ${option.kind || "context"}${index === mentionState.active ? " active" : ""}`;
      item.setAttribute("role", "option");
      item.setAttribute("aria-selected", index === mentionState.active ? "true" : "false");
      item.dataset.index = String(index);

      const token = document.createElement("span");
      token.className = "mention-token";
      token.textContent = option.token || option.label;

      const detail = document.createElement("span");
      detail.className = "mention-detail";
      detail.textContent = option.detail || "";

      item.append(token, detail);
      item.addEventListener("pointerdown", (event) => {
        event.preventDefault();
        insertMention(option);
      });
      mentionMenu.appendChild(item);
    }
    mentionMenu.hidden = false;
  }

  function renderSlashMenu() {
    mentionMenu.innerHTML = "";
    for (const [index, option] of slashState.options.entries()) {
      const item = document.createElement("div");
      item.className = `mention-item command${index === slashState.active ? " active" : ""}`;
      item.setAttribute("role", "option");
      item.setAttribute("aria-selected", index === slashState.active ? "true" : "false");
      item.dataset.index = String(index);

      const token = document.createElement("span");
      token.className = "mention-token";
      token.textContent = option.label;

      const detail = document.createElement("span");
      detail.className = "mention-detail";
      detail.textContent = option.detail || "";

      item.append(token, detail);
      item.addEventListener("pointerdown", (event) => {
        event.preventDefault();
        insertSlashCommand(option);
      });
      mentionMenu.appendChild(item);
    }
    mentionMenu.hidden = false;
  }

  function closeMentionMenu(clearPending = true) {
    if (clearPending) {
      window.clearTimeout(mentionSearchTimer);
    }
    mentionState = {
      open: false,
      start: -1,
      active: 0,
      options: [],
    };
    mentionMenu.hidden = true;
    mentionMenu.innerHTML = "";
  }

  function closeSlashMenu(clearDom = true) {
    slashState = {
      open: false,
      start: -1,
      active: 0,
      options: [],
    };
    if (clearDom && !mentionState.open) {
      mentionMenu.hidden = true;
      mentionMenu.innerHTML = "";
    }
  }

  function moveMention(delta) {
    if (!mentionState.open || !mentionState.options.length) {
      return;
    }
    mentionState.active =
      (mentionState.active + delta + mentionState.options.length) %
      mentionState.options.length;
    renderMentionMenu();
  }

  function moveSlash(delta) {
    if (!slashState.open || !slashState.options.length) {
      return;
    }
    slashState.active =
      (slashState.active + delta + slashState.options.length) %
      slashState.options.length;
    renderSlashMenu();
  }

  function insertActiveMention() {
    const option = mentionState.options[mentionState.active];
    if (option) {
      insertMention(option);
    }
  }

  function insertActiveSlashCommand() {
    const option = slashState.options[slashState.active];
    if (option) {
      insertSlashCommand(option);
    }
  }

  function insertMention(option) {
    const cursor = input.selectionStart ?? input.value.length;
    const before = input.value.slice(0, mentionState.start);
    const after = input.value.slice(cursor);
    const token = option.insert || option.token;
    const insert = `${token} `;
    input.value = before + insert + after;
    const nextCursor = before.length + insert.length;
    input.selectionStart = nextCursor;
    input.selectionEnd = nextCursor;
    closeMentionMenu();
    resizeComposer();
    persistComposerState();
    input.focus();
  }

  function insertSlashCommand(option) {
    const cursor = input.selectionStart ?? input.value.length;
    const before = input.value.slice(0, slashState.start);
    const after = input.value.slice(cursor);
    const suffix = after && !/^\s/.test(after) ? " " : "";
    const insert = `${option.insert}${suffix}`;
    input.value = before + insert + after;
    const nextCursor = before.length + insert.length;
    input.selectionStart = nextCursor;
    input.selectionEnd = nextCursor;
    closeSlashMenu();
    resizeComposer();
    persistComposerState();
    input.focus();
  }

  function resizeComposer() {
    input.style.height = "auto";
    const style = window.getComputedStyle(input);
    const maxHeight = Number.parseFloat(style.maxHeight) || 160;
    const nextHeight = Math.min(input.scrollHeight, maxHeight);
    input.style.height = `${nextHeight}px`;
    input.style.overflowY = input.scrollHeight > maxHeight ? "auto" : "hidden";
  }

  function sendComposer() {
    const text = input.value.trim();
    if (!text) {
      return;
    }
    forceTranscriptScroll = true;
    closeSlashMenu();
    closeMentionMenu();
    recordComposerHistory(text);
    resetComposerHistoryNavigation();
    vscode.postMessage({ type: "send", text });
    input.value = "";
    resizeComposer();
    persistComposerState();
  }

  function handleComposerHistoryKey(event) {
    if (
      event.defaultPrevented ||
      event.shiftKey ||
      event.altKey ||
      event.metaKey ||
      event.ctrlKey
    ) {
      return false;
    }

    if (event.key === "ArrowUp") {
      return recallPreviousPrompt();
    }
    if (event.key === "ArrowDown") {
      return recallNextPrompt();
    }
    if (event.key === "Escape" && composerHistoryIndex !== null) {
      setComposerText(composerDraftBeforeHistory);
      resetComposerHistoryNavigation();
      return true;
    }
    return false;
  }

  function recallPreviousPrompt() {
    if (!composerHistory.length || !caretIsOnFirstLine()) {
      return false;
    }
    if (composerHistoryIndex === null) {
      composerDraftBeforeHistory = input.value;
      composerHistoryIndex = 0;
    } else if (composerHistoryIndex < composerHistory.length - 1) {
      composerHistoryIndex += 1;
    } else {
      return true;
    }
    setComposerText(composerHistory[composerHistoryIndex] || "");
    return true;
  }

  function recallNextPrompt() {
    if (composerHistoryIndex === null || !caretIsOnLastLine()) {
      return false;
    }
    if (composerHistoryIndex > 0) {
      composerHistoryIndex -= 1;
      setComposerText(composerHistory[composerHistoryIndex] || "");
      return true;
    }
    setComposerText(composerDraftBeforeHistory);
    resetComposerHistoryNavigation();
    return true;
  }

  function setComposerText(value) {
    input.value = value;
    const cursor = input.value.length;
    input.selectionStart = cursor;
    input.selectionEnd = cursor;
    closeSlashMenu();
    closeMentionMenu();
    resizeComposer();
    persistComposerState();
  }

  function caretIsOnFirstLine() {
    const cursor = input.selectionStart ?? 0;
    return !input.value.slice(0, cursor).includes("\n");
  }

  function caretIsOnLastLine() {
    const cursor = input.selectionStart ?? input.value.length;
    return !input.value.slice(cursor).includes("\n");
  }

  window.addEventListener("message", (event) => {
    const message = event.data;
    if (message.type === "mentionSuggestions") {
      handleMentionSuggestions(message);
      return;
    }
    if (message.type !== "state") {
      return;
    }
    latestState = message;
    renderTranscript(
      message.transcript ||
        buildLegacyTranscript(message.messages || [], message.tools || []),
    );
    renderConnectionState(message);
    renderRuntime(message);
    renderActivity(message);
  });

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    sendComposer();
  });

  input.addEventListener("keydown", (event) => {
    if (slashState.open) {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        moveSlash(1);
        return;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        moveSlash(-1);
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        closeSlashMenu();
        return;
      }
      if ((event.key === "Enter" && !event.shiftKey) || event.key === "Tab") {
        event.preventDefault();
        insertActiveSlashCommand();
        return;
      }
    }
    if (mentionState.open) {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        moveMention(1);
        return;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        moveMention(-1);
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        closeMentionMenu();
        return;
      }
      if ((event.key === "Enter" && !event.shiftKey) || event.key === "Tab") {
        event.preventDefault();
        insertActiveMention();
        return;
      }
    }
    if (handleComposerHistoryKey(event)) {
      event.preventDefault();
      return;
    }
    if (event.key !== "Enter" || event.shiftKey) {
      return;
    }
    event.preventDefault();
    sendComposer();
  });

  input.addEventListener("input", () => {
    resetComposerHistoryNavigation();
    resizeComposer();
    if (!updateSlashMenu()) {
      updateMentionMenu();
    }
    persistComposerState();
  });

  input.addEventListener("click", () => {
    if (!updateSlashMenu()) {
      updateMentionMenu();
    }
  });

  input.addEventListener("keyup", (event) => {
    if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) {
      if (!updateSlashMenu()) {
        updateMentionMenu();
      }
    }
  });

  input.addEventListener("blur", () => {
    window.setTimeout(() => {
      closeSlashMenu();
      closeMentionMenu();
    }, 120);
  });

  modelSelect.addEventListener("change", () => {
    vscode.postMessage({ type: "setModel", value: modelSelect.value });
  });

  sessionSelect.addEventListener("change", () => {
    vscode.postMessage({ type: "loadSession", sessionId: sessionSelect.value });
  });

  thinkingSelect.addEventListener("change", () => {
    pushSetting("setThinkingLevel", thinkingSelect.value);
  });

  permissionSelect.addEventListener("change", () => {
    pushSetting("setPermissionMode", permissionSelect.value);
  });

  contextToggle.addEventListener("change", () => {
    pushSetting("setEditorContext", contextToggle.checked);
  });

  trafficToggle.addEventListener("change", () => {
    pushSetting("setTrafficLog", trafficToggle.checked);
  });

  daemonUrlInput.addEventListener("change", () => {
    pushSetting("setDaemonUrl", daemonUrlInput.value.trim());
  });

  acpPathInput.addEventListener("change", () => {
    pushSetting("setAcpPath", acpPathInput.value.trim());
  });

  connectButton.addEventListener("click", () => {
    vscode.postMessage({ type: "connect" });
  });

  newSessionButton.addEventListener("click", () => {
    vscode.postMessage({ type: "newSession" });
  });

  cancelButton.addEventListener("click", () => {
    vscode.postMessage({ type: "cancel" });
  });

  renderConnectionState({ connected: false, turnInProgress: false });
  renderRuntime({ connected: false, settings: {} });
  resizeComposer();
  vscode.postMessage({ type: "ready" });
})();
