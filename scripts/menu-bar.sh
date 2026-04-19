#!/usr/bin/env bash
#
# ============================================================================
# STOP-GAP: SwiftBar menu-bar plugin for alvum.
# Delete this file when alvum-app ships its own NSStatusItem.
# Keep under 150 lines. No settings panels, no config editors — use CLI.
# ============================================================================
#
# <xbar.title>alvum</xbar.title>
# <xbar.version>0.1</xbar.version>
# <xbar.author>alvum</xbar.author>
# <xbar.desc>Status + quick actions for the alvum daily briefing.</xbar.desc>
# <swiftbar.hideRunInTerminal>true</swiftbar.hideRunInTerminal>
# <swiftbar.hideLastUpdated>true</swiftbar.hideLastUpdated>
# <swiftbar.hideAbout>true</swiftbar.hideAbout>

set -euo pipefail

# SwiftBar symlinks this script into its plugins dir; resolve the real path
# so we can source lib.sh and reach the other scripts.
REAL="$(readlink "$0" 2>/dev/null || echo "$0")"
SCRIPTS="$(cd "$(dirname "$REAL")" && pwd)"
source "$SCRIPTS/lib.sh"

# ---------- menu bar title (the single line visible on screen) ----------

if briefing_fresh_today; then
  if any_capture_stale; then
    echo "◐ alvum | color=orange size=13"
  else
    echo "● alvum | color=#4a9d3d size=13"
  fi
else
  echo "○ alvum | color=gray size=13"
fi

echo "---"

# ---------- briefing actions ----------

last=$(last_briefing_relative)
echo "Today's briefing ($last) | bash='$SCRIPTS/view.sh' terminal=false"
echo "Run briefing now | bash='$SCRIPTS/briefing.sh' terminal=true refresh=true"

echo "---"

# ---------- capture state + per-source toggles ----------

echo "Capture: $(capture_state) | disabled=true"

for src in claude-code codex audio-mic audio-system screen; do
  on=$(is_source_enabled "$src")
  marker=$([[ "$on" == "true" ]] && echo "●" || echo "○")
  echo "$marker  $src | bash='$SCRIPTS/capture.sh' param1=toggle param2=$src terminal=false refresh=true"
done

echo "---"

if plist_loaded "$ALVUM_CAPTURE_LABEL"; then
  echo "Stop capture | bash='$SCRIPTS/capture.sh' param1=stop terminal=false refresh=true"
else
  echo "Start capture | bash='$SCRIPTS/capture.sh' param1=start terminal=false refresh=true"
fi

echo "---"

# ---------- diagnostics + file access ----------

echo "Status (verbose) | bash='$SCRIPTS/status.sh' terminal=true"
echo "Edit config | bash=/usr/bin/open param1='$ALVUM_CONFIG_FILE' terminal=false"
echo "Open briefings folder | bash=/usr/bin/open param1='$ALVUM_BRIEFINGS_DIR' terminal=false"
