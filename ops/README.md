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
