#!/usr/bin/env bash
# Install the stop-gap SwiftBar menu-bar plugin.
# Idempotent: re-running just re-symlinks.
#
# Requires Homebrew for SwiftBar install, unless SwiftBar is already present.

set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "==> menu-bar: install"

# 1. Ensure SwiftBar is installed.
if [[ ! -d /Applications/SwiftBar.app ]]; then
  if ! command -v brew >/dev/null; then
    cat >&2 <<EOF
SwiftBar is required for the menu-bar plugin, and Homebrew isn't installed.
Install SwiftBar manually from https://swiftbar.app/ and re-run this script.
EOF
    exit 1
  fi
  echo "--> installing SwiftBar via Homebrew"
  brew install --cask swiftbar
fi

# 2. Ensure SwiftBar's plugin dir exists.
PLUGIN_DIR="$HOME/Library/Application Support/SwiftBar/Plugins"
mkdir -p "$PLUGIN_DIR"

# 3. Symlink the plugin. SwiftBar reads the refresh interval from the filename:
#    alvum.60s.sh = run every 60 seconds.
chmod +x "$ALVUM_REPO/scripts/menu-bar.sh"
ln -sf "$ALVUM_REPO/scripts/menu-bar.sh" "$PLUGIN_DIR/alvum.60s.sh"
echo "    symlinked $ALVUM_REPO/scripts/menu-bar.sh -> $PLUGIN_DIR/alvum.60s.sh"

# 4. Start (or nudge) SwiftBar.
if ! pgrep -x SwiftBar >/dev/null; then
  open /Applications/SwiftBar.app
  echo "--> started SwiftBar"
  echo "    if it prompts for a plugin folder, choose: $PLUGIN_DIR"
else
  # Poke SwiftBar to rescan the plugin dir.
  osascript -e 'tell application "SwiftBar" to refresh all' 2>/dev/null || true
fi

echo
echo "menu-bar installed. look for the alvum dot in your menu bar (top right)."
echo "to remove:  $ALVUM_REPO/scripts/menu-bar-uninstall.sh"
