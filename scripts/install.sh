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

# 2. Build and install the release binary.
echo "--> building alvum"
(cd "$ALVUM_REPO" && cargo build --release -p alvum-cli)
install -m 755 "$ALVUM_REPO/target/release/alvum" "$ALVUM_BIN"
echo "    $ALVUM_BIN"

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
[capture.audio-system]
enabled = false
[capture.screen]
enabled = false
idle_interval_secs = 30
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
