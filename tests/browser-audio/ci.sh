#!/usr/bin/env bash
# Provision one ephemeral identity and run the real native-browser M-51 proof in CI.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
IDENTITY_DIR=$(mktemp -d)
REMOVE_EVIDENCE=false
cleanup() {
    trap - EXIT INT TERM
    rm -f "$IDENTITY_DIR/certificate.pem" "$IDENTITY_DIR/key.pem"
    rmdir "$IDENTITY_DIR" 2>/dev/null || true
    if "$REMOVE_EVIDENCE"; then
        case $EVIDENCE_ROOT in
            /tmp/sipx-browser-audio-evidence.*) rm -rf -- "$EVIDENCE_ROOT" ;;
            *) printf 'browser-audio proof: refused unsafe temporary cleanup target: %s\n' "$EVIDENCE_ROOT" >&2 ;;
        esac
    fi
}
trap cleanup EXIT INT TERM

openssl req \
    -x509 \
    -newkey rsa:2048 \
    -nodes \
    -days 1 \
    -keyout "$IDENTITY_DIR/key.pem" \
    -out "$IDENTITY_DIR/certificate.pem" \
    -subj /CN=localhost \
    -addext subjectAltName=DNS:localhost \
    >/dev/null 2>&1

if [[ -n ${RUNNER_TEMP:-} ]]; then
    EVIDENCE_ROOT=$RUNNER_TEMP/sipx-browser-audio-evidence
    [[ ! -e $EVIDENCE_ROOT ]] || {
        printf 'browser-audio proof: evidence directory already exists: %s\n' "$EVIDENCE_ROOT" >&2
        exit 1
    }
    mkdir "$EVIDENCE_ROOT"
else
    EVIDENCE_ROOT=$(mktemp -d /tmp/sipx-browser-audio-evidence.XXXXXX)
    REMOVE_EVIDENCE=true
fi
export SIPX_BROWSER_AUDIO_WSS_CA="$IDENTITY_DIR/certificate.pem"
export SIPX_BROWSER_AUDIO_WSS_CERT="$IDENTITY_DIR/certificate.pem"
export SIPX_BROWSER_AUDIO_WSS_KEY="$IDENTITY_DIR/key.pem"
SIPX_BROWSER_AUDIO_WSS_SPKI_SHA256=$(
    "$ROOT/tests/browser-audio/driver.py" print-pin --cert "$IDENTITY_DIR/certificate.pem"
)
export SIPX_BROWSER_AUDIO_WSS_SPKI_SHA256
export SIPX_BROWSER_AUDIO_WEBDRIVER_CMD="$ROOT/tests/browser-audio/webdriver.sh"
export SIPX_BROWSER_AUDIO_PROOF_BIN="${CARGO_TARGET_DIR:-$ROOT/target}/debug/examples/browser_audio_proof"
export SIPX_BROWSER_AUDIO_EVIDENCE_DIR="$EVIDENCE_ROOT"
SIPX_BROWSER_AUDIO_MEDIA_ADDRESS=$(python3 - <<'PY'
import ipaddress
import socket

with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as route:
    route.connect(("192.0.2.1", 9))
    address = ipaddress.ip_address(route.getsockname()[0])
if address.is_loopback or address.is_unspecified:
    raise SystemExit(f"browser-audio proof: no non-loopback host media address: {address}")
print(address)
PY
)
export SIPX_BROWSER_AUDIO_MEDIA_ADDRESS

"$ROOT/tests/browser-audio/run.sh"
"$ROOT/tests/browser-audio/driver.py" validate-proof \
    --directory "$EVIDENCE_ROOT" \
    --pin "$SIPX_BROWSER_AUDIO_WSS_SPKI_SHA256"
