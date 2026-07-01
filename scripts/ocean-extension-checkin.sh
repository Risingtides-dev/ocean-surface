#!/bin/zsh
set -eu

repo="/Users/risingtidesdev/dev/ocean-surface"
log_dir="$HOME/Library/Logs"
log_file="$log_dir/ocean-extension-checkin.log"
stamp="$(date '+%I:%M%p %m-%d-%y %Z')"

mkdir -p "$log_dir"

cat >>"$log_file" <<EOF
[$stamp] Ocean extension polish check-in
repo: $repo
loop:
- Capture Cursor with Codex, Ocean, and Cursor chat visible.
- Remove visible tool-call floods from the Ocean transcript.
- Verify session history/load/resume, model, context-window usage, and settings are reachable without button sprawl.
- Rebuild and reinstall the VSIX, then screenshot the installed extension.
- Keep going until the installed UI is clearly better than the last capture.

EOF

/usr/bin/osascript -e 'display notification "Ocean extension polish pass due. Screenshot, compare, rebuild, reinstall, verify." with title "Ocean UI check-in"' >/dev/null 2>&1 || true

if ! /usr/bin/pgrep -x "Cursor" >/dev/null 2>&1; then
  /usr/bin/open -a "Cursor" "$repo" >/dev/null 2>&1 || true
fi
