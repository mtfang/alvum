#!/usr/bin/env bash
# Control the capture daemon and the per-source config flags.
#
# Subcommands:
#   start                        — load + bootstrap the daemon plist
#   stop                         — unload the daemon plist
#   status                       — print daemon + per-source state
#   toggle <source>              — flip a source on/off; kickstart daemon to reload
#                                  (source ∈ audio-mic | audio-system | screen | claude-code)

set -euo pipefail
source "$(dirname "$0")/lib.sh"

cmd="${1:-status}"
PLIST_SRC="$ALVUM_REPO/launchd/$ALVUM_CAPTURE_LABEL.plist"
PLIST_DST="$ALVUM_LAUNCHAGENTS/$ALVUM_CAPTURE_LABEL.plist"

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
        # Enabling this mic/system captures → ensure the shared audio
        # connector is on so extract picks up the data.
        "$ALVUM_BIN" config-set "connectors.audio.enabled" "true" >/dev/null
      else
        # Only turn off the shared connector when the sibling is also off.
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

case "$cmd" in
  start)
    ensure_dirs
    echo "--> starting capture daemon (reads config for enabled sources)"
    install_plist "$PLIST_SRC" "$PLIST_DST"

    cat <<EOF

capture daemon started. writing to: $ALVUM_CAPTURE/<today>/

per-source enable/disable is in $ALVUM_CONFIG_FILE; toggle with:
  capture.sh toggle <source>
where <source> ∈ { audio-mic, audio-system, screen, claude-code }.

macOS will prompt for permissions the first time each source captures.
If capture.err shows permission errors, grant in System Settings → Privacy & Security.
EOF
    ;;

  stop)
    echo "--> stopping capture daemon"
    unload_plist "$PLIST_DST"
    echo "    capture stopped (config flags left alone; restart with capture.sh start)"
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
      echo "$src: disabled"
    else
      _flip_source "$src" "true"
      echo "$src: enabled"
    fi
    # Kick the daemon ONLY for sources that actually run in it. claude-code
    # and codex are read-only at extract time — no daemon, no kickstart,
    # and no surprise macOS permission prompts on toggle.
    case "$src" in
      screen|audio-mic|audio-system)
        if plist_loaded "$ALVUM_CAPTURE_LABEL"; then
          launchctl kickstart -k "gui/$UID/$ALVUM_CAPTURE_LABEL" 2>/dev/null || true
        fi
        ;;
    esac
    ;;

  status)
    if plist_loaded "$ALVUM_CAPTURE_LABEL"; then
      echo "daemon:   loaded"
    else
      echo "daemon:   not loaded"
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
