# Ocean Surface — session handoff

Snapshot **2026-07-10 ~7:15pm EDT (23:15 UTC)**, written by the takeover
session after the Rust 1.97 green-up and committed-tree production deploy.
One live handoff per project — this is it. Read order: **this file → AGENTS.md
(auto-loaded) → `events.md` tail**. Sibling repo `../ocean-os`
(daemon/runtime authority) has its own docs.

## How to handle this doc

Stale handoffs are worse than none. This doc is disposable by design.

1. **Trust commands, not snapshot numbers.** Several agent sessions share this
   checkout. Re-derive state before acting.
2. **Kill rot on contact.** Fix or delete any fact proven stale.
3. **Supersede, don't append.** At a major milestone or context reset, write a
   fresh current-state handoff, move this one to
   `.agentignore/handoff-archived-<YYYY-MM-DD>[-n].md`, and log the refresh in
   `events.md`.
4. **Never read `.agentignore/` back into active thinking.** It is forensic
   history only.
5. Keep this file to live deltas. AGENTS.md owns standing rules.

## The system

Ocean is two repos, one system. `../ocean-os` is the brain: Rust daemon
(`127.0.0.1:4780`, hand-launched, not supervised) owning the agent loop,
tools, providers, sessions, and events. This repo is the face: one
Leptos/WASM app (`crates/ocean-surface-ui`) shipped to the browser PWA through
`crates/ocean-surface-proxy`, the Tauri 2 desktop shell, and the Chrome
extension. GPUI (`crates/ocean-gui`) is soft-deprecated and mine-only.
Surfaces attach to daemon sessions (`POST /v1/agent/sessions` →
`GET /v1/agent/events?session_id=` → `POST /v1/agent/turns`); never adopt a
session from the global stream.

## Live topology

| Thing | Current state | Re-derive |
|---|---|---|
| Prod origin | `https://ocean.agentsworld.org` → cloudflared → `:8790` | `curl -sS -m 10 -o /dev/null -w '%{http_code}\n' https://ocean.agentsworld.org/health` |
| Proxy supervisor | launchd `dev.risingtides.ocean-surface-proxy` | `launchctl print gui/$(id -u)/dev.risingtides.ocean-surface-proxy` |
| Prod bundle | `~/.config/ocean-surface/dist-prod/ocean-surface-ui-11f5c9a2ebf16ff4_bg.wasm`; committed-tree deploy from `a7d0940`; 14,945,218 bytes (prior release 14,949,941) | authed curl of local `:8790/` and tunnel `/`, then compare to `dist-prod` |
| Basic auth | ON; unauth `/` and `/v1` → 401; `/health` → 200. Authoritative credentials only in `~/.config/ocean-surface/proxy-auth.env` (0600); `proxy-basic-auth.txt` is stale | source the env file in a subshell; never print values |
| Daemon | `:4780` healthy at snapshot | `curl -sS -m 3 http://127.0.0.1:4780/health` |
| Tool/provider credentials | `~/.config/ocean-rs/tools.env`; `~/.config/ocean-rs/auth.json` | never print or track |
| Private ops runbook | `~/.config/ocean-surface/ops-runbook.md` | — |

Production quality was verified against the exact deployed `dist-prod` through
a private no-auth census proxy at 390×844: Canvas Tide Coin, five-letter
wordmark, composer, and status dot present; exact `11f5…` WASM; zero
horizontal overflow; zero browser errors. Direct real-origin DOM automation in
this harness was confounded by Basic-auth injection and service-worker state.
For deployment provenance, the authoritative checks are authed curl on both
origins plus the exact-bundle private census.

## State of main

- **Surface `origin/main` tip `a7d0940` — CI GREEN.** GitHub Actions run
  `29129121053` passed both jobs: proxy build/test/clippy, UI wasm check/clippy,
  rustfmt, ocean-gui checks/clippy/tests. The code repair is `ed17423`; the
  ledger commit is `a7d0940`. Verify with
  `gh run list --repo Risingtides-dev/ocean-surface --workflow CI --limit 3`.
- Recent Realtime lifecycle hardening is landed (`7ea1cab`, `9d98529`): stale
  sessions cancel, rAF/WebRTC callbacks tear down safely, and connecting-mic
  races are generation-guarded.
- Already shipped; do not rebuild from old plans: pinned widget rail,
  Soundings loaders and Tide Coin, nested sessions grouping, MCP client +
  CapabilityRegistry in ocean-os, native in-app council deck, and removal of
  the obsolete proxy council page.

## Workspace state — read before any commit

This is a shared GitButler checkout. Its merge-base remains `a9c6da7`, well
behind `a7d0940`, with substantial peer WIP plus stale-base phantoms. Active
areas include the sessions-create UI and voice/rooms work. **Workspace de-rot
is not done and remains gated on peers landing or dropping their work.**

Classify before believing `but status`; never quote a cached count:

```sh
cd ~/dev/ocean-surface && but status 2>&1 | grep -oE '[AM] [^ ]+$' | awk '{print $2}' | sort -u | \
while read -r f; do [ -f "$f" ] || continue; w=$(git hash-object "$f"); \
m=$(git rev-parse -q --verify "origin/main:$f" 2>/dev/null); \
if [ -z "$m" ]; then echo "NEW $f"; elif [ "$w" = "$m" ]; then echo "MATCH $f"; else echo "DIFFERS $f"; fi; done | sort | uniq -c | sort -rn
```

- Treat every DIFFERS file as peer WIP or a stale snapshot until proven
  otherwise. Compare to `origin/main` before committing; `app.rs` and
  `sessions.rs` have repeatedly reverted landed work when committed whole.
- Land from a detached origin/main worktree with
  `ocean-surface-clean-room-landing`; fetch and rebase immediately before
  push.
- Main's `events.md` is canonical. Union-merge ledger conflicts; never drop
  either tail.
- `but pull` refuses over peer WIP. Do not force it. Recovery skill:
  `gitbutler-workspace-recovery`.

## Hard rules

1. **Never launch or relaunch Tauri while John may be using it.** Launch only
   on an explicit “show me”; otherwise use web, a throwaway proxy, or passive
   screenshots (`ocean-tauri-launch-verify`).
2. Census uncommitted UI work on a throwaway proxy at `:8791` with auth off
   and a fresh release build. Never use `:8790` for uncommitted work
   (`ocean-surface-headless-ui-census`).
3. Prod deploys are committed-tree-only: detached worktree, guarded Trunk
   build, `rsync` to `dist-prod`, then hash verification on both origins
   (`ocean-surface-prod-deploy`).
4. A stale service worker can serve an ancient shell. Reload twice or use a
   fresh profile before diagnosing a regression.
5. Stylesheets are enumerated in `index.html`, `extension/sidepanel.html`, and
   `scripts/build-extension.sh`; colors live only in `styles/tokens.css`.
6. Rust 1.97 rejects redundant closure rebinds under clippy. For Leptos
   captures, use the established `StoredValue`/`daemon_for_*` patterns.
7. Full green means every CI step, not the first passing layer: proxy
   build/test/clippy, UI wasm check/clippy, `cargo fmt --all --check`, plus
   ocean-gui check/clippy/tests.

## Next phases

- **0 — Re-ground every session.** Fetch both repos; compare main tips to this
  file; classify GitButler dirt; check prod + daemon health; read the current
  `events.md` tail.
- **1 — Workspace de-rot.** Gated on sessions-create and voice/rooms peers
  landing or dropping WIP. Until then, use detached clean-room landings. CI and
  prod are not blocked on this.
- **2 — Prod currency. COMPLETE.** `11f5c9a2` from green `a7d0940` is live and
  provenance-verified. Recheck only if another deploy occurs.
- **3 — Desktop follow-through.** John's next natural launch verifies the
  unified titlebar, circular composer, Files-as-body, and slash popover. Then
  audit menu-command coverage, folder-picker polish, and native notifications.
  Native LiveKit remains later and feature-flagged. Do not launch without an
  explicit “show me.”
- **4 — Council v2.** A first real convene burns 45s+ of worker LLM calls and
  needs John's nod. Then improve long-running convening state, errors, and
  progress streaming across surface + ocean-os.
- **5 — Sessions create-flow.** Codex owns in-flight `sessions.rs` and
  `styles/panels.css` work. Integrate only after it lands.
- **6 — Voice cutover + rooms federation.** Peer-active. Follow the landed
  daemon-owned STT/TTS and federated-rooms designs; check current events and
  file ownership before editing.
- **7 — Optional authenticated prod deep-drive.** Transcript lifecycle,
  command/overlay matrix, and PWA/SW update path if still desired.
- **8 — Platform remainder in ocean-os.** More component kinds such as audio,
  sortable table, and calendar. MCP registry and pinned widgets are done.

## Blockers

- Workspace de-rot waits for peer WIP to land or be dropped.
- Desktop follow-through waits for John's natural launch; never relaunch it
  autonomously.
- A real council convene needs John's nod because it spends multiple worker
  calls.
- Voice/rooms remain peer-active; coordinate rather than clobber.

## Skills index

`rust-ci-toolchain-drift-greenup` · `gitbutler-multilane-landing` ·
`ocean-surface-clean-room-landing` · `ocean-surface-prod-deploy` ·
`ocean-surface-headless-ui-census` · `ocean-tauri-launch-verify` ·
`tauri-macos-overlay-titlebar` · `gitbutler-workspace-recovery` ·
`worktree-rot-triage` · `ocean-surface-web-ui-loop` ·
`ocean-surface-web-smoke`
