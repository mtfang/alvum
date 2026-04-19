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

# Returns "true"/"false" for a given source.
# Sources: audio-mic, audio-system, screen, claude-code.
is_source_enabled() {
  local src="$1"
  local section
  case "$src" in
    claude-code) section="connectors.claude-code" ;;
    *)           section="capture.$src" ;;
  esac
  local out
  out=$("$ALVUM_BIN" config-show 2>/dev/null \
    | awk -v sect="[$section]" '
        $0 == sect { in_s=1; next }
        in_s && /^\[/ { in_s=0 }
        in_s && /^enabled[[:space:]]*=/ { gsub(/[[:space:]]/, ""); split($0, a, "="); print a[2]; exit }
      ')
  echo "${out:-false}"
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
