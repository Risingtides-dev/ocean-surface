# Code Context

## Files Retrieved
1. `crates/ocean-daemon/src/main.rs` (lines 668-748, 1822-2036, 4200-4434, 6630-6780, 7663-7739, 9110-9250) - route table, permission-token enforcement, call/voice behavior, and dirty context-usage response fanout.
2. `crates/ocean-runtime/src/agent_loop.rs` (lines 390-464; dirty lines 25-28, 95-96, 245-248, 666-674) - sequential permission gating, run-scoped allow semantics, and new final-round context measurement.
3. `crates/ocean-runtime/src/types.rs` (lines 90-154) - `PermissionDecision`, `requires_permission`, and read-only/mutating defaults.
4. `crates/ocean-agent-sdk/src/lib.rs` (dirty lines 28-47, 399-418, 599-621) - additive `ContextUsage` wire type and optional response/event fields.
5. `crates/ocean-core/src/lib.rs` (dirty lines 236-248) - additive `TokenUsage.context_tokens/context_window` fields with serde defaults.
6. `crates/ocean-agent/src/lib.rs` (dirty lines 1626-1634) - maps runtime final-round context plus effective model window into core usage.
7. `crates/ocean-daemon/src/voice_realtime.rs` (lines 1-160) - OpenAI Realtime secret mint performs a real upstream POST and includes handoff/component tools.
8. `crates/ocean-daemon/src/voice_speech.rs` (lines 1-160) - xAI STT/TTS are real credentialed upstream calls.
9. `deploy/dev.risingtides.ocean-daemon.plist` (dirty lines 26-85) - localizes launcher, home cwd, and PATH from `risingtidesdev` to `smathdaddy-macbook`.
10. `crates/ocean-daemon/AGENTS.md` (lines 1-54) - restart/build/provenance and health contracts.
11. `crates/ocean-tui/src/shell/components/file_tree.rs`, `session_rail.rs`, `session_tray.rs`, and new `crates/ocean-tui/src/shell/rail.rs` (whole-file diff/stat inspection) - large, unrelated active TUI rail/layout work makes the tree unsafe to treat as a clean daemon-only checkout.

## Key Code

### Dirty change summary
- **Wire/runtime feature (medium compatibility concern):** adds truthful provider-reported final-round context occupancy. `AgentRun.context_tokens` records the last provider round while cumulative `usage.total_tokens` remains summed; daemon publishes optional `ContextUsage { used_tokens, context_window, source, measured_at_ms }` on `AgentTurnResponse` and `AgentTurnEvent::TurnFinished`. Fields are additive and `Option`/`#[serde(default)]`, so tolerant clients remain compatible; strict fixture/constructor consumers require updates.
- Fanout touches `ocean-runtime`, `ocean-agent`, `ocean-core`, `ocean-agent-sdk`, daemon, CLI/ACP tests. No permission/call/voice behavior was changed by this dirty diff except adding `context_usage: None` to voice/error response constructors.
- **Deployment-local change (medium):** plist paths now point to this user/repo/home. Installing it would replace the machine service definition; it is not a portable source change.
- **Unrelated active UI work (high stomp risk):** extensive TUI file-tree/session rail/tray edits plus untracked `crates/ocean-tui/src/shell/rail.rs` (overall tree: 637 insertions, 173 deletions).
- Branch is `main`, **11 commits behind `origin/main`**, with 18 modified tracked files and 1 untracked file; nothing staged.

### Permission semantics
- Tools default to no permission requirement but exclusive scheduling; built-in reads (`read`, `ls`, `grep`, `glob`, `web_fetch`) opt into shared/read-only behavior, while mutating/side-effecting tools explicitly return `requires_permission() = true` (`crates/ocean-runtime/src/types.rs:90-154`).
- Permission checks are sequential in tool-call order before execution. `Allow` approves one call; `AllowSession` caches the **tool name for the rest of the current agent run**, not a durable daemon session and not a tool+args tuple; `Deny` emits a synthetic denied result (`crates/ocean-runtime/src/agent_loop.rs:390-464`).
- Daemon `allow_mutating`/effective YOLO bypasses checks by returning `Allow`. Otherwise identical tool+canonical-args within a turn reuses one `PermissionId` (`crates/ocean-daemon/src/main.rs:1969-2036`).
- Permission decision POST must match path/body ID. Client-submitted turns bind waiters to a per-turn `decision_token`; missing/wrong token is 403 without consuming the waiter. Legacy/internal unbound waiters skip token checking. Successful decisions consume the waiter; races/already handled return 404 (`crates/ocean-daemon/src/main.rs:1822-1958`).
- Voice turns without a decision token are rejected up front with 400 unless effective YOLO is on; with a token they use the same gate as text turns (`crates/ocean-daemon/src/main.rs:6630-6780`).

### Safe smoke-test matrix
- **Safe/read-only and actually checked:** `GET /health` and `GET /ready`. Both returned HTTP 200; health reported daemon rev `493b24057a22`, backend `deepseek/deepseek-v4-pro`, and zero persistence/GC failures. Ready confirmed credential presence without exposing the secret.
- **Generally safe/read-only:** `GET /`, `GET /metrics`, `GET /v1/permissions`, `GET /v1/requests`, `GET /v1/model`, `GET /v1/models`, `GET /v1/agents`, `GET /v1/projects`, `GET /v1/lsp`. Some expose operational/user metadata, so use locally only. SSE GETs (`/v1/events`, `/v1/agent/events`) are non-mutating but long-lived and should use a short timeout.
- **Do not speed-run smoke as harmless:** `POST /v1/calls/demo` creates a durable room/transcript and emits events; `POST /v1/calls/place` can dial a real phone; webhook mutates lifecycle state. Realtime secret, STT, and TTS POST upstream and consume provider services. `/v1/agent/voice` runs a real model turn and may create/update sessions. Permission decisions, cancellation, settings/model/project routes, and component/session-message POSTs mutate state.

## Architecture
`POST /v1/agent/turns` and `/v1/agent/voice` enter daemon session/turn orchestration, construct `DaemonPermissionPolicy`, then run `ocean-runtime`. Runtime gates side-effecting calls before scheduling them. Permission requests are broadcast over SSE and resolved through token-bound `POST /v1/permissions/{id}/decision`. Call routes are separate: demo persists synthetic transcript events; place/webhook drive real telephony lifecycle. Voice Realtime/STT/TTS are credential-holding provider proxies, while `/v1/agent/voice` is a normal transcribed agent turn.

## Risk Brief
- **High — do not restart now without owner coordination:** PID `12130` is live on the installed `~/.cargo/bin/ocean-daemon`, cwd is correctly neutral (`/Users/smathdaddy-macbook`), and it is not registered under a matching `launchctl` label. Its PPID is `75685`, so it appears manually/shell supervised rather than the documented LaunchAgent. Restarting loses in-memory request/permission waiters, interrupts active turns/SSE/calls, and may change supervision behavior.
- **High — install/rebuild can stomp active work/artifacts:** the checkout is dirty with cross-crate protocol work and large unrelated TUI work, is behind upstream, and the binary install target is the same executable currently running. A plain Cargo build only writes `target/`, but deployment/install or copying into `~/.cargo/bin` overwrites the live executable; installing the dirty plist changes service ownership/path. Do not build-and-deploy from this tree for a live speed-run.
- **Medium — provenance ambiguity:** running health rev equals local HEAD `493b24057a22`, but compile-time git rev cannot attest whether dirty source was included. Installed binary mtime (01:03:47) is later than daemon source mtime (22:48:35), and process start (00:41:47) predates the installed binary mtime, meaning the on-disk executable has been replaced since this process started. Restarting may therefore change behavior even without another build.
- **Residual:** process inspection cannot prove whether a turn/call is active at this instant; only that the daemon is healthy. No mutation was used to probe active work.

## Start Here
Open `crates/ocean-daemon/src/main.rs` at lines 668-748 first: it is the authoritative route table; then read lines 1822-2036 for permission enforcement before attempting any live turn.