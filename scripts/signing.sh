#!/usr/bin/env bash
# Shared code-signing identity resolution. Source, don't execute.

ALVUM_DEV_CERT_NAME="${ALVUM_DEV_CERT_NAME:-alvum-dev}"

alvum_developer_id_identity() {
  security find-identity -v -p codesigning 2>/dev/null \
    | awk -F '"' '/Developer ID Application:/ { print $2; exit }'
}

alvum_resolve_sign_identity() {
  if [[ -n "${ALVUM_SIGN_IDENTITY:-}" ]]; then
    printf '%s\n' "$ALVUM_SIGN_IDENTITY"
    return 0
  fi

  local developer_id
  developer_id="$(alvum_developer_id_identity || true)"
  if [[ -n "$developer_id" ]]; then
    printf '%s\n' "$developer_id"
    return 0
  fi

  printf '%s\n' "$ALVUM_DEV_CERT_NAME"
}

alvum_sign_identity_available() {
  local identity="$1"
  [[ "$identity" == "-" ]] && return 0

  security find-identity -v -p codesigning 2>/dev/null \
    | awk -v id="$identity" '
        index($0, "\"" id "\"") > 0 { found=1 }
        $2 == id { found=1 }
        END { exit found ? 0 : 1 }
      ' && return 0

  security find-certificate -c "$identity" >/dev/null 2>&1
}

alvum_codesign_args() {
  local identity="$1"
  ALVUM_CODESIGN_ARGS=(--sign "$identity" --force)
  if [[ "${ALVUM_SIGN_TIMESTAMP:-none}" == "none" ]]; then
    ALVUM_CODESIGN_ARGS+=(--timestamp=none)
  else
    ALVUM_CODESIGN_ARGS+=(--timestamp)
  fi
}
