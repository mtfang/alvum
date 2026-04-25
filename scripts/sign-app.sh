#!/usr/bin/env bash
# Sign the Alvum.app bundle with the persistent self-signed `alvum-dev`
# cert so macOS TCC keys permissions on cert identity (stable across
# rebuilds) instead of binary content hash. Companion to sign-binary.sh,
# which signs the standalone Rust binary at $ALVUM_BIN.
#
# WHY THIS SCRIPT EXISTS (rather than codesign --deep):
#   `codesign --deep --options runtime` against an Electron bundle leaves
#   the embedded `Electron Framework.framework` with a hardened-runtime
#   signature whose Team-ID expectation dyld then refuses at load time
#   ("mapping process and mapped file have different Team IDs"). The bundle
#   verifies as valid but won't launch. The fix: sign inside-out, no
#   --deep, NO hardened runtime (matches the electron-builder ad-hoc
#   default that the original April-23 build used).
#
# Order is important — codesign records the hashes of nested binaries,
# so any later modification to a child invalidates the parent's signature.

set -euo pipefail

CERT_NAME="alvum-dev"
APP="${1:-}"
if [[ -z "$APP" || ! -d "$APP" ]]; then
  echo "usage: $0 <path-to-Alvum.app>" >&2
  exit 1
fi

# Common args. --timestamp=none skips Apple's RFC3161 service (we're
# self-signed, no point). No --options runtime — see header.
SIGN_ARGS=(--sign "$CERT_NAME" --force --timestamp=none)

echo "==> signing inner dylibs"
find "$APP/Contents/Frameworks" -name "*.dylib" -exec \
  codesign "${SIGN_ARGS[@]}" {} \; 2>&1 | grep -v "replacing existing signature" || true

echo "==> signing each .framework"
for fw in "$APP/Contents/Frameworks"/*.framework; do
  inner="$fw/Versions/A/$(basename "$fw" .framework)"
  if [[ -f "$inner" ]]; then
    codesign "${SIGN_ARGS[@]}" "$inner" 2>&1 | grep -v "replacing existing signature" || true
  fi
  codesign "${SIGN_ARGS[@]}" "$fw" 2>&1 | grep -v "replacing existing signature" || true
done

echo "==> signing helper apps"
for helper in "$APP/Contents/Frameworks"/*.app; do
  helper_inner="$helper/Contents/MacOS/$(basename "$helper" .app)"
  if [[ -f "$helper_inner" ]]; then
    codesign "${SIGN_ARGS[@]}" "$helper_inner" 2>&1 | grep -v "replacing existing signature" || true
  fi
  codesign "${SIGN_ARGS[@]}" "$helper" 2>&1 | grep -v "replacing existing signature" || true
done

echo "==> signing outer bundle"
codesign "${SIGN_ARGS[@]}" "$APP" 2>&1 | grep -v "replacing existing signature" || true

echo "==> verifying"
codesign --verify --deep --strict "$APP" 2>&1 | tail -3

# Final smoke-check: make sure the main binary's TCC identity is stable
# (Identifier=com.alvum.capture). If this drifts, TCC will treat the app
# as new and re-prompt for Mic / Screen on next launch.
got=$(codesign -dv "$APP" 2>&1 | awk -F= '/^Identifier=/ {print $2}')
if [[ "$got" != "com.alvum.capture" ]]; then
  echo "warning: Identifier='$got' (expected com.alvum.capture); TCC grants may not persist" >&2
fi

echo "==> signed: $APP"
