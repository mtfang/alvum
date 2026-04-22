#!/usr/bin/env bash
# Sign $ALVUM_BIN with a persistent self-signed code-signing certificate
# so macOS TCC keys permissions on cert identity (stable across rebuilds)
# instead of binary content hash (changes on every build).
#
# First run generates the cert in the login keychain. Subsequent runs
# reuse it. Falls back to ad-hoc signing if cert creation fails.

set -euo pipefail
source "$(dirname "$0")/lib.sh"

CERT_NAME="alvum-dev"
# login.keychain-db is the standard path on modern macOS (10.12+).
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"
# Non-empty password for the PKCS#12 blob. macOS `security import` rejects
# some OpenSSL-3-produced empty-password p12s with 'MAC verification failed';
# a trivial real password sidesteps that. The password never leaves this
# script — the cert is in the keychain after import, the p12 is deleted.
P12_PASS="alvum"

# `find-identity -v` would require the cert to be *trusted* by the system,
# which self-signed certs aren't by default. We only need the cert to *exist*
# with its private key — codesign is happy to use an untrusted self-signed
# cert for local dev signing.
have_cert() {
  security find-certificate -c "$CERT_NAME" "$KEYCHAIN" >/dev/null 2>&1
}

create_cert() {
  echo "--> generating self-signed code-signing cert '$CERT_NAME' in login keychain"
  local tmpdir
  tmpdir=$(mktemp -d)
  # Fire-and-forget cleanup; don't rely on the trap since we may be sourced.
  # Use an exit hook scoped to this function.

  cat > "$tmpdir/cert.conf" <<EOF
[req]
distinguished_name = req_dn
x509_extensions    = v3_extensions
prompt             = no

[req_dn]
CN = $CERT_NAME

[v3_extensions]
keyUsage               = critical, digitalSignature
extendedKeyUsage       = critical, codeSigning
subjectKeyIdentifier   = hash
authorityKeyIdentifier = keyid
basicConstraints       = critical, CA:false
EOF

  openssl req -new -x509 \
    -newkey rsa:2048 -nodes \
    -keyout "$tmpdir/cert.key" \
    -out "$tmpdir/cert.crt" \
    -days 3650 \
    -config "$tmpdir/cert.conf" \
    -extensions v3_extensions \
    2>/dev/null

  # Bundle into PKCS#12 for import into the keychain. OpenSSL 3.x defaults to
  # newer PKCS12 encryption algorithms that macOS's `security` tool does NOT
  # recognize (it errors with "MAC verification failed during PKCS12 import").
  # Pin the ciphers to the legacy SHA1/3DES set that Apple supports.
  openssl pkcs12 -export \
    -inkey "$tmpdir/cert.key" \
    -in "$tmpdir/cert.crt" \
    -name "$CERT_NAME" \
    -out "$tmpdir/cert.p12" \
    -passout "pass:$P12_PASS" \
    -certpbe PBE-SHA1-3DES \
    -keypbe  PBE-SHA1-3DES \
    -macalg  SHA1

  # -T /usr/bin/codesign lets codesign use the key without keychain prompts.
  security import "$tmpdir/cert.p12" \
    -k "$KEYCHAIN" \
    -P "$P12_PASS" \
    -T /usr/bin/codesign \
    -T /usr/bin/security \
    >/dev/null

  # Add codesign to the key's partition list so it can access the key
  # without prompting. Ignored if set-key-partition-list is unavailable
  # (older macOS) — in that case user may see one-time keychain prompts.
  security set-key-partition-list \
    -S 'apple-tool:,apple:,codesign:' \
    -s \
    -k "" \
    "$KEYCHAIN" >/dev/null 2>&1 || true

  rm -rf "$tmpdir"
  echo "    cert '$CERT_NAME' created"
}

sign_it() {
  # Sign the whole .app bundle so macOS recognises it as a bundled app and
  # keys TCC grants on the bundle identity, not a per-build cdhash.
  local target="$ALVUM_APP_DIR"
  [[ -d "$target" ]] || target="$ALVUM_BIN"
  echo "--> signing $target with '$CERT_NAME'"
  codesign \
    --sign "$CERT_NAME" \
    --force \
    --options runtime \
    --timestamp=none \
    --deep \
    "$target"

  codesign --verify --verbose=2 "$target" 2>&1 \
    | head -4 \
    | sed 's/^/    /'
}

fall_back_adhoc() {
  echo "--> falling back to ad-hoc sign (TCC will still re-prompt on each build)" >&2
  local target="$ALVUM_APP_DIR"
  [[ -d "$target" ]] || target="$ALVUM_BIN"
  codesign --sign - --force --deep "$target"
}

main() {
  if [[ ! -x "$ALVUM_BIN" ]]; then
    echo "no binary at $ALVUM_BIN; run install.sh first" >&2
    exit 1
  fi
  if [[ -d "$ALVUM_APP_DIR" && ! -f "$ALVUM_APP_PLIST" ]]; then
    echo "app bundle missing Info.plist at $ALVUM_APP_PLIST; run install.sh first" >&2
    exit 1
  fi

  if ! command -v openssl >/dev/null; then
    echo "openssl not found; falling back to ad-hoc sign" >&2
    fall_back_adhoc
    exit 0
  fi

  if ! have_cert; then
    create_cert || {
      echo "cert generation failed" >&2
      fall_back_adhoc
      exit 0
    }
  fi

  if ! have_cert; then
    # Cert creation silently didn't install (unusual) — fall back.
    fall_back_adhoc
    exit 0
  fi

  sign_it
}

main "$@"
