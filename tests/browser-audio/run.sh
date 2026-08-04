#!/usr/bin/env bash
# Own the complete M-51 proof process tree. See docs/specs/browser-audio-proof.md.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
DRIVER="$ROOT/tests/browser-audio/driver.py"
TOTAL_TIMEOUT=${SIPX_BROWSER_AUDIO_TOTAL_TIMEOUT:-300}
ROLE_TIMEOUT=${SIPX_BROWSER_AUDIO_ROLE_TIMEOUT:-120}
OUTPUT_BLOCKS=1024 # Bash ulimit -f units: cap each stdout/stderr file at 1 MiB.
declare -a OWNED_GROUPS=()
declare -a LEADERS=()
ADMIT=true

if [[ ${SIPX_BROWSER_AUDIO_INTERNAL:-0} != 1 ]]; then
    SUPERVISOR_DIR=$(mktemp -d)
    SUPERVISOR_OWNED_GROUPS="$SUPERVISOR_DIR/owned-groups"
    : >"$SUPERVISOR_OWNED_GROUPS"
    setsid env \
        SIPX_BROWSER_AUDIO_INTERNAL=1 \
        SIPX_BROWSER_AUDIO_OWNED_GROUPS_FILE="$SUPERVISOR_OWNED_GROUPS" \
        "$0" "$@" &
    SUPERVISED_PID=$!

    supervisor_cleanup() {
        local pid deadline
        trap - EXIT INT TERM
        kill -TERM -- "-$SUPERVISED_PID" 2>/dev/null || true
        while IFS= read -r pid; do
            [[ $pid =~ ^[0-9]+$ ]] && kill -TERM -- "-$pid" 2>/dev/null || true
        done <"$SUPERVISOR_OWNED_GROUPS"
        deadline=$((SECONDS + 3))
        while (( SECONDS < deadline )) && kill -0 "$SUPERVISED_PID" 2>/dev/null; do
            sleep 0.05 # poll interval: supervised-owner exit is the cleanup condition
        done
        kill -KILL -- "-$SUPERVISED_PID" 2>/dev/null || true
        while IFS= read -r pid; do
            [[ $pid =~ ^[0-9]+$ ]] && kill -KILL -- "-$pid" 2>/dev/null || true
        done <"$SUPERVISOR_OWNED_GROUPS"
        wait "$SUPERVISED_PID" 2>/dev/null || true
        rm -f "$SUPERVISOR_OWNED_GROUPS"
        rmdir "$SUPERVISOR_DIR" 2>/dev/null || true
    }
    trap supervisor_cleanup EXIT INT TERM
    deadline=$((SECONDS + TOTAL_TIMEOUT))
    while kill -0 "$SUPERVISED_PID" 2>/dev/null; do
        if (( SECONDS >= deadline )); then
            printf 'browser-audio proof: complete proof exceeded %ss\n' "$TOTAL_TIMEOUT" >&2
            supervisor_cleanup
            exit 124
        fi
        sleep 0.05 # poll interval: supervised-owner exit is the completion condition
    done
    set +e
    wait "$SUPERVISED_PID"
    SUPERVISED_STATUS=$?
    set -e
    trap - EXIT INT TERM
    rm -f "$SUPERVISOR_OWNED_GROUPS"
    rmdir "$SUPERVISOR_DIR" 2>/dev/null || true
    exit "$SUPERVISED_STATUS"
fi

cleanup() {
    local pid deadline
    ADMIT=false
    trap - EXIT INT TERM
    for pid in "${OWNED_GROUPS[@]}"; do
        kill -TERM -- "-$pid" 2>/dev/null || true
    done
    deadline=$((SECONDS + 3))
    while (( SECONDS < deadline )); do
        local live=false
        for pid in "${OWNED_GROUPS[@]}"; do
            if kill -0 -- "-$pid" 2>/dev/null; then
                live=true
                break
            fi
        done
        "$live" || break
        sleep 0.05 # poll interval: process-group disappearance is the cleanup condition
    done
    for pid in "${OWNED_GROUPS[@]}"; do
        kill -KILL -- "-$pid" 2>/dev/null || true
    done
    for pid in "${LEADERS[@]}"; do
        wait "$pid" 2>/dev/null || true
    done
}
on_signal() {
    cleanup
    exit 124
}
trap cleanup EXIT
trap on_signal INT TERM

die() {
    printf 'browser-audio proof: %s\n' "$*" >&2
    exit 1
}

require_file() {
    [[ -f $2 ]] || die "$1 is not a file: $2"
}

require_executable() {
    [[ -x $2 ]] || die "$1 is not executable: $2"
}

start_group() {
    local stdout=$1 stderr=$2
    shift 2
    "$ADMIT" || die "process admission is closed"
    mkdir -p "$(dirname "$stdout")" "$(dirname "$stderr")"
    setsid bash -c 'ulimit -f "$1"; shift; exec "$@"' _ "$OUTPUT_BLOCKS" "$@" >"$stdout" 2>"$stderr" &
    STARTED_PID=$!
    OWNED_GROUPS+=("$STARTED_PID")
    LEADERS+=("$STARTED_PID")
    if [[ -n ${SIPX_BROWSER_AUDIO_OWNED_GROUPS_FILE:-} ]]; then
        printf '%s\n' "$STARTED_PID" >>"$SIPX_BROWSER_AUDIO_OWNED_GROUPS_FILE"
    fi
}

wait_group() {
    local pid=$1 label=$2 status deadline=$((SECONDS + ROLE_TIMEOUT))
    while kill -0 "$pid" 2>/dev/null; do
        if (( SECONDS >= deadline )); then
            kill -TERM -- "-$pid" 2>/dev/null || true
            die "$label exceeded ${ROLE_TIMEOUT}s"
        fi
        sleep 0.05 # poll interval: leader exit is the completion condition
    done
    set +e
    wait "$pid"
    status=$?
    set -e
    (( status == 0 )) || die "$label exited $status"
}

preflight_identity() {
    "$DRIVER" preflight-cert --cert "$1" --host "$2" --pin "$3"
}

if [[ ${1:-} == --lifecycle-probe ]]; then
    [[ $# == 4 ]] || die "usage: run.sh --lifecycle-probe COMMAND PID_FILE OUTPUT_DIR"
    require_executable "lifecycle probe" "$2"
    mkdir -p "$4"
    start_group "$4/probe.stdout" "$4/probe.stderr" "$2" "$3"
    wait_group "$STARTED_PID" "lifecycle probe"
    exit 0
fi

if [[ ${1:-} == --identity-probe ]]; then
    [[ $# == 6 ]] || die "usage: run.sh --identity-probe CERT HOST PIN COMMAND OUTPUT_DIR"
    require_file "identity certificate" "$2"
    require_executable "identity marker" "$5"
    preflight_identity "$2" "$3" "$4"
    mkdir -p "$6"
    start_group "$6/identity.stdout" "$6/identity.stderr" "$5"
    wait_group "$STARTED_PID" "identity marker"
    exit 0
fi

if [[ ${1:-} == --capture-probe ]]; then
    [[ $# == 3 ]] || die "usage: run.sh --capture-probe COMMAND OUTPUT_DIR"
    require_executable "capture probe" "$2"
    mkdir -p "$3"
    start_group "$3/capture.stdout" "$3/capture.stderr" "$2"
    wait_group "$STARTED_PID" "capture probe"
    exit 0
fi

: "${SIPX_BROWSER_AUDIO_WSS_CA:?set SIPX_BROWSER_AUDIO_WSS_CA}"
: "${SIPX_BROWSER_AUDIO_WSS_CERT:?set SIPX_BROWSER_AUDIO_WSS_CERT}"
: "${SIPX_BROWSER_AUDIO_WSS_KEY:?set SIPX_BROWSER_AUDIO_WSS_KEY}"
: "${SIPX_BROWSER_AUDIO_WSS_SPKI_SHA256:?set SIPX_BROWSER_AUDIO_WSS_SPKI_SHA256}"
: "${SIPX_BROWSER_AUDIO_WEBDRIVER_CMD:?set SIPX_BROWSER_AUDIO_WEBDRIVER_CMD}"
: "${SIPX_BROWSER_AUDIO_PROOF_BIN:?set SIPX_BROWSER_AUDIO_PROOF_BIN}"
: "${SIPX_BROWSER_AUDIO_EVIDENCE_DIR:?set SIPX_BROWSER_AUDIO_EVIDENCE_DIR}"

SIPX_BROWSER_AUDIO_WSS_HOST=${SIPX_BROWSER_AUDIO_WSS_HOST:-localhost}
SIPX_BROWSER_AUDIO_CAPABILITIES_TEMPLATE=${SIPX_BROWSER_AUDIO_CAPABILITIES_TEMPLATE:-$ROOT/tests/browser-audio/config/capabilities.json}
SIPX_BROWSER_AUDIO_OFFERER_CONFIG=${SIPX_BROWSER_AUDIO_OFFERER_CONFIG:-$ROOT/tests/browser-audio/config/browser-offerer.json}
SIPX_BROWSER_AUDIO_ANSWERER_CONFIG=${SIPX_BROWSER_AUDIO_ANSWERER_CONFIG:-$ROOT/tests/browser-audio/config/browser-answerer.json}

require_file "WSS CA" "$SIPX_BROWSER_AUDIO_WSS_CA"
require_file "WSS certificate" "$SIPX_BROWSER_AUDIO_WSS_CERT"
require_file "WSS key" "$SIPX_BROWSER_AUDIO_WSS_KEY"
require_file "browser capabilities" "$SIPX_BROWSER_AUDIO_CAPABILITIES_TEMPLATE"
require_file "offerer config" "$SIPX_BROWSER_AUDIO_OFFERER_CONFIG"
require_file "answerer config" "$SIPX_BROWSER_AUDIO_ANSWERER_CONFIG"
require_executable "WebDriver command" "$SIPX_BROWSER_AUDIO_WEBDRIVER_CMD"
require_executable "sipx proof endpoint" "$SIPX_BROWSER_AUDIO_PROOF_BIN"
preflight_identity "$SIPX_BROWSER_AUDIO_WSS_CERT" "$SIPX_BROWSER_AUDIO_WSS_HOST" "$SIPX_BROWSER_AUDIO_WSS_SPKI_SHA256"

mkdir -p "$SIPX_BROWSER_AUDIO_EVIDENCE_DIR"
CAPABILITIES="$SIPX_BROWSER_AUDIO_EVIDENCE_DIR/capabilities.json"
"$DRIVER" prepare-capabilities \
    --input "$SIPX_BROWSER_AUDIO_CAPABILITIES_TEMPLATE" \
    --output "$CAPABILITIES" \
    --pin "$SIPX_BROWSER_AUDIO_WSS_SPKI_SHA256"
start_group \
    "$SIPX_BROWSER_AUDIO_EVIDENCE_DIR/webdriver.stdout" \
    "$SIPX_BROWSER_AUDIO_EVIDENCE_DIR/webdriver.stderr" \
    "$SIPX_BROWSER_AUDIO_WEBDRIVER_CMD"
WEBDRIVER_PID=$STARTED_PID
WEBDRIVER_URL=${SIPX_BROWSER_AUDIO_WEBDRIVER_URL:-http://127.0.0.1:9515}
"$DRIVER" wait-webdriver --url "$WEBDRIVER_URL" --timeout 10

run_case() {
    local role=$1 case_name=$2 config_template=$3 directory=$4 driver_command=$5
    local product_case=${case_name:-positive}
    mkdir -p "$directory"
    start_group \
        "$directory/sipx.stdout" \
        "$directory/sipx.stderr" \
        "$SIPX_BROWSER_AUDIO_PROOF_BIN" \
        --role "$role" \
        --case "$product_case" \
        --cert "$SIPX_BROWSER_AUDIO_WSS_CERT" \
        --key "$SIPX_BROWSER_AUDIO_WSS_KEY" \
        --result "$directory/sipx.json"
    local sipx_pid=$STARTED_PID
    local address wss_url config
    address=$("$DRIVER" wait-listening --input "$directory/sipx.stdout" --timeout 10)
    wss_url="wss://$SIPX_BROWSER_AUDIO_WSS_HOST:${address##*:}/"
    config="$directory/browser-config.json"
    "$DRIVER" prepare-config \
        --input "$config_template" \
        --output "$config" \
        --role "$role" \
        --wss-url "$wss_url"
    "$DRIVER" preflight-wss \
        --url "$wss_url" \
        --ca "$SIPX_BROWSER_AUDIO_WSS_CA" \
        --pin "$SIPX_BROWSER_AUDIO_WSS_SPKI_SHA256" \
        --timeout 10
    if [[ $driver_command == run-role ]]; then
        "$DRIVER" run-role \
            --url "$WEBDRIVER_URL" \
            --page "$ROOT/tests/browser-audio/peer.html" \
            --config "$config" \
            --capabilities "$CAPABILITIES" \
            --role "$role" \
            --pin "$SIPX_BROWSER_AUDIO_WSS_SPKI_SHA256" \
            --output "$directory/browser.json" \
            --timeout "$ROLE_TIMEOUT"
    else
        "$DRIVER" run-negative \
            --url "$WEBDRIVER_URL" \
            --page "$ROOT/tests/browser-audio/peer.html" \
            --config "$config" \
            --capabilities "$CAPABILITIES" \
            --role "$role" \
            --mutation "$case_name" \
            --pin "$SIPX_BROWSER_AUDIO_WSS_SPKI_SHA256" \
            --output "$directory/browser.json" \
            --timeout "$ROLE_TIMEOUT"
    fi
    wait_group "$sipx_pid" "$role $case_name sipx command"
}

run_positive() {
    local role=$1 config=$2 directory="$SIPX_BROWSER_AUDIO_EVIDENCE_DIR/$1"
    # Empty case argument means run-role receives no mutation flag.
    run_case "$role" "" "$config" "$directory" run-role
}

run_positive browser-offerer "$SIPX_BROWSER_AUDIO_OFFERER_CONFIG"
run_positive browser-answerer "$SIPX_BROWSER_AUDIO_ANSWERER_CONFIG"

mkdir -p "$SIPX_BROWSER_AUDIO_EVIDENCE_DIR/negatives"
for negative in FingerprintMismatch NoNominatedPair WeakerMedia; do
    if [[ $negative == FingerprintMismatch ]]; then
        role=browser-offerer
        config=$SIPX_BROWSER_AUDIO_OFFERER_CONFIG
    else
        role=browser-answerer
        config=$SIPX_BROWSER_AUDIO_ANSWERER_CONFIG
    fi
    negative_directory="$SIPX_BROWSER_AUDIO_EVIDENCE_DIR/negatives/$negative.run"
    run_case "$role" "$negative" "$config" "$negative_directory" run-negative
    "$DRIVER" combine-negative \
        --positive-directory "$SIPX_BROWSER_AUDIO_EVIDENCE_DIR/$role" \
        --browser "$negative_directory/browser.json" \
        --sipx "$negative_directory/sipx.json" \
        --error "$negative" \
        --role "$role" \
        --pin "$SIPX_BROWSER_AUDIO_WSS_SPKI_SHA256" \
        --output "$SIPX_BROWSER_AUDIO_EVIDENCE_DIR/negatives/$negative.json"
done

"$DRIVER" validate-proof \
    --directory "$SIPX_BROWSER_AUDIO_EVIDENCE_DIR" \
    --pin "$SIPX_BROWSER_AUDIO_WSS_SPKI_SHA256" \
    >"$SIPX_BROWSER_AUDIO_EVIDENCE_DIR/proof.json"

# The service should remain alive through all sessions; a vanished process is a failed environment.
kill -0 "$WEBDRIVER_PID" 2>/dev/null || die "WebDriver exited before proof completion"
