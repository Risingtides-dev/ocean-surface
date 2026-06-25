(function () {
  const vscode = acquireVsCodeApi();
  const activityEl = document.getElementById("activity");
  const messagesEl = document.getElementById("messages");
  const form = document.getElementById("composer");
  const input = document.getElementById("input");
  const connectButton = document.getElementById("connect");
  const newSessionButton = document.getElementById("newSession");
  const inlineAssistButton = document.getElementById("inlineAssist");
  const cancelButton = document.getElementById("cancel");
  const statusEl = document.getElementById("status");

  const roleLabels = {
    user: "YOU",
    assistant: "OCEAN",
    system: "SYS",
    tool: "TOOL",
  };

  function renderConnectionState(state) {
    const connected = Boolean(state.connected);
    const turnInProgress = Boolean(state.turnInProgress);
    connectButton.textContent = connected ? "Live" : "Link";
    connectButton.disabled = connected || turnInProgress;
    newSessionButton.disabled = !connected || turnInProgress;
    inlineAssistButton.disabled = turnInProgress;
    cancelButton.disabled = !turnInProgress;
    statusEl.textContent = state.cancelling
      ? "Cancelling"
      : turnInProgress
        ? "Working"
        : connected
          ? state.sessionId
            ? state.sessionId.slice(0, 8)
            : "Live"
          : "Offline";
  }

  function renderActivity(state) {
    const tools = state.tools ?? [];
    if (!state.turnInProgress || tools.length === 0) {
      activityEl.hidden = true;
      activityEl.innerHTML = "";
      return;
    }

    const latest = tools[tools.length - 1];
    const running = tools.filter(
      (tool) => tool.status === "pending" || tool.status === "in_progress",
    ).length;
    const failed = tools.filter((tool) => tool.status === "failed").length;
    const completed = tools.filter((tool) => tool.status === "completed").length;

    activityEl.hidden = false;
    activityEl.innerHTML = "";

    const label = document.createElement("span");
    label.className = "activity-label";
    label.textContent = state.cancelling ? "CANCELLING" : "TOOLS";

    const body = document.createElement("span");
    body.className = "activity-body";
    body.textContent = `${latest.title} · ${formatStatus(latest.status)}`;

    const count = document.createElement("span");
    count.className = "activity-count";
    count.textContent = `${tools.length} total`;
    if (running > 0) {
      count.textContent += ` · ${running} running`;
    }
    if (completed > 0) {
      count.textContent += ` · ${completed} done`;
    }
    if (failed > 0) {
      count.textContent += ` · ${failed} failed`;
    }

    activityEl.append(label, body, count);
  }

  function formatStatus(status) {
    switch (status) {
      case "in_progress":
        return "running";
      case "completed":
        return "done";
      default:
        return status ?? "pending";
    }
  }

  function renderMessages(messages) {
    messagesEl.innerHTML = "";
    if (!messages.length) {
      const empty = document.createElement("div");
      empty.className = "empty";
      const monogram = document.createElement("div");
      monogram.className = "empty-mark";
      monogram.textContent = "O";
      const title = document.createElement("strong");
      title.textContent = "OCEAN";
      const detail = document.createElement("span");
      detail.textContent = "LOCAL ACP";
      empty.append(monogram, title, detail);
      messagesEl.appendChild(empty);
      return;
    }

    for (const msg of messages) {
      const div = document.createElement("article");
      div.className = `msg ${msg.role}${msg.streaming ? " streaming" : ""}`;

      const label = document.createElement("span");
      label.className = "msg-label";
      label.textContent = roleLabels[msg.role] ?? msg.role.toUpperCase();

      const body = document.createElement("div");
      body.className = "msg-body";
      body.textContent = msg.content;

      div.append(label, body);
      messagesEl.appendChild(div);
    }
    messagesEl.scrollTop = messagesEl.scrollHeight;
  }

  function sendComposer() {
    const text = input.value.trim();
    if (!text) {
      return;
    }
    vscode.postMessage({ type: "send", text });
    input.value = "";
  }

  window.addEventListener("message", (event) => {
    const message = event.data;
    if (message.type === "state") {
      renderMessages(message.messages ?? []);
      renderConnectionState(message);
      renderActivity(message);
    }
  });

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    sendComposer();
  });

  input.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" || event.shiftKey) {
      return;
    }
    if (!event.metaKey && !event.ctrlKey && !event.altKey) {
      event.preventDefault();
      sendComposer();
      return;
    }
    if (event.metaKey || event.ctrlKey) {
      event.preventDefault();
      sendComposer();
    }
  });

  connectButton.addEventListener("click", () => {
    vscode.postMessage({ type: "connect" });
  });

  newSessionButton.addEventListener("click", () => {
    vscode.postMessage({ type: "newSession" });
  });

  inlineAssistButton.addEventListener("click", () => {
    vscode.postMessage({ type: "inlineAssist" });
  });

  cancelButton.addEventListener("click", () => {
    vscode.postMessage({ type: "cancel" });
  });

  renderConnectionState({ connected: false, turnInProgress: false });
  vscode.postMessage({ type: "ready" });
})();
