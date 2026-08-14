#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../../.." && pwd)"
RUN="$HERE/runs/$(date -u +%F)"
CONTAINER="sipx-comparison-aead-srtp-sdes"
IMAGE="docker.io/holius/baresip@sha256:3f2deef4f8a03ca569c3f252b919c439b8b2015a0a68f5b2ddc22d3edd8e2da7"
mkdir -p "$ROOT/target" "$RUN"
SCRATCH="$(mktemp -d "$ROOT/target/comparison-aead-srtp-sdes.XXXXXX")"
touch "$SCRATCH/.owned-comparison-aead-srtp-sdes"

cleanup() {
    python3 "$HERE/bounded.py" 10 docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
    if [[ -d "$SCRATCH" && -f "$SCRATCH/.owned-comparison-aead-srtp-sdes" ]]; then
        rm -rf -- "$SCRATCH"
    fi
}
trap cleanup EXIT INT TERM

bounded() {
    python3 "$HERE/bounded.py" "$@"
}

start_peer() {
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
    bounded 30 docker run -d --name "$CONTAINER" --label sipx-comparison-aead-srtp-sdes=1 \
        --network host -v "$SCRATCH/peer:/proof:ro" "$IMAGE" baresip -f /proof >/dev/null
    local ready=0 peer_log attempt
    for attempt in {1..20}; do
        peer_log="$(bounded 5 docker logs "$CONTAINER" 2>&1 || true)"
        if grep -F 'baresip is ready.' <<<"$peer_log" >/dev/null; then
            ready=1
            break
        fi
        sleep 0.25 # poll interval: the readiness line, not time, is the awaited event
    done
    [[ "$ready" -eq 1 ]]
}

stop_and_capture() {
    local output="$1"
    bounded 10 docker stop -t 2 "$CONTAINER" >/dev/null || true
    bounded 5 docker logs "$CONTAINER" >"$SCRATCH/raw-peer.log" 2>&1 || true
    # Logs are text evidence; strip terminal colour without changing protocol facts.
    sed -E $'s/\x1B\[[0-9;]*[[:alpha:]]//g' "$SCRATCH/raw-peer.log" >"$output"
    bounded 10 docker rm "$CONTAINER" >/dev/null 2>&1 || true
}

run_positive() {
    local suite="$1"
    start_peer
    bounded 30 "$SCRATCH/build/debug/m72-sdes-proof" "$suite" \
        "$SCRATCH/peer/ca.pem" 127.0.0.1:5091 >"$RUN/$suite.json"
    stop_and_capture "$RUN/peer-$suite.log"
    grep -F "SRTP is Enabled (cryptosuite=$suite)" "$RUN/peer-$suite.log" >/dev/null
}

cd "$ROOT"
export RUSTC_WRAPPER=sccache
export CARGO_TARGET_DIR="$SCRATCH/build"
mkdir -p "$SCRATCH/peer"
cp "$HERE/peer/config" "$HERE/peer/accounts" "$SCRATCH/peer/"
bounded 180 docker pull "$IMAGE"
bounded 120 cargo run --quiet -p sipx-testkit --example issue-certs -- "$SCRATCH/peer" sipx.test
cp "$SCRATCH/peer/server.key" "$SCRATCH/peer/server-combined.pem"
openssl x509 -in "$SCRATCH/peer/server.pem" >>"$SCRATCH/peer/server-combined.pem"
bounded 300 cargo build --manifest-path "$HERE/adapter/Cargo.toml"
python3 "$HERE/build-negative.py" "$SCRATCH/negative-source" "$SCRATCH/negative-build" \
    "$RUN/KdfPerturbationSdes.json"

run_positive AEAD_AES_128_GCM
run_positive AEAD_AES_256_GCM

start_peer
bounded 30 "$SCRATCH/negative-build/debug/m72-sdes-proof" AEAD_AES_256_GCM \
    "$SCRATCH/peer/ca.pem" 127.0.0.1:5091 expect-kdf-failure \
    >"$RUN/KdfPerturbationSdes-result.json"
stop_and_capture "$RUN/peer-KdfPerturbationSdes.log"
test "$(grep -c 'failed to decrypt RTP packet' "$RUN/peer-KdfPerturbationSdes.log")" -ge 45

python3 "$HERE/seal.py" "$RUN" \
    "$(sha256sum "$SCRATCH/build/debug/m72-sdes-proof" | cut -d' ' -f1)" \
    "$(sha256sum "$SCRATCH/negative-build/debug/m72-sdes-proof" | cut -d' ' -f1)"
python3 "$ROOT/scripts/comparison-report.py" --check
