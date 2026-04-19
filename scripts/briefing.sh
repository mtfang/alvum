#!/usr/bin/env bash
# Generate today's briefing using whatever connectors are enabled in config.
# If ~/.alvum/runtime/email.txt exists, email it afterward.

set -euo pipefail
source "$(dirname "$0")/lib.sh"
ensure_dirs

TODAY=$(today)
YESTERDAY=$(yesterday)
OUT_DIR="$ALVUM_BRIEFINGS_DIR/$TODAY"
mkdir -p "$OUT_DIR"

# Scope the Claude Code connector to the last 24h.
SINCE_ISO=$(date -j -f "%Y-%m-%d %H:%M:%S" "$YESTERDAY 00:00:00" -u +"%Y-%m-%dT%H:%M:%SZ")
"$ALVUM_BIN" config-set "connectors.claude-code.since" "$SINCE_ISO" >/dev/null

# Capture dir: yesterday's data if it exists (screen daemon writes there);
# otherwise an empty directory (alvum-cli requires the flag even for claude-only).
CAPTURE_DIR="$ALVUM_CAPTURE/$YESTERDAY"
mkdir -p "$CAPTURE_DIR"

echo "[$(now_utc)] briefing start (since=$SINCE_ISO)"

"$ALVUM_BIN" extract \
  --capture-dir "$CAPTURE_DIR" \
  --output "$OUT_DIR" \
  --provider cli \
  --model claude-sonnet-4-6

echo "[$(now_utc)] briefing done -> $OUT_DIR/briefing.md"

# Email, if configured. Isolated to the email script — do not inline here.
if [[ -f "$ALVUM_EMAIL_FILE" ]]; then
  "$(dirname "$0")/email.sh"
fi
