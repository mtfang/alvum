#!/usr/bin/env bash
# Build and notarize a distributable macOS Alvum release.
#
# This uses the existing `build-deploy` signing flow and then applies a
# notarization-only hardened-runtime signing pass before creating the DMG.
# The local deploy flow remains unchanged.
#
# Prerequisites:
# - A valid Developer ID Application certificate.
# - xcode-select developer tools, including xcrun/hdiutil.
# - notarytool keychain profile with API key credentials.
#
# Usage:
#   scripts/distribute-macos.sh [--version 0.1.0] [--bundle /path/Alvum.app]
#                              [--output-dir app/dist/release] [--notary-profile alvum]
#                              [--skip-notarize] [--skip-build]
#                              [--skip-hardened-sign]

set -euo pipefail

source "$(dirname "$0")/lib.sh"
source "$(dirname "$0")/signing.sh"

bundle="${ALVUM_REPO}/app/dist/mac-arm64/Alvum.app"
output_dir="${ALVUM_REPO}/app/dist/release"
notary_profile="${ALVUM_NOTARY_PROFILE:-alvum-notary}"
version=""
skip_notarize=0
build=1
hardened_sign=1
github_owner="${ALVUM_UPDATE_OWNER:-mtfang}"
github_repo="${ALVUM_UPDATE_REPO:-alvum}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --bundle)
      bundle="$2"
      shift 2
      ;;
    --output-dir)
      output_dir="$2"
      shift 2
      ;;
    --notary-profile)
      notary_profile="$2"
      shift 2
      ;;
    --version)
      version="$2"
      shift 2
      ;;
    --skip-notarize)
      skip_notarize=1
      shift 1
      ;;
    --skip-build)
      build=0
      shift 1
      ;;
    --skip-hardened-sign)
      hardened_sign=0
      shift 1
      ;;
    -h|--help)
      sed -n '1,220p' "$0" | sed 's/^# //'
      exit 0
      ;;
    *)
      echo "unknown flag: $1" >&2
      exit 1
      ;;
  esac
done

if [[ "$(uname)" != "Darwin" ]]; then
  echo "distribution is macOS-only" >&2
  exit 1
fi

if ! command -v xcrun >/dev/null; then
  echo "xcrun not found; install Xcode and run xcode-select -s" >&2
  exit 1
fi
if ! xcrun --find notarytool >/dev/null 2>&1; then
  echo "notarytool unavailable; install full Xcode (not CLT-only)" >&2
  exit 1
fi
if ! command -v hdiutil >/dev/null; then
  echo "hdiutil not found" >&2
  exit 1
fi

cert_name="$(alvum_resolve_sign_identity)"
if [[ "$cert_name" == "$ALVUM_DEV_CERT_NAME" ]]; then
  echo "distribution requires a Developer ID certificate, not the fallback '$ALVUM_DEV_CERT_NAME'" >&2
  exit 1
fi
if [[ "$cert_name" != *"Developer ID Application:"* ]] && ! security find-identity -v -p codesigning | grep -Fq "Developer ID Application:"; then
  echo "could not confirm a Developer ID Application identity (got: $cert_name)" >&2
  exit 1
fi
if ! alvum_sign_identity_available "$cert_name"; then
  echo "signing identity '$cert_name' is not available in this keychain" >&2
  exit 1
fi

if [[ "$build" == 1 ]]; then
  echo "==> build + release-sign (build-deploy.sh --full --no-restart)"
  ALVUM_SIGN_TIMESTAMP=1 "$ALVUM_REPO/scripts/build-deploy.sh" --full --no-restart --bundle "$bundle"
fi

if [[ ! -d "$bundle" ]]; then
  echo "built app bundle not found at: $bundle" >&2
  exit 1
fi

mkdir -p "$output_dir"

if [[ -z "$version" ]]; then
  version="$(awk -F'\"' '/"version"[[:space:]]*:/ {print $4; exit}' "$ALVUM_REPO/app/package.json" 2>/dev/null || true)"
fi
version="${version:-$(git -C "$ALVUM_REPO" describe --tags --dirty --always 2>/dev/null || echo 0.0.0)}"
arch="$(uname -m)"
artifact="$output_dir/Alvum-${version}-${arch}.dmg"
zip_artifact="$output_dir/Alvum-${version}-${arch}-mac.zip"
latest_yml="$output_dir/latest-mac.yml"
default_entitlements="$ALVUM_REPO/app/distribution-entitlements.plist"
update_config="$bundle/Contents/Resources/app-update.yml"

if ! command -v spctl >/dev/null; then
  echo "warning: spctl not found; skipping local signature gate check"
else
  echo "==> verify codesign"
  codesign -dv "$bundle/Contents/MacOS/Alvum" >/tmp/alvum.codesign.log 2>&1 && tail -n 5 /tmp/alvum.codesign.log
fi

echo "==> write updater config"
mkdir -p "$(dirname "$update_config")"
cat > "$update_config" <<EOF
provider: github
owner: $github_owner
repo: $github_repo
updaterCacheDirName: alvum-updater
EOF

if [[ "$hardened_sign" == 1 ]]; then
  entitlements="${ALVUM_DISTRIBUTION_ENTITLEMENTS:-$default_entitlements}"
  echo "==> re-sign for notarization hardened runtime"
  ALVUM_SIGN_TIMESTAMP=1 \
    ALVUM_CODESIGN_HARDENED_RUNTIME=1 \
    ALVUM_CODESIGN_ENTITLEMENTS="$entitlements" \
    "$ALVUM_REPO/scripts/sign-app.sh" "$bundle" 2>&1 | tail -3
else
  echo "==> skip hardened runtime sign (not recommended for notarization)"
  echo "==> re-sign app after updater config"
  ALVUM_SIGN_TIMESTAMP=1 "$ALVUM_REPO/scripts/sign-app.sh" "$bundle" 2>&1 | tail -3
fi

if [[ "$skip_notarize" == 0 ]]; then
  if [[ -z "$notary_profile" ]]; then
    echo "notary profile is empty; pass --notary-profile or set ALVUM_NOTARY_PROFILE" >&2
    exit 1
  fi

  echo "==> notarize app bundle for updater ZIP ($notary_profile)"
  app_notary_zip="$(mktemp -t alvum-notary-app.XXXXXX).zip"
  ditto -c -k --sequesterRsrc --keepParent "$bundle" "$app_notary_zip"
  xcrun notarytool submit "$app_notary_zip" --keychain-profile "$notary_profile" --wait
  rm -f "$app_notary_zip"
  xcrun stapler staple "$bundle"
fi

echo "==> create updater ZIP artifact"
rm -f "$zip_artifact"
ditto -c -k --sequesterRsrc --keepParent "$bundle" "$zip_artifact"
zip_size="$(stat -f%z "$zip_artifact")"
zip_sha512="$(openssl dgst -sha512 -binary "$zip_artifact" | openssl base64 -A)"
release_date="$(date -u +"%Y-%m-%dT%H:%M:%S.000Z")"
cat > "$latest_yml" <<EOF
version: $version
files:
  - url: $(basename "$zip_artifact")
    sha512: $zip_sha512
    size: $zip_size
path: $(basename "$zip_artifact")
sha512: $zip_sha512
releaseDate: '$release_date'
EOF

echo "==> create signed DMG artifact"
staging_dir="$(mktemp -d)"
cp -R "$bundle" "$staging_dir/"
ln -s /Applications "$staging_dir/Applications"
hdiutil create -srcfolder "$staging_dir" -volname "Alvum" -fs HFS+ -format UDZO -ov "$artifact"
rm -rf "$staging_dir"

echo "==> sign DMG artifact"
codesign --force --timestamp --sign "$cert_name" "$artifact"

if [[ "$skip_notarize" == 0 ]]; then
  echo "==> notarize DMG ($notary_profile)"
  xcrun notarytool submit "$artifact" --keychain-profile "$notary_profile" --wait
  xcrun stapler staple "$artifact"
fi

if command -v shasum >/dev/null; then
  shasum -a 256 "$artifact" | tee "${artifact}.sha256"
  shasum -a 256 "$zip_artifact" | tee "${zip_artifact}.sha256"
fi

echo "==> distribution ready"
echo "  artifact: $artifact"
echo "  updater zip: $zip_artifact"
echo "  updater feed: $latest_yml"
if command -v spctl >/dev/null; then
  echo "==> gate check (local bundle)"
  spctl --assess --type execute -vv "$bundle" || true
fi
