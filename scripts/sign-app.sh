#!/usr/bin/env bash
# Sign the Alvum.app bundle with the configured code-signing identity.
# Developer ID Application identities are preferred when installed; otherwise
# the scripts fall back to the persistent self-signed `alvum-dev` cert.
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
source "$(dirname "$0")/signing.sh"

CERT_NAME="$(alvum_resolve_sign_identity)"
APP="${1:-}"
if [[ -z "$APP" || ! -d "$APP" ]]; then
  echo "usage: $0 <path-to-Alvum.app>" >&2
  exit 1
fi

if ! alvum_sign_identity_available "$CERT_NAME"; then
  echo "signing identity '$CERT_NAME' not found; set ALVUM_SIGN_IDENTITY or run scripts/sign-binary.sh to create alvum-dev" >&2
  exit 1
fi

# Common args. --timestamp=none is the default for reproducible local
# deploys. Set ALVUM_SIGN_TIMESTAMP=1 to ask Apple for a timestamp.
# No --options runtime — see header.
alvum_codesign_args "$CERT_NAME"
SIGN_ARGS=("${ALVUM_CODESIGN_ARGS[@]}")

echo "==> signing $APP with '$CERT_NAME'"

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
  [[ -d "$helper" ]] || continue
  helper_inner="$helper/Contents/MacOS/$(basename "$helper" .app)"
  if [[ -f "$helper_inner" ]]; then
    codesign "${SIGN_ARGS[@]}" "$helper_inner" 2>&1 | grep -v "replacing existing signature" || true
  fi
  codesign "${SIGN_ARGS[@]}" "$helper" 2>&1 | grep -v "replacing existing signature" || true
done
for helper in "$APP/Contents/Helpers"/*.app; do
  [[ -d "$helper" ]] || continue
  helper_inner="$helper/Contents/MacOS/alvum"
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
