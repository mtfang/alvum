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

# Scope the Claude Code connector to the last 24h. BSD date's `-j -f ... -u +...`
# chain silently ignores the format when `-u` is placed between; using `-v-24H`
# is simpler and always produces ISO 8601 UTC.
SINCE_ISO=$(date -u -v-24H +"%Y-%m-%dT%H:%M:%SZ")
"$ALVUM_BIN" config-set "connectors.claude-code.since" "$SINCE_ISO" >/dev/null

# Capture dir: prefer today's if it has data (mid-day runs + live capture),
# else yesterday's (overnight cron at 07:00 after yesterday's day completed).
# Falls back to today's (possibly empty) if neither has anything.
CAPTURE_DIR="$ALVUM_CAPTURE/$TODAY"
if [[ ! -d "$CAPTURE_DIR" ]] || [[ -z "$(ls -A "$CAPTURE_DIR" 2>/dev/null)" ]]; then
  CAPTURE_DIR="$ALVUM_CAPTURE/$YESTERDAY"
fi
mkdir -p "$CAPTURE_DIR"

echo "[$(now_utc)] briefing start (since=$SINCE_ISO)"

"$ALVUM_BIN" extract \
  --capture-dir "$CAPTURE_DIR" \
  --output "$OUT_DIR" \
  --provider cli \
  --model claude-sonnet-4-6 \
  --resume

echo "[$(now_utc)] briefing done -> $OUT_DIR/briefing.md"

# Email, if configured. Isolated to the email script — do not inline here.
if [[ -f "$ALVUM_EMAIL_FILE" ]]; then
  "$(dirname "$0")/email.sh"
fi
