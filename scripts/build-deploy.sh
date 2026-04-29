#!/usr/bin/env bash
# One-shot rebuild → sign → redeploy → relaunch for the Alvum capture
# stack. Wraps the multi-step recipe that the docs in AGENTS.md describe.
#
# Default mode: rebuild only the Rust binary, sign it with the configured
# identity, re-seal the .app bundle (otherwise its sealed-resources hash
# fails verify), kill the running Alvum.app, and relaunch via LaunchServices
# so the capture subprocess inherits the .app's TCC chain.
#
# Use --full when main.js, package.json, or assets/ have changed: that
# adds an `npm run pack` step which rebuilds the Electron bundle from
# scratch (and itself runs sign-app.sh as a postpack step).
#
# Why each step exists:
#   - sign-binary.sh on the capture helper binary: TCC validates each process's own
#     signing identity, NOT the parent's grant. An ad-hoc signed inner
#     binary gets a content-hash identifier that changes on every cargo
#     build → TCC re-prompts. A stable cert keeps the identity identical
#     across rebuilds.
#   - sign-app.sh on Alvum.app: macOS verifies the .app's sealed-resources
#     hash on launch. Replacing the helper binary invalidates the parent seal,
#     and on a strict-verify host the launch is denied. Re-sealing
#     restores the seal without changing the .app's signing identity.
#   - LaunchServices relaunch (`open Alvum.app`): the bundled helper app's
#     capture process spawns AS A CHILD of Alvum.app, which makes Alvum.app the
#     responsible-process for TCC. Direct-running alvum from a
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
source "$(dirname "$0")/signing.sh"

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

set_plist_string() {
  local plist="$1" key="$2" value="$3"
  /usr/libexec/PlistBuddy -c "Set :$key $value" "$plist" 2>/dev/null \
    || /usr/libexec/PlistBuddy -c "Add :$key string $value" "$plist"
}

ensure_app_icon_assets_car() {
  local cache="$ALVUM_REPO/app/dist/.icon-assets/Assets.car"
  if [[ -n "${ALVUM_APP_ICON_ASSETS_CAR:-}" && -f "$ALVUM_APP_ICON_ASSETS_CAR" ]]; then
    return 0
  fi
  if [[ "${ALVUM_APP_ICON_ASSETS_UNAVAILABLE:-0}" == 1 ]]; then
    return 1
  fi
  if [[ -f "$cache" ]]; then
    ALVUM_APP_ICON_ASSETS_CAR="$cache"
    return 0
  fi

  local icon_source="$ALVUM_REPO/app/assets/icon.png"
  if [[ ! -f "$icon_source" ]]; then
    echo "warning: app icon source not found at $icon_source" >&2
    ALVUM_APP_ICON_ASSETS_UNAVAILABLE=1
    return 1
  fi
  if ! command -v xcrun >/dev/null || ! xcrun --find actool >/dev/null 2>&1; then
    echo "warning: xcrun actool not found; install full Xcode to compile Assets.car for Control Center icon lookup" >&2
    ALVUM_APP_ICON_ASSETS_UNAVAILABLE=1
    return 1
  fi

  local tmp catalog appicon compiled
  tmp="$(mktemp -d)"
  catalog="$tmp/AppIcon.xcassets"
  appicon="$catalog/AppIcon.appiconset"
  compiled="$tmp/compiled"
  mkdir -p "$appicon" "$compiled" "$(dirname "$cache")"

  sips -z 16 16 "$icon_source" --out "$appicon/icon_16x16.png" >/dev/null
  sips -z 32 32 "$icon_source" --out "$appicon/icon_16x16@2x.png" >/dev/null
  sips -z 32 32 "$icon_source" --out "$appicon/icon_32x32.png" >/dev/null
  sips -z 64 64 "$icon_source" --out "$appicon/icon_32x32@2x.png" >/dev/null
  sips -z 128 128 "$icon_source" --out "$appicon/icon_128x128.png" >/dev/null
  sips -z 256 256 "$icon_source" --out "$appicon/icon_128x128@2x.png" >/dev/null
  sips -z 256 256 "$icon_source" --out "$appicon/icon_256x256.png" >/dev/null
  sips -z 512 512 "$icon_source" --out "$appicon/icon_256x256@2x.png" >/dev/null
  sips -z 512 512 "$icon_source" --out "$appicon/icon_512x512.png" >/dev/null
  cp "$icon_source" "$appicon/icon_512x512@2x.png"

  cat > "$appicon/Contents.json" <<'JSON'
{
  "images": [
    { "idiom": "mac", "size": "16x16", "scale": "1x", "filename": "icon_16x16.png" },
    { "idiom": "mac", "size": "16x16", "scale": "2x", "filename": "icon_16x16@2x.png" },
    { "idiom": "mac", "size": "32x32", "scale": "1x", "filename": "icon_32x32.png" },
    { "idiom": "mac", "size": "32x32", "scale": "2x", "filename": "icon_32x32@2x.png" },
    { "idiom": "mac", "size": "128x128", "scale": "1x", "filename": "icon_128x128.png" },
    { "idiom": "mac", "size": "128x128", "scale": "2x", "filename": "icon_128x128@2x.png" },
    { "idiom": "mac", "size": "256x256", "scale": "1x", "filename": "icon_256x256.png" },
    { "idiom": "mac", "size": "256x256", "scale": "2x", "filename": "icon_256x256@2x.png" },
    { "idiom": "mac", "size": "512x512", "scale": "1x", "filename": "icon_512x512.png" },
    { "idiom": "mac", "size": "512x512", "scale": "2x", "filename": "icon_512x512@2x.png" }
  ],
  "info": { "version": 1, "author": "xcode" }
}
JSON

  xcrun actool \
    --compile "$compiled" \
    --platform macosx \
    --target-device mac \
    --minimum-deployment-target 10.15 \
    --app-icon AppIcon \
    --output-partial-info-plist "$tmp/assetcatalog-info.plist" \
    "$catalog" >/dev/null

  cp "$compiled/Assets.car" "$cache"
  rm -rf "$tmp"
  ALVUM_APP_ICON_ASSETS_CAR="$cache"
}

install_app_icon_metadata() {
  local app_dir="$1"
  local plist="$app_dir/Contents/Info.plist"
  local resources="$app_dir/Contents/Resources"
  [[ -f "$plist" ]] || return 0
  mkdir -p "$resources"

  set_plist_string "$plist" CFBundleIconName AppIcon
  set_plist_string "$plist" CFBundleIconFile icon.icns

  if ensure_app_icon_assets_car; then
    cp "$ALVUM_APP_ICON_ASSETS_CAR" "$resources/Assets.car"
  fi
}

echo "==> cargo build --release -p alvum-cli"
(cd "$ALVUM_REPO" && cargo build --release -p alvum-cli 2>&1 | tail -2)

if [[ ! -d "$bundle" ]]; then
  echo "error: bundle not found at $bundle" >&2
  echo "  hint: run with --full to build it from scratch" >&2
  exit 1
fi

helper_app="$bundle/Contents/Helpers/Alvum Capture.app"
helper_inner="$helper_app/Contents/MacOS/alvum"
helper_resources="$helper_app/Contents/Resources"
inner="$helper_inner"
legacy_inner="$bundle/Contents/Resources/bin/alvum"

echo "==> install Rust binary into bundle"
mkdir -p "$(dirname "$helper_inner")" "$helper_resources" "$(dirname "$legacy_inner")"
cp "$ALVUM_REPO/target/release/alvum" "$inner"
cp "$ALVUM_REPO/target/release/alvum" "$legacy_inner"
cp "$ALVUM_REPO/crates/alvum-cli/Info.plist" "$helper_app/Contents/Info.plist"
if [[ -f "$bundle/Contents/Resources/icon.icns" ]]; then
  cp "$bundle/Contents/Resources/icon.icns" "$helper_resources/icon.icns"
elif [[ -f "$ALVUM_REPO/app/dist/.icon-icns/icon.icns" ]]; then
  cp "$ALVUM_REPO/app/dist/.icon-icns/icon.icns" "$helper_resources/icon.icns"
else
  echo "warning: icon.icns not found; helper app will use the default app icon" >&2
fi
install_app_icon_metadata "$bundle"
install_app_icon_metadata "$helper_app"

# briefing.sh and other CLI tools spawn $ALVUM_BIN at
# ~/.alvum/runtime/Alvum.app/Contents/MacOS/alvum, NOT the .app's bundled
# helper binary. Without this sync the bundled binary gets the
# new code (capture works) but briefing.sh keeps running the old binary
# (no progress events, stale features). Same source binary, two
# install paths — update both so behaviour stays consistent.
mkdir -p "$(dirname "$ALVUM_BIN")"
cp "$ALVUM_REPO/target/release/alvum" "$ALVUM_BIN"
cp "$ALVUM_REPO/crates/alvum-cli/Info.plist" "$ALVUM_APP_PLIST"
if [[ -f "$helper_resources/icon.icns" ]]; then
  mkdir -p "$ALVUM_APP_CONTENTS/Resources"
  cp "$helper_resources/icon.icns" "$ALVUM_APP_CONTENTS/Resources/icon.icns"
fi
install_app_icon_metadata "$ALVUM_APP_DIR"

CERT_NAME="$(alvum_resolve_sign_identity)"

# Sign the runtime-location binary too. sign-binary.sh handles the local
# dev-cert keychain bootstrap + ad-hoc fallback if the fallback cert is
# missing. It uses the same identity as the bundle signing path.
echo "==> sign runtime-location binary ($CERT_NAME)"
"$ALVUM_REPO/scripts/sign-binary.sh"

if ! alvum_sign_identity_available "$CERT_NAME"; then
  echo "error: signing identity '$CERT_NAME' not available" >&2
  exit 1
fi
alvum_codesign_args "$CERT_NAME"

# Sign helper binaries BEFORE re-sealing the .app — codesign on the
# parent records child content hashes, so the order matters.
echo "==> sign helper binaries ($CERT_NAME)"
codesign "${ALVUM_CODESIGN_ARGS[@]}" "$inner" 2>&1 \
  | grep -v "replacing existing signature" || true
codesign "${ALVUM_CODESIGN_ARGS[@]}" "$legacy_inner" 2>&1 \
  | grep -v "replacing existing signature" || true

# `sign-app.sh` does the inside-out sign of every helper / framework
# / outer bundle without --options runtime. See AGENTS.md for why that
# specific incantation is required.
echo "==> re-seal bundle (sign-app.sh)"
"$ALVUM_REPO/scripts/sign-app.sh" "$bundle" 2>&1 | tail -3

echo "==> verify"
codesign --verify --strict "$bundle" 2>&1 | tail -3
echo "  inner: $(codesign -dvv "$inner" 2>&1 | grep -E 'Authority|Identifier|TeamIdentifier' | tr '\n' ' ')"

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
