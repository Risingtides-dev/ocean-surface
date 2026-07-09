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
    user: "you ▸",
    assistant: "ocean ▸",
    system: "sys ▸",
    tool: "tool ▸",
  };

  const WAVE_SVG =
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" ' +
    'stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
    '<path d="M2 14c0-4 3-7 7-7 1.2 0 2.3.3 3.3.8"/>' +
    '<path d="M22 10c0 4-3 7-7 7-1.2 0-2.3-.3-3.3-.8"/>' +
    '<path d="M5 17l-1 4 4-1"/><path d="M19 7l1-4-4 1"/></svg>';

  // Private-use-area sentinel wrapping extracted code blocks. It cannot appear
  // in agent prose, is not whitespace (so trim() leaves it intact), and is not
  // touched by escapeHtml — giving unambiguous detection and restoration.
  const SENT = "";
  const SENT_LINE = new RegExp("^" + SENT + "(\\d+)" + SENT + "$");

  function escapeHtml(text) {
    return text
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;");
  }

  // Inline spans on already-escaped text: code, bold, italic, links.
  function inline(text) {
    return text
      .replace(/`([^`]+)`/g, "<code>$1</code>")
      .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
      .replace(/(^|[^*])\*([^*\s][^*]*?)\*/g, "$1<em>$2</em>")
      .replace(/\[([^\]]+)\]\((https?:[^)\s]+)\)/g, '<a href="$2">$1</a>');
  }

  // Small, self-contained, CSP-safe Markdown renderer. Fenced code blocks are
  // pulled out (their contents escaped verbatim) and replaced with a sentinel
  // line; the remaining source is escaped, then a controlled subset of block/
  // inline formatting is layered back on. No raw agent HTML ever reaches the
  // DOM. Mirrors the web app's .block--text output.
  function renderMarkdown(src) {
    const codeBlocks = [];
    let text = src.replace(/```([\w-]*)\n?([\s\S]*?)```/g, (_m, _lang, code) => {
      codeBlocks.push(
        "<pre><code>" + escapeHtml(code.replace(/\n$/, "")) + "</code></pre>",
      );
      return "\n" + SENT + (codeBlocks.length - 1) + SENT + "\n";
    });

    text = escapeHtml(text);

    const lines = text.split("\n");
    const html = [];
    let listType = null;
    let inQuote = false;

    const closeList = () => {
      if (listType) {
        html.push("</" + listType + ">");
        listType = null;
      }
    };
    const closeQuote = () => {
      if (inQuote) {
        html.push("</blockquote>");
        inQuote = false;
      }
    };

    for (const line of lines) {
      const cbMatch = line.trim().match(SENT_LINE);
      if (cbMatch) {
        closeList();
        closeQuote();
        html.push(codeBlocks[Number(cbMatch[1])]);
        continue;
      }
      if (line.trim() === "") {
        closeList();
        closeQuote();
        continue;
      }

      const heading = line.match(/^(#{1,3})\s+(.*)$/);
      if (heading) {
        closeList();
        closeQuote();
        const level = heading[1].length;
        html.push("<h" + level + ">" + inline(heading[2]) + "</h" + level + ">");
        continue;
      }

      const ordered = line.match(/^\s*\d+\.\s+(.*)$/);
      const unordered = line.match(/^\s*[-*]\s+(.*)$/);
      if (ordered || unordered) {
        const type = ordered ? "ol" : "ul";
        if (listType !== type) {
          closeList();
          html.push("<" + type + ">");
          listType = type;
        }
        html.push("<li>" + inline((ordered || unordered)[1]) + "</li>");
        continue;
      }
      closeList();

      const quote = line.match(/^\s*>\s?(.*)$/);
      if (quote) {
        if (!inQuote) {
          html.push("<blockquote>");
          inQuote = true;
        }
        html.push(inline(quote[1]) + " ");
        continue;
      }
      closeQuote();

      html.push("<p>" + inline(line) + "</p>");
    }
    closeList();
    closeQuote();

    return html.join("\n");
  }

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
      monogram.innerHTML = WAVE_SVG;
      const title = document.createElement("strong");
      title.textContent = "Ask Ocean";
      const detail = document.createElement("span");
      detail.textContent =
        "Editor context — active file, selection, diagnostics — is injected automatically.";
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
      // Assistant text is Markdown (mirrors the web app's block--text); user
      // and system rows stay verbatim.
      if (msg.role === "assistant" && msg.content) {
        body.innerHTML = renderMarkdown(msg.content);
      } else {
        body.textContent = msg.content;
      }

      div.append(label, body);
      messagesEl.appendChild(div);
    }
    messagesEl.scrollTop = messagesEl.scrollHeight;
  }

  function autoGrow() {
    input.style.height = "auto";
    input.style.height = Math.min(input.scrollHeight, 190) + "px";
  }

  function sendComposer() {
    const text = input.value.trim();
    if (!text) {
      return;
    }
    vscode.postMessage({ type: "send", text });
    input.value = "";
    autoGrow();
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

  input.addEventListener("input", autoGrow);

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
