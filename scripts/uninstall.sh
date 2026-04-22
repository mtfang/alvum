#!/usr/bin/env bash
# Clean reverse of install. Leaves ~/.alvum/generated/briefings/ alone unless --purge.
# Usage: uninstall.sh [--purge]

set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "==> uninstall"

# Remove the menu-bar plugin (if installed — silent if not).
"$(dirname "$0")/menu-bar-uninstall.sh" >/dev/null 2>&1 || true

# Stop capture first (if running).
if alvum_app_running; then
  "$(dirname "$0")/capture.sh" stop
fi

# Remove the briefing schedule.
unload_plist "$ALVUM_LAUNCHAGENTS/$ALVUM_BRIEFING_LABEL.plist"

if [[ "${1:-}" == "--purge" ]]; then
  rm -rf "$ALVUM_ROOT"
  echo "purged ~/.alvum (config, briefings, capture, logs all under one root now)"
fi
echo "done."
