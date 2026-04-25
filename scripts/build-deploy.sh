#!/usr/bin/env bash
# One-shot rebuild → sign → redeploy → relaunch for the Alvum capture
# stack. Wraps the multi-step recipe that the docs in AGENTS.md describe.
#
# Default mode: rebuild only the Rust binary, sign it with alvum-dev,
# re-seal the .app bundle (otherwise its sealed-resources hash fails
# verify), kill the running Alvum.app, and relaunch via LaunchServices
# so the capture subprocess inherits the .app's TCC chain.
#
# Use --full when main.js, package.json, or assets/ have changed: that
# adds an `npm run pack` step which rebuilds the Electron bundle from
# scratch (and itself runs sign-app.sh as a postpack step).
#
# Why each step exists:
#   - sign-binary.sh on bin/alvum: TCC validates each process's own
#     signing identity, NOT the parent's grant. An ad-hoc signed inner
#     binary gets a content-hash identifier that changes on every cargo
#     build → TCC re-prompts. Stable cert (alvum-dev) keeps the identity
#     identical across rebuilds.
#   - sign-app.sh on Alvum.app: macOS verifies the .app's sealed-resources
#     hash on launch. Replacing bin/alvum invalidates the parent seal,
#     and on a strict-verify host the launch is denied. Re-sealing
#     restores the seal without changing the .app's signing identity.
#   - LaunchServices relaunch (`open Alvum.app`): the bin/alvum capture
#     process spawns AS A CHILD of Alvum.app, which makes Alvum.app the
#     responsible-process for TCC. Direct-running bin/alvum from a
#     terminal makes the terminal the responsible process, so TCC checks
#     the terminal's grants instead — a silently-different code path.
#
# Usage:
#   scripts/build-deploy.sh                      # Rust-only iteration loop
#   scripts/build-deploy.sh --full               # also npm run pack
#   scripts/build-deploy.sh --no-restart         # skip the pkill+open
#   scripts/build-deploy.sh --bundle /path/to/Alvum.app  # target a different bundle

set -euo pipefail
source "$(dirname "$0")/lib.sh"

# Make the agent / non-interactive shell case work too — cargo lives in
# ~/.cargo/bin which a login shell adds via rustup but a bash subshell
# usually doesn't.
[[ ":$PATH:" == *":$HOME/.cargo/bin:"* ]] || export PATH="$HOME/.cargo/bin:$PATH"

mode="rust"
restart=1
bundle="$ALVUM_REPO/app/dist/mac-arm64/Alvum.app"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --full) mode="full"; shift ;;
    --no-restart) restart=0; shift ;;
    --bundle) bundle="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,/^set -e/p' "$0" | sed 's/^# \?//'
      exit 0
      ;;
    *) echo "unknown flag: $1" >&2; exit 1 ;;
  esac
done

if [[ "$mode" == "full" ]]; then
  echo "==> npm install (idempotent)"
  (cd "$ALVUM_REPO/app" && npm install >/dev/null 2>&1)

  echo "==> npm run pack (electron-builder + sign-app.sh)"
  (cd "$ALVUM_REPO/app" && npm run pack 2>&1 | tail -5)

  # npm run pack writes into the worktree's app/dist/mac-arm64/Alvum.app.
  # If the requested bundle target lives elsewhere (typical: a sibling
  # main worktree where the dock alias is pinned), copy it over.
  source_bundle="$ALVUM_REPO/app/dist/mac-arm64/Alvum.app"
  if [[ "$bundle" != "$source_bundle" ]]; then
    echo "==> copying bundle → $bundle"
    rm -rf "$bundle"
    cp -R "$source_bundle" "$(dirname "$bundle")"
  fi
fi

echo "==> cargo build --release -p alvum-cli"
(cd "$ALVUM_REPO" && cargo build --release -p alvum-cli 2>&1 | tail -2)

inner="$bundle/Contents/Resources/bin/alvum"
if [[ ! -d "$bundle" ]]; then
  echo "error: bundle not found at $bundle" >&2
  echo "  hint: run with --full to build it from scratch" >&2
  exit 1
fi

echo "==> install Rust binary into bundle"
mkdir -p "$(dirname "$inner")"
cp "$ALVUM_REPO/target/release/alvum" "$inner"

# briefing.sh and other CLI tools spawn $ALVUM_BIN at
# ~/.alvum/runtime/Alvum.app/Contents/MacOS/alvum, NOT the .app's bundled
# Resources/bin/alvum. Without this sync the bundled binary gets the
# new code (capture works) but briefing.sh keeps running the old binary
# (no progress events, stale features). Same source binary, two
# install paths — update both so behaviour stays consistent.
mkdir -p "$(dirname "$ALVUM_BIN")"
cp "$ALVUM_REPO/target/release/alvum" "$ALVUM_BIN"

# Sign the inner binary BEFORE re-sealing the .app — codesign on the
# parent records the inner binary's content hash, so the order matters.
echo "==> sign inner binary (alvum-dev)"
codesign --sign alvum-dev --force --timestamp=none "$inner" 2>&1 \
  | grep -v "replacing existing signature" || true

# Sign the runtime-location binary too. sign-binary.sh handles the
# self-signed-cert keychain dance + ad-hoc fallback if cert is missing.
echo "==> sign runtime-location binary ($ALVUM_BIN)"
"$ALVUM_REPO/scripts/sign-binary.sh" 2>&1 | tail -1

# `sign-app.sh` does the inside-out sign of every helper / framework
# / outer bundle without --options runtime. See AGENTS.md for why that
# specific incantation is required.
echo "==> re-seal bundle (sign-app.sh)"
"$ALVUM_REPO/scripts/sign-app.sh" "$bundle" 2>&1 | tail -3

echo "==> verify"
codesign --verify --strict "$bundle" 2>&1 | tail -3
echo "  inner: $(codesign -dv "$inner" 2>&1 | grep -E 'Authority|Identifier' | tr '\n' ' ')"

if [[ "$restart" == 1 ]]; then
  echo "==> restart Alvum.app via LaunchServices"
  pkill -TERM -f "Alvum.app/Contents" 2>/dev/null || true
  sleep 2
  open "$bundle"
  sleep 2
  if pgrep -f "$bundle/Contents/MacOS/Alvum" >/dev/null; then
    echo "  ✓ Alvum.app running"
  else
    echo "  ✗ Alvum.app not running — check ~/.alvum/runtime/logs/shell.log" >&2
    exit 1
  fi
fi

echo "==> done"
