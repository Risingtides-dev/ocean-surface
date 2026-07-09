# Ocean Surface — Ops

## ocean-surface-proxy is supervised by launchd (OCEAN-161 / OCEAN-385)

The **surface proxy** (`crates/ocean-surface-proxy`, built to
`target/release/ocean-surface-proxy`) serves the compiled PWA bundle from `dist/`
and reverse-proxies `/v1/*` to the Ocean daemon. It listens on
**`0.0.0.0:8790`** by default.

Previously it was hand-launched via `run-surface.sh` with **no supervision** — if
it crashed, the web surface went **silently offline**. It is now run under a
launchd **LaunchAgent** that respawns it on crash (`KeepAlive`) and starts it at
login (`RunAtLoad`).

| Thing | Value |
|---|---|
| launchd label | `dev.risingtides.ocean-surface-proxy` |
| Version-controlled plist | `deploy/dev.risingtides.ocean-surface-proxy.plist` |
| Launcher it execs | `deploy/ocean-surface-proxy.sh` |
| Installed plist path | `~/Library/LaunchAgents/dev.risingtides.ocean-surface-proxy.plist` |
| Bind address | `0.0.0.0:8790` (env `OCEAN_SURFACE_BIND`) |
| Bundle served | `~/.config/ocean-surface/dist-prod` (env `OCEAN_SURFACE_DIST`; repo `dist/` is dev-only) |
| Daemon proxied to | `http://127.0.0.1:4780` (env `OCEAN_DAEMON_URL`) |
| Auth env file | `~/.config/ocean-surface/proxy-auth.env` (0600; sourced by the launcher) |
| Logs (stdout+stderr) | `/private/tmp/ocean-surface-proxy.log` |

> The launcher serves a **prebuilt** bundle — it does **not** run `trunk build` on
> every respawn (that's what `run-surface.sh` is for during dev). Prod serves the
> dedicated `dist-prod` dir so a local `trunk serve` / `run-surface.sh` loop can
> never clobber the public site. Rebuild + rsync into `dist-prod` after UI changes
> (see the ocean-surface-prod-deploy skill); re-running the install script alone is
> not enough if you only refreshed repo `dist/`.
>
> Secrets stay out of the plist. The xAI voice key is resolved from
> `~/.config/ocean-surface/xai.key` (or env `XAI_API_KEY`). HTTP Basic auth is
> **on by default** and requires operator-supplied creds in
> `~/.config/ocean-surface/proxy-auth.env` (0600) exporting
> `OCEAN_SURFACE_AUTH=on`, `OCEAN_SURFACE_USER`, and `OCEAN_SURFACE_PASS`
> (password typically read from `proxy-basic-auth.txt`). There are **no**
> built-in operator credentials. The tracked plist template does **not** set
> `OCEAN_SURFACE_AUTH`; do not put USER/PASS in the plist. For a trusted
> localhost throwaway only, export `OCEAN_SURFACE_AUTH=off` in the process
> environment (never commit that override into the template).

### Install / enable supervision

```bash
# Stage only — builds + copies the plist, then PRINTS the launchctl commands:
ops/install-surface-proxy.sh

# Or actually start supervision now (touches the live launchd domain):
ops/install-surface-proxy.sh --bootstrap
```

By default the script is **scripts-only**: it builds the proxy (release, **from
main** — warns if you're on a feature branch), ensures a valid `dist/` bundle
exists, copies the plist into `~/Library/LaunchAgents/`, and then **prints** the
`launchctl bootstrap/enable/kickstart` commands for you to run. The live bootstrap
is **opt-in** via `--bootstrap` so a routine re-run never restarts the service out
from under you. Idempotent — safe to re-run after a pull/rebuild. (Equivalent manual steps:
`cp deploy/dev.risingtides.ocean-surface-proxy.plist ~/Library/LaunchAgents/` then
`launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/dev.risingtides.ocean-surface-proxy.plist`
and `launchctl enable gui/$(id -u)/dev.risingtides.ocean-surface-proxy`.)

### Check status

```bash
# Is it listening?
lsof -nP -iTCP:8790 -sTCP:LISTEN

# launchd's view (state, pid, last exit code):
launchctl print gui/$(id -u)/dev.risingtides.ocean-surface-proxy | grep -E 'state|pid|last exit'

# Unauthenticated health endpoint:
curl -fsS http://127.0.0.1:8790/health && echo
```

### Restart / read logs

```bash
# Force a restart (e.g. after rebuilding the binary or bundle):
launchctl kickstart -k gui/$(id -u)/dev.risingtides.ocean-surface-proxy

# Tail logs:
tail -f /private/tmp/ocean-surface-proxy.log
```

### Uninstall / stop supervision

```bash
ops/uninstall-surface-proxy.sh
```

Boots the job out of launchd and removes the installed plist. The repo, the built
binary, and `dist/` are left untouched.

> **Note on the daemon:** the Ocean **daemon** (`:4780`) on this box is currently
> hand-launched and is **not** covered by this LaunchAgent — this ticket only
> supervises the **surface proxy**. Supervision state on this box drifts, so
> re-verify with `launchctl list | grep -i ocean` before assuming either process
> is supervised.
