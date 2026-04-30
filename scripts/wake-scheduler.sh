#!/usr/bin/env bash
# Wake the Electron app so its main-process synthesis scheduler can decide
# whether any completed days are due. This script is intentionally thin:
# launchd supplies the wall-clock trigger, Electron owns queue policy.

set -euo pipefail
source "$(dirname "$0")/lib.sh"

intent_file="${ALVUM_LAUNCH_INTENT_FILE:-$ALVUM_RUNTIME/launch-intent.json}"
bundle="${ALVUM_APP_BUNDLE:-}"
if [[ -z "$bundle" || ! -d "$bundle" ]]; then
  bundle="$(alvum_app_bundle_path || true)"
fi

mkdir -p "$(dirname "$intent_file")"
cat > "$intent_file" <<JSON
{"source":"launchd-scheduler","run_synthesis_due":true,"skip_capture_autostart":false,"created_at":"$(date -u +"%Y-%m-%dT%H:%M:%SZ")"}
JSON

if [[ -n "$bundle" && -d "$bundle" ]]; then
  open -gj "$bundle"
else
  echo "Alvum.app not found; cannot wake synthesis scheduler" >&2
  exit 1
fi
