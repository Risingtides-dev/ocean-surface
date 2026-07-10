# Ocean Surface — session handoff

Snapshot **2026-07-10 ~22:20 EDT (2026-07-11 02:20 UTC)**, written by the
green-up session (claude/fable-5). One live handoff per project — this is it.
Read order: **this file → AGENTS.md (auto-loaded) → `events.md` tail**.
Sibling repo `../ocean-os` (daemon/runtime authority) has its own docs.

## How to handle this doc (read first, this is the contract)

Stale handoffs are worse than none. This doc is **disposable by design**.

1. **Trust the commands, not the numbers.** This box runs several concurrent
   agent sessions (codex desktop, claude, omp). Every snapshot fact below
   carries a re-derivation command — run it before acting on the fact.
2. **Kill rot on contact.** If you act on a section and find it stale, fix or
   delete that section in place, immediately. Never leave a known-wrong fact
   standing for the next reader.
3. **Supersede, don't append.** At a major milestone, a big landing, or
   before a context reset:
   1. Write a fresh `handoff.md` — current state, decisions, next steps,
      blockers, paths. No history, no resolved issues, no dead ends.
   2. Archive this file:
      `mv handoff.md .agentignore/handoff-archived-<YYYY-MM-DD>[-n].md`.
   3. Log the refresh in `events.md` (schema in `~/.claude/CLAUDE.md`).
4. **`.agentignore/` is archived context — never read it back into active
   thinking.** It exists for forensics only.
5. Keep this doc scannable (~this length). If a section only restates
   AGENTS.md, delete it; AGENTS.md is the standing contract, this file is
   only the *live delta*.

## The system in one paragraph

Ocean is two repos, one system. `../ocean-os` is the brain: Rust daemon
(`127.0.0.1:4780`, hand-launched, NOT supervised) owning the agent loop,
tools, providers, sessions, events. This repo is the face: ONE Leptos/WASM
app (`crates/ocean-surface-ui`) built by Trunk, shipped to three shells —
browser PWA (served by `crates/ocean-surface-proxy` behind launchd +
cloudflared), Tauri 2 native desktop (`crates/ocean-tauri`, same `dist/`),
and a Chrome extension. GPUI (`crates/ocean-gui`) is soft-deprecated,
mine-only. Surfaces attach to daemon sessions (`POST /v1/agent/sessions` →
`GET /v1/agent/events?session_id=` → `POST /v1/agent/turns`); never adopt
sessions off the global stream.

## Live topology (verify each; all checks passed at snapshot)

| Thing | Value | Verify |
|---|---|---|
| Prod origin | `https://ocean.agentsworld.org` → cloudflared → `:8790` | `curl -sS -m 10 -o /dev/null -w '%{http_code}\n' https://ocean.agentsworld.org/health` → 200 |
| Proxy supervisor | launchd `dev.risingtides.ocean-surface-proxy`, state running | `launchctl print gui/$(id -u)/dev.risingtides.ocean-surface-proxy \| grep state` |
| Prod bundle | `~/.config/ocean-surface/dist-prod` serving `ocean-surface-ui-75f5d7b6886c572_bg.wasm` — **peer-deployed, commit provenance unverified** | authed `curl :8790/` grep `_bg.wasm` vs `ls ~/.config/ocean-surface/dist-prod/*_bg.wasm` |
| Basic auth | ON; authoritative creds **only** `~/.config/ocean-surface/proxy-auth.env` (0600). `proxy-basic-auth.txt` is stale/401. Never print secrets | subshell: `. ~/.config/ocean-surface/proxy-auth.env; curl -su "$OCEAN_SURFACE_USER:$OCEAN_SURFACE_PASS" -o /dev/null -w '%{http_code}' http://127.0.0.1:8790/` → 200; unauth `/` → 401 |
| Daemon | `:4780` healthy, backend `deepseek/deepseek-v4-pro`, 0 persist/gc failures | `curl -sS -m 3 http://127.0.0.1:4780/health` |
| Keys | tools: `~/.config/ocean-rs/tools.env` · providers: `~/.config/ocean-rs/auth.json` · xAI (transitional STT/TTS): `~/.config/ocean-surface/xai.key` | — |
| Ops runbook (private) | `~/.config/ocean-surface/ops-runbook.md` | — |

## State of the mains (the headline)

- **surface `origin/main` tip `9b24614` — CI RED.** The green baseline is
  `c7db9fc` (first fully-green run in repo history, 2026-07-10 21:53Z; four
  layers of serial-step debt cleared: proxy dead code → 101 ui wasm clippy →
  fmt --all gate → gui-check canary). Peer voice landings above it
  (`7ea1cab`, `9d98529` Realtime hardening + `9b24614` docs) re-broke it.
  **First real task for the takeover agent: re-green the tip** — skill
  `ci-layered-debt-greenup` is the procedure (enumerate ALL ci.yml steps,
  reproduce locally in one pass, push once). Verify:
  `gh run list --repo Risingtides-dev/ocean-surface --workflow CI --limit 3`.
- **ocean-os `origin/main` tip `a0eef51c` — CI green.** TUI shell rebuild
  merged, skill packs live (SKILL.md under `~/.config/ocean-rs/skills/<name>/`
  indexes on the running daemon; query field is `query`, NOT `prompt` —
  wrong field silently returns empty). Local checkout sits on the merged
  `feat/ocean-tui-shell-rebuild` branch — harmless, switch/fetch as needed.

## Landed tonight (do NOT rebuild these)

- **Pinned widget rail** (`234f431`): `props.placement:"pinned"` docks
  components to a session-scoped rail — registry in `daemon.rs`,
  `PinnedRail`/`PinnedCard` in `components.rs`, CSS `panels.css` +
  `compact.css` (side rail ≥1480px, else strip under header). Wire-compatible.
- **MCP client + CapabilityRegistry**: already shipped on ocean-os main
  (`McpProvider`, `[[mcp.server]]` in ocean.toml, `tools.env` passthrough,
  agent loop consumes `registry.tools_for_session`). Old roadmap said "build
  this" — it's done.
- **Rooms federation architecture doc** (`8ac3977`) + Realtime voice
  hardening (peer, in flight — see below).
- Council deck's dead proxy route deleted — council is native in-app now
  (`/ui/council` + `COUNCIL_DECK_HTML` are gone from the proxy; don't
  resurrect).

## Workspace state — READ BEFORE ANY COMMIT

Shared **GitButler** checkout (`but`, never raw git write cmds in it).
Concurrent sessions hold live uncommitted WIP: at snapshot **27 modified +
92 untracked**, and a peer was actively editing **`voice/mod.rs`,
`voice/realtime.rs`, rooms plans** (mtimes < 1h). Extra worktrees:
`/private/tmp/ocean-rooms-surface` (peer, `feat/slack-quality-rooms-surface`)
and `/private/tmp/ocean-surface-land` (detached, likely dead — verify then
prune).

- Most dirt is **stale-base phantoms** (byte-identical to origin/main).
  Classify before believing `but status`:

  ```sh
  cd ~/dev/ocean-surface && but status 2>&1 | grep -oE '[AM] [^ ]+$' | awk '{print $2}' | sort -u | \
  while read -r f; do [ -f "$f" ] || continue; w=$(git hash-object "$f"); \
  m=$(git rev-parse -q --verify "origin/main:$f" 2>/dev/null); \
  if [ -z "$m" ]; then echo "NEW $f"; elif [ "$w" = "$m" ]; then echo "MATCH $f"; else echo "DIFFERS $f"; fi; done | sort | uniq -c | sort -rn
  ```

- DIFFERS files = peer WIP or stale snapshots. **Never commit a shared-tree
  file without `git diff origin/main -- <file>` first** — committing a stale
  copy silently reverts landed work (has happened; sessions.rs/app.rs are
  repeat offenders).
- Landing from this checkout: use a detached worktree at origin/main and
  cherry-pick your exact content (skill `ocean-surface-clean-room-landing`).
  Expect push races — fetch+rebase immediately before every push.
- `events.md` merge conflicts (every landing race): resolve with the
  conflict reader — read `<file>:conflicts`, write `conflict://N` content
  `@both` (ours-then-theirs keeps both ledger tails). Main's copy is
  canonical; union-merge, never drop entries.
- `but pull` refuses over uncommitted changes — that's peers' WIP, don't
  force it. Recovery if it goes wrong: skill `gitbutler-workspace-recovery`.

## Hard rules (violations have burned us)

1. **NEVER launch/relaunch the Tauri app while John may be using it.**
   Verify via web (`:8790`), a throwaway proxy, or passive screenshots;
   launch only on explicit "show me" (skill `ocean-tauri-launch-verify`).
2. Census uncommitted UI work on a throwaway proxy
   (`OCEAN_SURFACE_BIND=127.0.0.1:8791 OCEAN_SURFACE_DIST=$PWD/dist
   OCEAN_SURFACE_AUTH=off` + fresh `trunk build --release`), never `:8790`
   (prod snapshot). Skill `ocean-surface-headless-ui-census`.
3. Prod deploys: committed-tree-only, detached worktree, rsync to
   `dist-prod`, verify served hash on both origins. Skill
   `ocean-surface-prod-deploy`.
4. Stale service worker serves ancient shells — reload twice / fresh profile
   before diagnosing "regressions".
5. `styles/` is enumerated in THREE places (index.html,
   `extension/sidepanel.html`, `scripts/build-extension.sh`); colors only in
   `styles/tokens.css`.
6. `let x = x;` closure rebinds are banned (clippy 1.97 `redundant_locals`,
   swept repo-wide tonight). For Leptos capture use
   `StoredValue::new(x.clone())` + `.get_value()` — the `daemon_for_*` idiom
   in app.rs.
7. Full green gate = CI's exact steps: proxy build+test, ui wasm check,
   clippy (proxy all-targets AND ui wasm) `-D warnings`, **`cargo fmt --all
   --check`**, and gui-check (`RUSTFLAGS='-D warnings' cargo check -p
   ocean-gui --all-targets`). Serial steps mask later ones — run ALL locally
   before pushing (skill `ci-layered-debt-greenup`).

## Next phases (claims, not truth — re-derive at Phase 0)

- **0 — Re-ground (always).** Fetch both repos; main tips vs this doc;
  `but status` + phantom classifier; prod + daemon health; `events.md` tail
  for peer activity since snapshot.
- **1 — Re-green surface main.** Tip `9b24614` is red (peer voice landings).
  Layered-debt procedure; coordinate with the voice peer if the break is in
  their in-flight area.
- **2 — Workspace de-rot.** Gated on peers (codex sessions-create, voice/rooms
  WIP) landing or dropping. Then `but` base sync → phantoms dissolve.
- **3 — Prod provenance.** Verify served `75f5d7b6` wasm corresponds to a
  committed main tree; if unverifiable, redeploy from tip via the
  prod-deploy skill, then finish the deferred prod deep-drive (PWA/SW update
  path, transcript lifecycle, command/overlay matrix).
- **4 — Desktop follow-through.** John's next natural `./run-tauri.sh`
  verifies unified titlebar, circular composer, Files-tab-as-body, slash
  popover. Then: menu-command coverage, folder-picker polish, native
  notifications. LiveKit native stays feature-flagged later.
- **5 — Council v2.** First REAL convene against the live daemon (burns 45s+
  of worker LLM spend — get John's nod). Then convening-state UX, error
  surfaces, streaming progress. Spans `../ocean-os`.
- **6 — Sessions create-flow.** Codex owns the in-flight UI
  (panels.css/sessions.rs). Integrate only after they land.
- **7 — Voice cutover + rooms federation.** STT/TTS ownership proxy → daemon
  (proxy's xAI handling is transitional per AGENTS.md); rooms per `8ac3977`
  doc. **Peer-active RIGHT NOW** — check `events.md` + file mtimes first.
- **8 — Platform roadmap remainder (ocean-os).** More component kinds
  (audio, sortable table, calendar). MCP registry and pinned widgets are
  DONE — strike them from any older list you find.

## Blockers

- Phase 1: break may be inside peer's live voice WIP — coordinate, don't
  clobber.
- Phase 2 gated on peers landing; Phase 4 on John's natural launch; Phase
  5's real convene needs John's nod (LLM spend).

## Skills index (auto-surface next session)

`ci-layered-debt-greenup` · `rust-ci-toolchain-drift-greenup` ·
`gitbutler-multilane-landing` · `ocean-surface-clean-room-landing` ·
`ocean-surface-prod-deploy` · `ocean-surface-headless-ui-census` ·
`ocean-tauri-launch-verify` · `tauri-macos-overlay-titlebar` ·
`gitbutler-workspace-recovery` · `worktree-rot-triage` ·
`ocean-surface-web-ui-loop` · `ocean-surface-web-smoke`
