#!/usr/bin/env bash
# Remove the menu-bar plugin symlink. Does NOT uninstall SwiftBar itself.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

rm -f "$HOME/Library/Application Support/SwiftBar/Plugins/alvum.60s.sh"
osascript -e 'tell application "SwiftBar" to refresh all' 2>/dev/null || true
echo "menu-bar plugin removed. SwiftBar itself left installed."
