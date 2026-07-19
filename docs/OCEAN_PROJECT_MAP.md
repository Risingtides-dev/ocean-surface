# Ocean Project Map

Status: active cross-repo orientation map for agents.

This local boundary summary lets an agent route work from `ocean-surface`; the
canonical public ecosystem map lives in
`ocean-os/docs/OCEAN_PROJECT_MAP.md`. This map does not replace the local
`AGENTS.md` chain. Read the target repo's `AGENTS.md` before editing.

Visibility: `ocean-os`, `ocean-surface`, and `ocean-agents` are public.
`ocean-bedrock` is a private repository and an optional authenticated knowledge
service for team deployments. Public Ocean use must not require Bedrock access.

## First Routing Rule

| If the user says | Start here | Why |
| --- | --- | --- |
| runtime, daemon, tools, permissions, providers, models, sessions, events, TUI, ACP, MCP client | `../../ocean-os` | Rust runtime authority and daemon API |
| surface, GUI, GPUI, web, PWA, Chrome extension, Cursor or VS Code extension, voice, canvas, LiveKit | `../../ocean-surface` | Thin client surfaces that steer the daemon |
| assistant, agent package, surface profile, Bonzai, content-agent, courier, Slack bridge, Slack delivery | `../../ocean-agents` | Provider-agnostic assistant/courier packages and profiles |
| bedrock, Longhouse files, shared docs, context, handoffs, ledger, source runners, semantic search, graph, MCP wrapper, cloud box | `../../ocean-bedrock` (authorized private checkout) | Optional shared knowledge/data plane and collaboration substrate |

When in doubt, inspect the live repo and code path before answering. Do not
collapse these repos into one product surface.

## Ecosystem Connection Map

The three public repos form the local product and package system. An authorized
team deployment may additionally connect the private Bedrock service. The
routing table only identifies ownership; many features cross repo boundaries.

| Connection | Contract |
| --- | --- |
| `ocean-surface` -> `ocean-os` | Surfaces create/select sessions, submit turns, subscribe to scoped SSE, and render daemon state. The daemon owns the actual session, tool, model, provider, and permission decisions. |
| `ocean-os` -> `ocean-surface` | The daemon emits events, permission requests, tool activity, model/session state, and runtime results for surfaces to present. Surface-specific context travels through `client_type`, `session_id`, `cwd`, `project_id`, guidance, and room/canvas fields. |
| `ocean-os` <-> `ocean-agents` | Agent packages supply editable profiles, manifests, SOPs, tool declarations, couriers, and assistant source material. `ocean-os` supplies the runtime that enforces tools, permissions, providers, memory, sessions, and typed contracts. |
| `ocean-os` <-> `ocean-bedrock` | Optional, authenticated integration: `ocean-os` can use Bedrock data as shared context, workflow specs, handoffs, ledger history, source-runner output, and MCP/API-accessible knowledge. Local machine side effects still go through daemon permission gates. |
| `ocean-surface` <-> `ocean-agents` | Surfaces expose the context that selects or shapes assistant behavior, such as `client_type`, active surface, session, room, canvas, Slack, or editor state. The agent behavior itself remains package/profile data, not UI code. |
| `ocean-surface` <-> `ocean-bedrock` | In an authorized team deployment, surfaces may display or request shared files, context, handoffs, search results, and workflow artifacts through authenticated Bedrock APIs or a daemon-owned proxy. Browser state never becomes the shared-storage authority. |
| `ocean-agents` <-> `ocean-bedrock` | In an authorized team deployment, agents and couriers may use authenticated Bedrock contracts for shared collaboration records. Public packages cannot assume Bedrock access. |
| all connected components | A team workflow may begin in a surface, run through the daemon, be shaped by an agent package, optionally retrieve or write authenticated shared context in Bedrock, and stream results back to the surface. |

## System Shape

```text
operator
  |
  v
ocean-surface  ->  ocean-os daemon :4780  ->  ocean-runtime / tools / providers
thin clients       sessions, turns, SSE        permission-gated local execution
  |
  | sends client_type, session_id, cwd/project intent
  v
surface context

ocean-agents  ->  profiles, manifests, assistants, couriers
                  package/source material consumed by Ocean workflows

ocean-bedrock (private, optional)
              ->  authenticated shared files, context, ledger, graph, search
```

## Repo Ownership

| Repo | Owns | Does not own |
| --- | --- | --- |
| `ocean-os` | Rust daemon, agent loop, provider routing, tool execution, local sessions, permissions, TUI/CLI/ACP, Longhouse coordination crate, local memory/context crates | Product UI chrome, shared cloud file store, agent package content |
| `ocean-surface` | Canonical Leptos web/PWA UI, Tauri and extension hosts, proxy, voice/canvas presentation; retained GPUI source is reference-only | Provider calls, reasoning loop, session authority, permission policy, tool execution |
| `ocean-agents` | Assistant/courier folders, editable profiles, manifests, SOP/tool declarations, transitional Python harnesses | Runtime enforcement, provider credentials, daemon-owned session storage |
| `ocean-bedrock` (private, optional) | Authenticated shared filesystem, Ocean Ledger/Context, graph/semantic search, source runners, and workflow data for authorized team deployments | Local machine execution authority, UI session ownership, provider routing |

## Shared Contracts

- `Project -> Workspace -> Session -> Turns -> Events` is the daemon-side model.
- `Surface -> Session` is explicit. A surface creates or chooses a `session_id`
  before posting turns.
- `client_type` names the medium, such as `surface-gpui`,
  `surface-web`, or `surface-extension`. It is not a session id or workspace id.
- `ocean-surface` must call the daemon API instead of inventing agent state.
- `ocean-agents` packages can define profiles, tools, SOPs, and courier
  contracts, but runtime enforcement belongs in `ocean-os`.
- An authorized `ocean-bedrock` deployment can provide optional authenticated
  shared context and APIs, but public clients and packages cannot require it and
  local shell/filesystem side effects still route through `ocean-os` permission gates.
- Longhouse has two pieces: `ocean-os/crates/ocean-longhouse` for local/runtime
  coordination logic and, when configured for an authorized team deployment,
  private `ocean-bedrock` data-plane support.

## Live Ports And APIs

| Service | Default | Owner |
| --- | --- | --- |
| Ocean daemon | `127.0.0.1:4780` | `ocean-os` |
| Standalone Longhouse service direction | `127.0.0.1:4781` | `ocean-os` |
| Surface proxy/dev web app | `0.0.0.0:8790` via `./run-surface.sh` | `ocean-surface` |
| Ocean Bedrock server (private, optional) | deployment-defined; local default `:8080` | `ocean-bedrock` |

Core daemon routes used by surfaces:

```text
POST /v1/agent/sessions
GET  /v1/agent/events?session_id=<id>
POST /v1/agent/turns
POST /v1/voice/stt
POST /v1/voice/tts
POST /v1/voice/realtime/client-secret
GET  /health
```

Voice is daemon-owned as of voice phase 4 (2026-07-10): the surface proxy's
`/api/stt` and `/api/tts` forward to the daemon's `/v1/voice/stt` and
`/v1/voice/tts`, and the browser Realtime client mints its ephemeral secret
via `POST /v1/voice/realtime/client-secret`. Provider keys (xAI, OpenAI)
resolve only inside `ocean-os`.

Authorized maintainers can inspect the private checkout's `docs/API.md`,
`docs/openapi.yaml`, and `docs/MCP.md`. Public surfaces must rely only on
published integration contracts rather than private repository paths.

## Source Anchors

Read these public source anchors before making cross-repository claims:

- `../../ocean-os/AGENTS.md`
- `../../ocean-os/README.md`
- `../../ocean-os/docs/OCEAN_ECOSYSTEM_CONTRACT.md`
- `../../ocean-os/docs/LONGHOUSE.md`
- `../../ocean-os/Cargo.toml`
- `../../ocean-agents/AGENTS.md`
- `../../ocean-agents/docs/AGENT_FILESYSTEM_ARCHITECTURE.md`
- `../../ocean-agents/docs/ocean-agents-builds.toml`
- `../../ocean-agents/assistants/README.md`
- `../../ocean-agents/couriers/ARCHITECTURE.md`
- `../../ocean-surface/AGENTS.md`
- `../../ocean-surface/README.md`
- `../../ocean-surface/docs/OCEAN_GPUI_CANVAS_LIVEKIT_SPEC.md`
- `../../ocean-surface/Cargo.toml`

Authorized maintainers may additionally inspect private Bedrock anchors:

- `../../ocean-bedrock/README.md`
- `../../ocean-bedrock/docs/OCEAN-CONTEXT.md`
- `../../ocean-bedrock/docs/OCEAN-LONGHOUSE-DATA-PLANE.md`
- `../../ocean-bedrock/docs/MCP.md`
- `../../ocean-bedrock/workflows/`

## Confusion Guards

- Do not put provider calls or permission policy in `ocean-surface`.
- Do not treat `ocean-agents` Python harnesses as the final runtime engine; they
  are package-side conventions or transitional harnesses unless a manifest says
  otherwise and the daemon path verifies it.
- Do not treat private, optional `ocean-bedrock` as a local root shell or a
  public-client dependency. It is an authenticated team knowledge/data plane,
  not the user's machine authority.
- Do not assume the daemon is reading a repo profile just because a local file
  exists. Verify the running daemon's profile source when the distinction
  matters.
- If another checkout with the same repo name exists, verify `pwd`, `git
  rev-parse --show-toplevel`, and `git remote -v` before writing.

## Maintenance

Keep this local boundary summary aligned with the canonical public map in
`ocean-os/docs/OCEAN_PROJECT_MAP.md`. The private Bedrock repository maintains
its own operating references. Update the nearest owning `AGENTS.md` and append
`events.md` entries where that repo's devlog contract requires it.
