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

# 4. Pre-configure SwiftBar so its first-launch folder-picker dialog does NOT appear.
#    SwiftBar writes BOTH of these keys when the user picks a folder; setting both
#    imperatively plus FirstLaunch=false makes the dialog think setup is already done.
#    Discovered empirically 2026-04-18 by diffing defaults pre/post dialog completion.
echo "--> pre-configuring SwiftBar preferences (skip first-launch folder picker)"
pkill -x SwiftBar 2>/dev/null || true
sleep 1
defaults write com.ameba.SwiftBar pluginDirectoryPath -string "$PLUGIN_DIR"
defaults write com.ameba.SwiftBar PluginDirectory      -string "$PLUGIN_DIR"
defaults write com.ameba.SwiftBar FirstLaunch -bool false
# Turn off update-check prompts too — first-run noise we don't want.
defaults write com.ameba.SwiftBar SUEnableAutomaticChecks -bool false
defaults write com.ameba.SwiftBar SUHasLaunchedBefore    -bool true

# 5. Launch SwiftBar headless-ish (no foreground activation).
open -g /Applications/SwiftBar.app
sleep 2

# Nudge SwiftBar to rescan via its URL scheme. (AppleScript 'tell ... to refresh'
# isn't in SwiftBar's scripting dictionary and throws a syntax error.)
open -g 'swiftbar://refreshallplugins' 2>/dev/null || true

echo
echo "menu-bar installed. look for the alvum dot in your menu bar (top right)."
echo "to remove:  $ALVUM_REPO/scripts/menu-bar-uninstall.sh"
