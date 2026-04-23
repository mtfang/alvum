#!/usr/bin/env bash
# One-time setup: build binary, write config, schedule daily briefing.
# Re-run: safe. Overwrites binary, config, and plist.

set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "==> install"

# 1. Dependencies.
command -v cargo  >/dev/null || { echo "cargo not found; install Rust first"; exit 1; }
command -v claude >/dev/null || { echo "claude CLI not found; install from claude.com/download"; exit 1; }

ensure_dirs

# 1b. Fetch Whisper model for the audio connector. Skipped if ALVUM_SKIP_WHISPER=1.
if [[ "${ALVUM_SKIP_WHISPER:-}" != "1" ]]; then
  echo "--> provisioning Whisper model"
  "$ALVUM_REPO/scripts/download-whisper-model.sh"
fi

# 2. Build and install the release binary into the .app bundle.
echo "--> building alvum"
(cd "$ALVUM_REPO" && cargo build --release -p alvum-cli)
install -m 755 "$ALVUM_REPO/target/release/alvum" "$ALVUM_BIN"
install -m 644 "$ALVUM_REPO/crates/alvum-cli/Info.plist" "$ALVUM_APP_PLIST"
echo "    $ALVUM_APP_DIR"

# 2b. Sign with a persistent self-signed cert so macOS TCC keys permissions
# on a stable identity (not the per-build binary content hash). First install
# generates the 'alvum-dev' cert in the login keychain; subsequent installs
# reuse it. Without this, every 'cargo build' re-prompts for Mic / Screen
# Recording / Accessibility.
"$ALVUM_REPO/scripts/sign-binary.sh"

# 3. Write a minimal default config.
echo "--> writing config"
cat > "$ALVUM_CONFIG_FILE" <<EOF
# Minimal default: Claude Code only. Enable more capabilities with capture.sh / email.
[pipeline]
provider = "cli"
model = "claude-sonnet-4-6"
output_dir = "$ALVUM_BRIEFINGS_DIR"

[connectors.claude-code]
enabled = true
session_dir = "$HOME/.claude/projects"
# 'since' is overridden per-run by briefing.sh to scope to the last 24h.

[connectors.codex]
enabled = true
session_dir = "$HOME/.codex"
# 'since' is overridden per-run by briefing.sh.

[connectors.screen]
enabled = true
vision = "ocr"

[connectors.audio]
enabled = true
# Path to the ggml Whisper model. Downloaded by scripts/download-whisper-model.sh.
whisper_model = "$ALVUM_MODELS_DIR/ggml-base.en.bin"

[capture.audio-mic]
enabled = false
# Silence gate: keep a 60 s chunk if EITHER its RMS ≥ silence_rms_dbfs OR
# its peak ≥ silence_peak_dbfs. RMS catches sustained speech, peak catches
# transients (claps, keystrokes) that average out. Defaults suit the
# MacBook built-in mic in a quiet room; loosen if quiet chunks get dropped.
# silence_gate = false   # set to false / "off" to write every chunk
# silence_rms_dbfs  = -45
# silence_peak_dbfs = -15

[capture.audio-system]
enabled = false
# System audio has no ambient floor, so default RMS threshold sits lower.
# silence_rms_dbfs  = -60
# silence_peak_dbfs = -15
# Per-app filter for system audio. Two modes, mutually exclusive:
#
#   1. Blacklist (default) — capture everything EXCEPT listed apps.
#        exclude_apps       = ["Music", "Spotify"]
#        exclude_bundle_ids = ["com.apple.Music"]
#
#   2. Whitelist — capture ONLY listed apps.
#        include_apps       = ["Zoom", "Safari"]
#        include_bundle_ids = ["us.zoom.xos"]
#
# Rules: applicationName match is case-insensitive, bundleIdentifier
# match is exact. Setting both include_* and exclude_* is a config error.
#
# Note: SCK uses a single content filter for BOTH audio and screen
# capture. Whichever apps the filter keeps out of the audio mix are
# also kept out of screenshots — keep that in mind when choosing rules.
[capture.screen]
enabled = false
# Fire an idle trigger every N seconds when no focus change has occurred.
idle_interval_secs = 30
# Minimum wall-clock gap (s) between two saved PNGs. Caps volume when an
# app has an animated title or rapid tab cycling. 0 disables.
min_interval_secs = 10
# Toggle which focus signals can fire a screenshot. Disable window_focus
# if terminals / chat apps with live titles dominate the capture.
app_focus = true
window_focus = true
EOF
echo "    $ALVUM_CONFIG_FILE"

# 4. Schedule the daily briefing.
echo "--> scheduling daily briefing (07:00 local)"
install_plist \
  "$ALVUM_REPO/launchd/$ALVUM_BRIEFING_LABEL.plist" \
  "$ALVUM_LAUNCHAGENTS/$ALVUM_BRIEFING_LABEL.plist"

# 5. Dry-run config to validate.
"$ALVUM_BIN" config-show >/dev/null

# 6. Optionally install the menu-bar plugin (STOP-GAP until full app ships).
if [[ "${ALVUM_SKIP_MENU_BAR:-}" != "1" ]]; then
  read -r -p "Install the menu-bar plugin (adds a status dot + quick actions)? [Y/n] " ans
  if [[ "$ans" != "n" && "$ans" != "N" ]]; then
    "$ALVUM_REPO/scripts/menu-bar-install.sh"
  fi
fi

echo
echo "installed."
echo
echo "next:"
echo "  $ALVUM_REPO/scripts/briefing.sh        # run a briefing right now"
echo "  $ALVUM_REPO/scripts/view.sh            # open today's briefing"
echo "  $ALVUM_REPO/scripts/capture.sh start   # enable capture daemon (opt-in)"
echo "  echo you@example.com > $ALVUM_EMAIL_FILE   # enable email delivery (opt-in)"
