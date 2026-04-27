#!/usr/bin/env bash
# Generate today's briefing using whatever connectors are enabled in config.
# If ~/.alvum/runtime/email.txt exists, email it afterward.

set -euo pipefail
source "$(dirname "$0")/lib.sh"
ensure_dirs

TODAY=$(today)
YESTERDAY=$(yesterday)

date_add_days() {
  date -j -v+"$2"d -f "%Y-%m-%d %H:%M:%S" "$1 00:00:00" "+%Y-%m-%d"
}

local_midnight_utc() {
  local epoch
  epoch=$(date -j -f "%Y-%m-%d %H:%M:%S" "$1 00:00:00" "+%s")
  TZ=UTC date -r "$epoch" "+%Y-%m-%dT%H:%M:%SZ"
}

capture_has_data() {
  [[ -d "$1" ]] && [[ -n "$(ls -A "$1" 2>/dev/null)" ]]
}

run_briefing_for_date() {
  local date="$1" capture_dir="$2" since_iso="$3" before_iso="$4"
  local out_dir="$ALVUM_BRIEFINGS_DIR/$date"
  mkdir -p "$out_dir" "$capture_dir"

  echo "[$(now_utc)] briefing start date=$date capture=$capture_dir since=$since_iso before=$before_iso"

  "$ALVUM_BIN" extract \
    --capture-dir "$capture_dir" \
    --output "$out_dir" \
    --provider auto \
    --since "$since_iso" \
    --before "$before_iso" \
    --briefing-date "$date" \
    --resume

  echo "[$(now_utc)] briefing done -> $out_dir/briefing.md"
}

# Catch up missing historical briefings first, oldest to newest. This keeps
# corpus learning and downstream daily artifacts monotonic instead of letting a
# later manual/launchd run leapfrog unprocessed capture days.
while IFS= read -r capture_dir; do
  date_name=$(basename "$capture_dir")
  [[ "$date_name" < "$TODAY" ]] || continue
  [[ -f "$ALVUM_BRIEFINGS_DIR/$date_name/briefing.md" ]] && continue
  capture_has_data "$capture_dir" || continue

  SINCE_ISO=$(local_midnight_utc "$date_name")
  BEFORE_ISO=$(local_midnight_utc "$(date_add_days "$date_name" 1)")
  run_briefing_for_date "$date_name" "$capture_dir" "$SINCE_ISO" "$BEFORE_ISO"
done < <(find "$ALVUM_CAPTURE" -maxdepth 1 -type d -name '20??-??-??' -print | sort)

# Current run: preserve the existing behavior. Prefer today's capture dir for
# mid-day runs; fall back to yesterday's data for the morning launchd run.
CAPTURE_DIR="$ALVUM_CAPTURE/$TODAY"
if ! capture_has_data "$CAPTURE_DIR"; then
  CAPTURE_DIR="$ALVUM_CAPTURE/$YESTERDAY"
fi

SINCE_ISO=$(date -u -v-24H +"%Y-%m-%dT%H:%M:%SZ")
BEFORE_ISO=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
run_briefing_for_date "$TODAY" "$CAPTURE_DIR" "$SINCE_ISO" "$BEFORE_ISO"

# Email, if configured. Isolated to the email script — do not inline here.
if [[ -f "$ALVUM_EMAIL_FILE" ]]; then
  "$(dirname "$0")/email.sh"
fi
