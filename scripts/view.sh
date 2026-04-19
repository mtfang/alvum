#!/usr/bin/env bash
# Open today's briefing in the default viewer.

set -euo pipefail
source "$(dirname "$0")/lib.sh"

B="$ALVUM_BRIEFINGS_DIR/$(today)/briefing.md"
if [[ ! -f "$B" ]]; then
  echo "no briefing for $(today). run:  $(dirname "$0")/briefing.sh"
  exit 1
fi
open "$B"
