#!/usr/bin/env bash
# Email today's briefing to the address in ~/.alvum/runtime/email.txt.
# Idempotent — re-running re-sends the same day's briefing.

set -euo pipefail
source "$(dirname "$0")/lib.sh"

if [[ ! -f "$ALVUM_EMAIL_FILE" ]]; then
  echo "no email configured at $ALVUM_EMAIL_FILE" >&2
  echo "set one:  echo you@example.com > $ALVUM_EMAIL_FILE" >&2
  exit 1
fi
RECIPIENT=$(tr -d '[:space:]' < "$ALVUM_EMAIL_FILE")

TODAY=$(today)
BRIEFING="$ALVUM_BRIEFINGS_DIR/$TODAY/briefing.md"
if [[ ! -f "$BRIEFING" ]]; then
  echo "no briefing for $TODAY. run briefing.sh first." >&2
  exit 1
fi

SUBJECT="alvum · $(date +"%A, %B %-d")"
mail -s "$SUBJECT" "$RECIPIENT" < "$BRIEFING"
echo "[$(now_utc)] emailed briefing to $RECIPIENT"
