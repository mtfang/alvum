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

# Map a source name to its config key.
source_config_key() {
  case "$1" in
    claude-code) echo "connectors.claude-code.enabled" ;;
    audio-mic|audio-system|screen)
                 echo "capture.$1.enabled" ;;
    *)           echo ""; return 1 ;;
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
    key=$(source_config_key "$src") || {
      echo "usage: $0 toggle { audio-mic | audio-system | screen | claude-code }" >&2
      exit 2
    }
    current=$(is_source_enabled "$src")
    if [[ "$current" == "true" ]]; then
      "$ALVUM_BIN" config-set "$key" "false" >/dev/null
      echo "$src: disabled"
    else
      "$ALVUM_BIN" config-set "$key" "true" >/dev/null
      echo "$src: enabled"
    fi
    # Kick the daemon so it picks up the new config.
    if plist_loaded "$ALVUM_CAPTURE_LABEL"; then
      launchctl kickstart -k "gui/$UID/$ALVUM_CAPTURE_LABEL" 2>/dev/null || true
    fi
    ;;

  status)
    if plist_loaded "$ALVUM_CAPTURE_LABEL"; then
      echo "daemon:   loaded"
    else
      echo "daemon:   not loaded"
    fi
    for s in claude-code audio-mic audio-system screen; do
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
