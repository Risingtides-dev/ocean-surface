# Ocean Surface — Ops

## ocean-surface-proxy is supervised by launchd (OCEAN-161 / OCEAN-385)

The **surface proxy** (`crates/ocean-surface-proxy`, built to
`target/release/ocean-surface-proxy`) serves the compiled PWA release selected
by `~/.config/ocean-surface/current` and reverse-proxies `/v1/*` to the Ocean
daemon. It listens on **`0.0.0.0:8790`** by default.

Two launchd LaunchAgents own the live surface: the proxy respawns on crash and
starts at login; the deploy watcher polls `origin/main` every two minutes and
promotes a new release only after a clean detached-main build passes its gates.

| Thing | Value |
|---|---|
| Proxy launchd label | `dev.risingtides.ocean-surface-proxy` |
| Deploy launchd label | `dev.risingtides.ocean-surface-auto-deploy` |
| Version-controlled plists | `deploy/dev.risingtides.ocean-surface-*.plist` |
| Installed launchers | `~/.config/ocean-surface/bin/ocean-surface-{proxy,auto-deploy}.sh` |
| Installed plist path | `~/Library/LaunchAgents/dev.risingtides.ocean-surface-*.plist` |
| Bind address | `0.0.0.0:8790` (env `OCEAN_SURFACE_BIND`) |
| Bundle served | `~/.config/ocean-surface/current` (atomic symlink; repo `dist/` is dev-only) |
| Deployed revision | `~/.config/ocean-surface/deployed-rev` |
| Daemon proxied to | `http://127.0.0.1:4780` (env `OCEAN_DAEMON_URL`) |
| Auth env file | `~/.config/ocean-surface/proxy-auth.env` (0600; sourced by the launcher) |
| Logs | `/private/tmp/ocean-surface-{proxy,auto-deploy}.log` |

> The proxy serves a **prebuilt immutable release** — it does not build on
> respawn. The deploy watcher fetches `origin/main`, builds in a disposable
> detached worktree, runs the WASM check/tests/strict Clippy/format gate, builds
> the release WASM and proxy, validates the wasm magic and release HTML, then
> atomically advances `current` and `deployed-rev`. Any failure leaves the
> last-known-good release selected. A local `trunk serve` / `run-surface.sh`
> loop cannot touch the live release.
>
> Secrets stay out of the plist. The xAI voice key is resolved from
> `~/.config/ocean-surface/xai.key` (or env `XAI_API_KEY`). Operator login is
> **on by default** and requires operator-supplied creds in
> `~/.config/ocean-surface/proxy-auth.env` (0600) exporting
> `OCEAN_SURFACE_AUTH=on`, `OCEAN_SURFACE_USER`, and `OCEAN_SURFACE_PASS`
> (plus `OCEAN_SURFACE_COOKIE_SECURE=on` behind the public HTTPS tunnel).
> There are **no**
> built-in operator credentials. The tracked plist template does **not** set
> `OCEAN_SURFACE_AUTH`; do not put USER/PASS in the plist. For a trusted
> localhost throwaway only, export `OCEAN_SURFACE_AUTH=off` in the process
> environment (never commit that override into the template).

### Install / enable supervision

```bash
# Stage only — builds and promotes HEAD, then installs both plist files:
ops/install-surface-proxy.sh

# Build/promote and start both supervised jobs now:
ops/install-surface-proxy.sh --bootstrap
```

The installer hard-fails unless HEAD is `main` or detached exactly at
`origin/main`; `--allow-non-main` is the explicit escape hatch. It builds the
proxy and release bundle, seeds the immutable release store and `current`
symlink, copies both launchers out of the mutable shared checkout, validates
both plist files, then either prints the launchctl commands or bootstraps both
jobs. Later main revisions deploy automatically.

### Check status

```bash
# Is it listening?
lsof -nP -iTCP:8790 -sTCP:LISTEN

# launchd's view:
launchctl print gui/$(id -u)/dev.risingtides.ocean-surface-proxy | grep -E 'state|pid|last exit'
launchctl print gui/$(id -u)/dev.risingtides.ocean-surface-auto-deploy | grep -E 'state|pid|last exit'

# Exact revision selected by the live symlink:
cat ~/.config/ocean-surface/deployed-rev
readlink ~/.config/ocean-surface/current

# Unauthenticated health endpoint:
curl -fsS http://127.0.0.1:8790/health && echo
```

### Restart / read logs

```bash
# Force a proxy restart:
launchctl kickstart -k gui/$(id -u)/dev.risingtides.ocean-surface-proxy

# Trigger an immediate main check/deployment:
launchctl kickstart -k gui/$(id -u)/dev.risingtides.ocean-surface-auto-deploy

# Tail logs:
tail -f /private/tmp/ocean-surface-proxy.log
tail -f /private/tmp/ocean-surface-auto-deploy.log
```

### Uninstall / stop supervision

```bash
ops/uninstall-surface-proxy.sh
```

Boots both jobs out of launchd and removes both installed plist files. The repo,
built artifacts, and immutable deployed releases are left untouched.

> **Note on the daemon:** the Ocean **daemon** (`:4780`) is separate from these
> surface LaunchAgents. Re-verify supervision with
> `launchctl list | grep -i ocean` instead of assuming process state.

## Devices — reaching your own machines from one login

A person's roster entry in `~/.config/ocean-surface/users.json` may carry a
list of **devices**: the machines whose Ocean daemons that login can attach to.
Signing in at `https://ocean.agentsworld.org` and picking a device lands the
browser in that machine's sessions; switching devices is a click, not a second
login. The choice is recorded server-side in
`~/.config/ocean-surface/device-selections.json` (0600, written atomically), so
it survives a proxy restart and a deploy — keyed by a digest of the session
cookie **and** an opaque per-browser id (the `ocean_device` cookie), so one
person's phone and desktop hold their own choices instead of re-pointing each
other. Selecting also ends any event stream still open on the machine being
left, so a tab that was already connected reconnects onto the new one rather
than blending two machines into one transcript.

```json
[
  {
    "username": "ecfromthedc",
    "password": "…",
    "devices": [
      { "name": "mac mini", "daemon_url": "http://100.119.217.76:4780",
        "observer_token_path": "/Users/e/.config/ocean-rs/observatory-token",
        "operator_key_path": "/Users/e/.config/ocean-rs/operator.key",
        "default": true },
      { "name": "studio", "daemon_url": "http://100.64.0.12:4780" }
    ]
  }
]
```

Rules the proxy enforces when it loads this file:

- The file must be **0600**; a group- or world-readable roster is refused at
  startup, as before — it holds every teammate's password.
- Device names are unique per person and non-empty; `daemon_url` must be an
  absolute `http://` or `https://` URL; **at most one** device may be marked
  `"default": true`, and with none marked the first in the list wins.
- The legacy single `daemon_url` on a user entry still works and becomes that
  person's one device, **named after its host**. Setting both `daemon_url` and
  `devices` on one entry is refused rather than merged.
- `observer_token_path` and `operator_key_path` belong to the device whose
  daemon minted them. There is no fallback across machines: a device that names
  no credential gets none, and Observatory or Room-authority routes fail closed
  rather than sending one machine's token to another.

### On each device

Run the Ocean daemon bound to **that machine's tailnet address only**:

```bash
# on the device, from ../ocean-os
OCEAN_BIND=100.64.0.12:4780 cargo run --release -p ocean-daemon
#           ^ that machine's own tailscale IP — never 0.0.0.0, never a LAN IP
```

Then add the device to the person's entry **on the mini** (where the proxy and
the roster live):

```bash
ops/add-device.sh ecfromthedc studio http://100.64.0.12:4780
ops/add-device.sh ecfromthedc studio http://100.64.0.12:4780 \
  --observer-token /Users/e/.config/ocean-rs/observatory-token \
  --operator-key   /Users/e/.config/ocean-rs/operator.key \
  --default
launchctl kickstart -k gui/$(id -u)/dev.risingtides.ocean-surface-proxy
```

`ops/add-device.sh` rewrites `users.json` atomically with 0600 preserved and
**refuses a `daemon_url` that is not on the tailnet** (Tailscale's `100.64/10`
CGNAT range, a `*.ts.net` name, or loopback) unless `--allow-public` is passed.
The proxy re-reads the roster at startup, so kick the LaunchAgent after an edit.

> **The tailnet ACL is the whole boundary — say it out loud.** The Ocean daemon
> has no authentication of its own; its security model is that it listens on
> loopback and only local processes can reach it (`OCEAN_BIND`, default
> `127.0.0.1:4780`). Binding it to a tailnet address trades that for the
> tailnet's own authentication and ACLs: anything that can reach the address
> can drive the daemon — read every session, run every tool, write every file
> it can write. So bind the tailnet interface specifically (never `0.0.0.0`,
> which also exposes it to whatever café Wi-Fi the laptop is on), keep the
> device tagged and ACL'd to your own nodes, and do not port-forward it.
>
> This is a person reaching **their own** machines, and nothing else. It is not
> the cross-person federation path: two people's daemons still never accept
> each other's connections, and shared Rooms still federate through Bedrock —
> see `docs/superpowers/specs/2026-07-10-ocean-federated-rooms-design.md`.
> Do not use a device entry to hand somebody else a login onto your daemon.

### Check it from the surface

```bash
# As a signed-in browser would (cookie from the login form):
curl -fsS -b "ocean_session=…" https://ocean.agentsworld.org/api/devices | jq
curl -fsS -b "ocean_session=…" -H 'content-type: application/json' \
  -d '{"name":"studio"}' https://ocean.agentsworld.org/api/devices/select
```

`GET /api/devices` probes each daemon's `/health` with a short timeout and
reports `ok` (with `version`/`rev`), `unhealthy`, or `unreachable`. It answers
device **names** only — no `daemon_url` is ever published to the browser, so
nobody types a URL and no page learns your tailnet addresses. A request routed
to a device that is unreachable, or to one the roster no longer has, answers
`503 {"error":"device_unavailable"}` naming the device.
