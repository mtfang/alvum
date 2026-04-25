#!/usr/bin/env bash
# Control the capture lifecycle (Electron shell) and per-source config flags.
#
# Subcommands:
#   start                        — launch Alvum.app (via LaunchServices)
#   stop                         — quit Alvum.app (graceful SIGTERM)
#   status                       — print shell + per-source state
#   toggle <source>              — flip a source on/off; restart Alvum.app to reload
#                                  (source ∈ audio-mic | audio-system | screen | claude-code | codex)
#
# The capture daemon used to be a launchd user agent. macOS TCC permission
# dialogs don't render for launchd-spawned headless processes, so capture
# now runs under an Electron app bundle that presents the prompts and
# spawns the Rust binary as a subprocess inheriting grants.

set -euo pipefail
source "$(dirname "$0")/lib.sh"

cmd="${1:-status}"

# Flip every underlying flag that gates a user-facing source, in lockstep.
# See lib.sh::is_source_enabled for the source -> flags mapping. Sibling-aware
# for audio: disabling audio-mic only turns off connectors.audio when
# audio-system is also off (and vice versa), so disabling one doesn't kill
# the other.
_flip_source() {
  local src="$1" new_val="$2"  # new_val is "true" or "false"
  case "$src" in
    claude-code)
      "$ALVUM_BIN" config-set "connectors.claude-code.enabled" "$new_val" >/dev/null
      ;;
    codex)
      "$ALVUM_BIN" config-set "connectors.codex.enabled" "$new_val" >/dev/null
      ;;
    screen)
      "$ALVUM_BIN" config-set "capture.screen.enabled"     "$new_val" >/dev/null
      "$ALVUM_BIN" config-set "connectors.screen.enabled"  "$new_val" >/dev/null
      ;;
    audio-mic|audio-system)
      "$ALVUM_BIN" config-set "capture.$src.enabled" "$new_val" >/dev/null
      if [[ "$new_val" == "true" ]]; then
        "$ALVUM_BIN" config-set "connectors.audio.enabled" "true" >/dev/null
      else
        local other
        if [[ "$src" == "audio-mic" ]]; then other="audio-system"; else other="audio-mic"; fi
        local other_cap
        other_cap=$(_read_enabled_flag "capture.$other")
        if [[ "$other_cap" != "true" ]]; then
          "$ALVUM_BIN" config-set "connectors.audio.enabled" "false" >/dev/null
        fi
      fi
      ;;
    *)
      return 1
      ;;
  esac
}

_stop_app() {
  # TERM goes to all Alvum.app processes (main, helpers, and the Rust
  # subprocess inherited as a child). Electron's before-quit hook also
  # stops the subprocess gracefully.
  pkill -TERM -f "$ALVUM_APP_BUNDLE_NAME/Contents/MacOS/Alvum" 2>/dev/null || true
  # Wait up to 3s for it to exit.
  local i=0
  while alvum_app_running && (( i < 6 )); do
    sleep 0.5
    i=$((i + 1))
  done
  if alvum_app_running; then
    pkill -KILL -f "$ALVUM_APP_BUNDLE_NAME/Contents/MacOS/Alvum" 2>/dev/null || true
  fi
}

case "$cmd" in
  start)
    ensure_dirs
    app=$(alvum_app_bundle_path) || {
      echo "Alvum.app not found. Build it:" >&2
      echo "  cd app && npm install && npx electron-builder --mac --dir" >&2
      exit 1
    }
    echo "--> starting capture via $app"
    open "$app"
    # Give Electron a moment to register with LaunchServices + spawn the
    # capture subprocess, so status prints truthfully if called next.
    sleep 2
    if alvum_app_running; then
      echo "    Alvum.app running"
    else
      echo "    Alvum.app failed to start; check $ALVUM_LOGS_DIR/shell.log" >&2
      exit 1
    fi
    ;;

  stop)
    echo "--> stopping capture"
    _stop_app
    echo "    stopped (config flags left alone; restart with capture.sh start)"
    ;;

  toggle)
    src="${2:-}"
    case "$src" in
      claude-code|codex|audio-mic|audio-system|screen) ;;
      *)
        echo "usage: $0 toggle { claude-code | codex | audio-mic | audio-system | screen }" >&2
        exit 2
        ;;
    esac
    current=$(is_source_enabled "$src")
    if [[ "$current" == "true" ]]; then
      _flip_source "$src" "false"
      new_state="disabled"
    else
      _flip_source "$src" "true"
      new_state="enabled"
    fi
    echo "$src: $new_state"
    # Route the toast through Alvum.app's queue so the alvum bundle
    # icon shows. `osascript display notification` is hard-locked to
    # the Script Editor icon since Big Sur, which is why we don't
    # call it directly.
    alvum_notify "Alvum" "$src $new_state"
    # Rust-side sources live inside Alvum.app's capture subprocess —
    # restart the app so the fresh config takes effect. claude-code /
    # codex are read-only at extract time; no restart needed.
    case "$src" in
      screen|audio-mic|audio-system)
        if alvum_app_running; then
          echo "    restarting Alvum.app to reload config"
          _stop_app
          app=$(alvum_app_bundle_path) && open "$app" || true
        fi
        ;;
    esac
    ;;

  status)
    if alvum_app_running; then
      echo "shell:    running"
    else
      echo "shell:    stopped"
    fi
    for s in claude-code codex audio-mic audio-system screen; do
      on=$(is_source_enabled "$s")
      marker=$([[ "$on" == "true" ]] && echo "✓" || echo "·")
      printf "  %s %s\n" "$marker" "$s"
    done
    today_dir="$ALVUM_CAPTURE/$(today)"
    if [[ -d "$today_dir" ]]; then
      echo "today:    $(find "$today_dir" -type f | wc -l | tr -d ' ') files under $today_dir"
    else
      echo "today:    no captures"
    fi
    ;;

  *)
    echo "usage: $0 {start|stop|status|toggle <source>}" >&2
    exit 2
    ;;
esac
