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

json_escape() {
  printf '%s' "$1" | perl -0pe 's/\\/\\\\/g; s/"/\\"/g; s/\r/\\r/g; s/\n/\\n/g; s/\t/\\t/g'
}

make_run_id() {
  printf '%s-%s' "$(date -u +%Y%m%d%H%M%S)" "$$"
}

write_run_status() {
  local status_file="$1" status="$2" date="$3" run_id="$4" run_dir="$5" label="$6" started_at="$7" duration_ms="${8:-0}" reason="${9:-}" code="${10:-null}"
  local updated_at completed_line reason_line code_value
  updated_at=$(now_utc)
  completed_line=""
  [[ "$status" != "running" ]] && completed_line=",
  \"completed_at\": \"$(json_escape "$updated_at")\""
  reason_line=""
  [[ -n "$reason" ]] && reason_line=",\"reason\":\"$(json_escape "$reason")\""
  code_value="$code"
  [[ "$code_value" =~ ^[0-9]+$ ]] || code_value=null
  cat > "$status_file" <<JSON
{
  "status": "$(json_escape "$status")",
  "run_id": "$(json_escape "$run_id")",
  "date": "$(json_escape "$date")",
  "label": "$(json_escape "$label")",
  "run_dir": "$(json_escape "$run_dir")",
  "started_at": "$(json_escape "$started_at")",
  "updated_at": "$(json_escape "$updated_at")"$completed_line,
  "duration_ms": $duration_ms,
  "code": $code_value$reason_line
}
JSON
}

emit_run_marker() {
  local event="$1" date="$2" run_id="$3" run_dir="$4" label="$5" progress_file="$6" events_file="$7" stdout_log="$8" stderr_log="$9" status_file="${10}" expected_briefing="${11}" started_at="${12}" duration_ms="${13:-0}" code="${14:-null}" reason="${15:-}"
  local code_value reason_line
  code_value="$code"
  [[ "$code_value" =~ ^[0-9]+$ ]] || code_value=null
  reason_line=""
  [[ -n "$reason" ]] && reason_line=",\"reason\":\"$(json_escape "$reason")\""
  printf '[alvum-run] {"event":"%s","date":"%s","run_id":"%s","run_dir":"%s","label":"%s","progress_file":"%s","events_file":"%s","stdout_log":"%s","stderr_log":"%s","status_file":"%s","expected_briefing":"%s","started_at":"%s","duration_ms":%s,"code":%s%s}\n' \
    "$(json_escape "$event")" \
    "$(json_escape "$date")" \
    "$(json_escape "$run_id")" \
    "$(json_escape "$run_dir")" \
    "$(json_escape "$label")" \
    "$(json_escape "$progress_file")" \
    "$(json_escape "$events_file")" \
    "$(json_escape "$stdout_log")" \
    "$(json_escape "$stderr_log")" \
    "$(json_escape "$status_file")" \
    "$(json_escape "$expected_briefing")" \
    "$(json_escape "$started_at")" \
    "$duration_ms" \
    "$code_value" \
    "$reason_line"
}

write_failure_marker() {
  local date="$1" out_dir="$2" run_id="$3" run_dir="$4" reason="$5" code="$6" stderr_log="$7"
  local stderr_tail
  stderr_tail=$(tail -c 24576 "$stderr_log" 2>/dev/null || true)
  cat > "$out_dir/briefing.failed.json" <<JSON
{
  "date": "$(json_escape "$date")",
  "reason": "$(json_escape "$reason")",
  "failedAt": "$(now_utc)",
  "run_id": "$(json_escape "$run_id")",
  "run_dir": "$(json_escape "$run_dir")",
  "code": $code,
  "stderr_tail": "$(json_escape "$stderr_tail")"
}
JSON
}

run_briefing_for_date() {
  local date="$1" capture_dir="$2" since_iso="$3" before_iso="$4"
  local out_dir="$ALVUM_BRIEFINGS_DIR/$date"
  local run_id run_dir progress_file events_file stdout_log stderr_log status_file label started_at started_epoch code duration_ms reason
  run_id=$(make_run_id)
  run_dir="$out_dir/runs/$run_id"
  progress_file="$run_dir/progress.jsonl"
  events_file="$run_dir/events.jsonl"
  stdout_log="$run_dir/stdout.log"
  stderr_log="$run_dir/stderr.log"
  status_file="$run_dir/status.json"
  label="Briefing $date"
  started_at=$(now_utc)
  started_epoch=$(date +%s)
  mkdir -p "$out_dir" "$capture_dir" "$run_dir"
  : > "$progress_file"
  : > "$events_file"
  : > "$stdout_log"
  : > "$stderr_log"
  write_run_status "$status_file" "running" "$date" "$run_id" "$run_dir" "$label" "$started_at"

  emit_run_marker "start" "$date" "$run_id" "$run_dir" "$label" "$progress_file" "$events_file" "$stdout_log" "$stderr_log" "$status_file" "$out_dir/briefing.md" "$started_at"
  echo "[$(now_utc)] briefing start date=$date capture=$capture_dir since=$since_iso before=$before_iso run=$run_id" | tee -a "$stdout_log"

  if ALVUM_PROGRESS_FILE="$progress_file" ALVUM_PIPELINE_EVENTS_FILE="$events_file" "$ALVUM_BIN" extract \
    --capture-dir "$capture_dir" \
    --output "$out_dir" \
    --since "$since_iso" \
    --before "$before_iso" \
    --briefing-date "$date" \
    --resume \
    > >(tee -a "$stdout_log") \
    2> >(tee -a "$stderr_log" >&2); then
    code=0
  else
    code=$?
  fi
  duration_ms=$(( ($(date +%s) - started_epoch) * 1000 ))

  if [[ "$code" -eq 0 && -f "$out_dir/briefing.md" ]]; then
    rm -f "$out_dir/briefing.failed.json"
    write_run_status "$status_file" "success" "$date" "$run_id" "$run_dir" "$label" "$started_at" "$duration_ms" "" "$code"
    echo "[$(now_utc)] briefing done -> $out_dir/briefing.md run=$run_id" | tee -a "$stdout_log"
    emit_run_marker "finish" "$date" "$run_id" "$run_dir" "$label" "$progress_file" "$events_file" "$stdout_log" "$stderr_log" "$status_file" "$out_dir/briefing.md" "$started_at" "$duration_ms" "$code"
    return 0
  fi

  if [[ "$code" -eq 0 ]]; then
    reason="no briefing generated"
    code=1
  else
    reason="code $code"
  fi
  write_run_status "$status_file" "failed" "$date" "$run_id" "$run_dir" "$label" "$started_at" "$duration_ms" "$reason" "$code"
  write_failure_marker "$date" "$out_dir" "$run_id" "$run_dir" "$reason" "$code" "$stderr_log"
  echo "[$(now_utc)] briefing failed date=$date reason=$reason run=$run_id" | tee -a "$stdout_log"
  emit_run_marker "finish" "$date" "$run_id" "$run_dir" "$label" "$progress_file" "$events_file" "$stdout_log" "$stderr_log" "$status_file" "$out_dir/briefing.md" "$started_at" "$duration_ms" "$code" "$reason"
  return "$code"
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
