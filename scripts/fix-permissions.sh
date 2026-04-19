#!/usr/bin/env bash
# Open the macOS System Settings pane for a specific capture source's
# permission requirement. After granting, run `capture.sh stop && start`
# to re-initialize the daemon with the fresh permission.

set -euo pipefail
source "$(dirname "$0")/lib.sh"

src="${1:-}"
if [[ -z "$src" ]]; then
  echo "usage: $0 { screen | audio-mic | audio-system | accessibility }" >&2
  exit 2
fi

if ! open_permissions_for "$src"; then
  echo "no known permission pane for '$src'" >&2
  exit 1
fi

echo "opened System Settings → Privacy & Security → relevant pane for '$src'."
echo "After toggling alvum on, run:  $ALVUM_REPO/scripts/capture.sh stop && $ALVUM_REPO/scripts/capture.sh start"
