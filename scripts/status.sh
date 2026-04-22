#!/usr/bin/env bash
# Concise health report for this Mac's alvum install.

set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "alvum — $(date +'%Y-%m-%d %H:%M:%S')"
echo

# Binary.
if [[ -x "$ALVUM_BIN" ]]; then
  echo "  binary:     $ALVUM_BIN"
else
  echo "  binary:     MISSING ($ALVUM_BIN)"
fi

# Config.
if [[ -f "$ALVUM_CONFIG_FILE" ]]; then
  echo "  config:     $ALVUM_CONFIG_FILE"
else
  echo "  config:     MISSING"
fi

# Launchd jobs.
plist_loaded "$ALVUM_BRIEFING_LABEL" \
  && echo "  briefing:   scheduled" \
  || echo "  briefing:   not scheduled"
alvum_app_running \
  && echo "  capture:    running (Alvum.app)" \
  || echo "  capture:    off"

# Today & yesterday briefings.
for d in "$(today)" "$(yesterday)"; do
  B="$ALVUM_BRIEFINGS_DIR/$d/briefing.md"
  if [[ -f "$B" ]]; then
    printf "  briefing %s: %d lines · modified %s\n" "$d" \
      "$(wc -l < "$B" | tr -d ' ')" "$(stat -f %Sm "$B")"
  else
    printf "  briefing %s: MISSING\n" "$d"
  fi
done

# Email config.
if [[ -f "$ALVUM_EMAIL_FILE" ]]; then
  echo "  email to:   $(tr -d '[:space:]' < "$ALVUM_EMAIL_FILE")"
else
  echo "  email to:   not configured"
fi

# Recent errors.
for lf in briefing.err capture.err; do
  path="$ALVUM_LOGS_DIR/$lf"
  if [[ -s "$path" ]]; then
    echo
    echo "  recent $lf (tail):"
    tail -5 "$path" | sed 's/^/    /'
  fi
done
