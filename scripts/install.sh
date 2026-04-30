#!/usr/bin/env bash
# One-time CLI/dev setup: build binary and write privacy-first config.
# Re-run: safe. Overwrites binary and config; Electron owns scheduling.

set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "==> install"

# 1. Dependencies.
command -v cargo  >/dev/null || { echo "cargo not found; install Rust first"; exit 1; }

ensure_dirs

# 1b. Optional Whisper provisioning for local audio processing. The Electron
# onboarding flow owns the normal one-click install path; CLI/dev installs only
# fetch it when explicitly requested.
if [[ "${ALVUM_INSTALL_WHISPER:-}" == "1" ]]; then
  echo "--> provisioning Whisper model"
  "$ALVUM_REPO/scripts/download-whisper-model.sh"
fi

# 2. Build and install the release binary into the .app bundle.
echo "--> building alvum"
(cd "$ALVUM_REPO" && cargo build --release -p alvum-cli)
install -m 755 "$ALVUM_REPO/target/release/alvum" "$ALVUM_BIN"
install -m 644 "$ALVUM_REPO/crates/alvum-cli/Info.plist" "$ALVUM_APP_PLIST"
echo "    $ALVUM_APP_DIR"

# 2b. Sign with a persistent identity so macOS TCC keys permissions on a
# stable certificate (not the per-build binary content hash). Developer ID
# is preferred when installed; otherwise the first install generates the
# local 'alvum-dev' cert in the login keychain.
"$ALVUM_REPO/scripts/sign-binary.sh"

# 3. Write a minimal default config.
echo "--> writing config"
cat > "$ALVUM_CONFIG_FILE" <<EOF
# Privacy-first default: session connectors and managed providers are available;
# capture sources and scheduled synthesis stay opt-in.
[pipeline]
provider = "auto"
model = "claude-sonnet-4-6"
output_dir = "$ALVUM_BRIEFINGS_DIR"

[providers.claude-cli]
enabled = true

[providers.codex-cli]
enabled = true

[providers.anthropic-api]
enabled = true

[providers.bedrock]
enabled = true

[providers.ollama]
enabled = true

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

[connectors.audio]
enabled = true

[processors.screen]
mode = "ocr"

[processors.audio]
mode = "local"
whisper_model = "$ALVUM_MODELS_DIR/ggml-base.en.bin"
whisper_language = "en"

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

[scheduler.synthesis]
enabled = false
time = "07:00"
policy = "completed_days"
setup_completed = false
last_auto_run_date = ""
EOF
echo "    $ALVUM_CONFIG_FILE"

# 4. Scheduling is enabled by Electron after provider setup and the first
# successful manual synthesis. Clear any old installer-owned schedule.
echo "--> scheduler disabled until first successful synthesis"
unload_plist "$ALVUM_LAUNCHAGENTS/$ALVUM_BRIEFING_LABEL.plist" >/dev/null 2>&1 || true

# 5. Dry-run config to validate.
"$ALVUM_BIN" config-show >/dev/null

# 6. Legacy SwiftBar integration remains available for dev users who ask for it.
if [[ "${ALVUM_INSTALL_SWIFTBAR:-}" == "1" ]]; then
  "$ALVUM_REPO/scripts/menu-bar-install.sh"
fi

echo
echo "installed."
echo
echo "next:"
echo "  $ALVUM_REPO/scripts/briefing.sh        # run synthesis manually (CLI fallback)"
echo "  $ALVUM_REPO/scripts/view.sh            # open today's briefing"
echo "  $ALVUM_REPO/scripts/capture.sh start   # enable capture daemon (opt-in)"
echo "  echo you@example.com > $ALVUM_EMAIL_FILE   # enable email delivery (opt-in)"
