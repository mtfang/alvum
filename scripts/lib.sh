#!/usr/bin/env bash
# Shared path + env conventions. Source, don't execute.
# Layout authority: docs/superpowers/specs/2026-04-18-storage-layout.md

set -euo pipefail

# Single root — override for testing; production is always $HOME/.alvum.
export ALVUM_ROOT="${ALVUM_ROOT:-$HOME/.alvum}"

# Three lifecycle buckets.
export ALVUM_CAPTURE="$ALVUM_ROOT/capture"
export ALVUM_GENERATED="$ALVUM_ROOT/generated"
export ALVUM_RUNTIME="$ALVUM_ROOT/runtime"

# Binaries + config + small state — all under runtime/.
export ALVUM_BIN="${ALVUM_BIN:-$ALVUM_RUNTIME/bin/alvum}"
export ALVUM_CONFIG_FILE="$ALVUM_RUNTIME/config.toml"
export ALVUM_EMAIL_FILE="$ALVUM_RUNTIME/email.txt"
export ALVUM_LOGS_DIR="$ALVUM_RUNTIME/logs"

# Generated data — where briefings land.
export ALVUM_BRIEFINGS_DIR="$ALVUM_GENERATED/briefings"

export ALVUM_MODELS_DIR="$ALVUM_RUNTIME/models"

export ALVUM_LAUNCHAGENTS="$HOME/Library/LaunchAgents"
export ALVUM_BRIEFING_LABEL="com.alvum.briefing"
export ALVUM_CAPTURE_LABEL="com.alvum.capture"

export ALVUM_REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

today()      { date +%Y-%m-%d; }
yesterday()  { date -v-1d +%Y-%m-%d; }
now_utc()    { date -u +%Y-%m-%dT%H:%M:%SZ; }

ensure_dirs() {
  mkdir -p "$ALVUM_RUNTIME/bin" "$ALVUM_RUNTIME/logs" \
           "$ALVUM_CAPTURE" "$ALVUM_BRIEFINGS_DIR" \
           "$ALVUM_MODELS_DIR" \
           "$ALVUM_LAUNCHAGENTS"
}

# Install a templated launchd plist. Arguments: <src plist> <dst plist>.
# Any @@VAR@@ in the plist is replaced with env("VAR") if set.
install_plist() {
  local src="$1" dst="$2"
  sed -e "s|@@ALVUM_ROOT@@|$ALVUM_ROOT|g" \
      -e "s|@@ALVUM_RUNTIME@@|$ALVUM_RUNTIME|g" \
      -e "s|@@ALVUM_BIN@@|$ALVUM_BIN|g" \
      -e "s|@@ALVUM_REPO@@|$ALVUM_REPO|g" \
      "$src" > "$dst"
  launchctl bootout "gui/$UID" "$dst" 2>/dev/null || true
  launchctl bootstrap "gui/$UID" "$dst"
}

unload_plist() {
  local dst="$1"
  launchctl bootout "gui/$UID" "$dst" 2>/dev/null || true
  rm -f "$dst"
}

plist_loaded() {
  local label="$1"
  launchctl list | awk -v l="$label" '$3 == l { found=1 } END { exit !found }'
}

# ---- derived-state helpers (used by status.sh, doctor.sh, and menu-bar.sh) ----

# 0 (true) if today's briefing.md exists.
briefing_fresh_today() {
  [[ -f "$ALVUM_BRIEFINGS_DIR/$(today)/briefing.md" ]]
}

# 0 (true) if capture daemon is loaded but hasn't written anything in > 2h.
# 1 (false) if capture daemon is off (nothing to be stale).
any_capture_stale() {
  plist_loaded "$ALVUM_CAPTURE_LABEL" || return 1
  local today_dir="$ALVUM_CAPTURE/$(today)"
  [[ ! -d "$today_dir" ]] && return 0
  local newest
  newest=$(find "$today_dir" -type f -print0 2>/dev/null \
    | xargs -0 stat -f "%m" 2>/dev/null \
    | sort -rn | head -1)
  [[ -z "$newest" ]] && return 0
  local age=$(( $(date +%s) - newest ))
  (( age > 7200 ))
}

# Read a single `enabled = <bool>` field from a named TOML section.
# `section` is "connectors.audio", "capture.screen", etc.
_read_enabled_flag() {
  local section="$1"
  local out
  out=$("$ALVUM_BIN" config-show 2>/dev/null \
    | awk -v sect="[$section]" '
        $0 == sect { in_s=1; next }
        in_s && /^\[/ { in_s=0 }
        in_s && /^enabled[[:space:]]*=/ { gsub(/[[:space:]]/, ""); split($0, a, "="); print a[2]; exit }
      ')
  echo "${out:-false}"
}

# Returns "true"/"false" for a user-facing source. TRUE only when every
# underlying flag that gates the source is true. Consumed by menu-bar.sh,
# capture.sh status, etc.
#
# User-facing source  -> underlying flags (all must be true)
#   claude-code       -> connectors.claude-code
#   codex             -> connectors.codex
#   screen            -> capture.screen AND connectors.screen
#   audio-mic         -> capture.audio-mic AND connectors.audio  (shared)
#   audio-system      -> capture.audio-system AND connectors.audio  (shared)
is_source_enabled() {
  local src="$1"
  case "$src" in
    claude-code)
      _read_enabled_flag "connectors.claude-code"
      ;;
    codex)
      _read_enabled_flag "connectors.codex"
      ;;
    screen)
      local cap conn
      cap=$(_read_enabled_flag "capture.screen")
      conn=$(_read_enabled_flag "connectors.screen")
      if [[ "$cap" == "true" && "$conn" == "true" ]]; then echo "true"; else echo "false"; fi
      ;;
    audio-mic|audio-system)
      local cap conn
      cap=$(_read_enabled_flag "capture.$src")
      conn=$(_read_enabled_flag "connectors.audio")
      if [[ "$cap" == "true" && "$conn" == "true" ]]; then echo "true"; else echo "false"; fi
      ;;
    *)
      echo "false"
      ;;
  esac
}

# Human-readable age of the newest briefing, or "never" if none exists.
last_briefing_relative() {
  local f
  for d in "$(today)" "$(yesterday)"; do
    f="$ALVUM_BRIEFINGS_DIR/$d/briefing.md"
    [[ -f "$f" ]] && break
    f=""
  done
  [[ -z "$f" ]] && { echo "never"; return; }
  local age=$(( $(date +%s) - $(stat -f "%m" "$f") ))
  if   (( age <  3600 )); then echo "$(( age / 60 ))m ago"
  elif (( age < 86400 )); then echo "$(( age / 3600 ))h ago"
  else                         echo "$(( age / 86400 ))d ago"
  fi
}

# "running" | "stopped"
capture_state() {
  if plist_loaded "$ALVUM_CAPTURE_LABEL"; then echo "running"
  else                                          echo "stopped"
  fi
}

# Detect macOS permission denials for a given source by scanning the daemon's
# recent stdout (the daemon emits a clear ERROR line when a source fails to
# init for lack of permission; stderr stays empty — a quirk of the current
# capture logger). Returns "" if no issue detected; returns a short reason
# string (e.g., "Screen Recording") otherwise.
#
# Only considers errors that appear AFTER the most recent "created capture
# source" INFO line for this source — so stale errors from earlier daemon
# runs don't trigger false positives.
detect_permission_issue() {
  local src="$1"
  local log="$ALVUM_LOGS_DIR/capture.out"
  [[ ! -f "$log" ]] && return 0

  # Daemon writes ANSI color codes; strip before matching so the regex doesn't
  # have to deal with '[3msource[0m[2m=[0m"screen"'.
  local stripped
  stripped=$(sed -E $'s/\x1b\\[[0-9;]*m//g' "$log")

  # Use `grep || true` throughout so the non-match case (no issue found)
  # doesn't trip `set -euo pipefail` in the caller.

  local start
  start=$(echo "$stripped" | grep -n "created capture source.*source=\"$src\"" | tail -1 | cut -d: -f1 || true)
  [[ -z "$start" ]] && return 0

  local err_line
  err_line=$(echo "$stripped" \
    | tail -n "+$start" \
    | grep -E "capture source failed.*source=\"$src\".*permission not granted" \
    | tail -1 || true)
  [[ -z "$err_line" ]] && return 0

  echo "$err_line" \
    | grep -oE '[A-Z][a-zA-Z ]+permission not granted' \
    | sed 's/ permission not granted//' \
    | head -1 || true
}

# Open macOS System Settings directly to the Privacy & Security pane most
# relevant to a source. No-op if we don't know the right URL for that source.
open_permissions_for() {
  local src="$1"
  local url=""
  case "$src" in
    screen)      url="x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture" ;;
    audio-mic|audio-system)
                 url="x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone" ;;
    accessibility)
                 url="x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility" ;;
    *)           return 1 ;;
  esac
  open "$url"
}
