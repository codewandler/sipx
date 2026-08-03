#!/usr/bin/env bash
# Phase 1: a real host, a scripted document app, and the sipx command-line far end.
set -euo pipefail

root=$(cd "$(dirname "$0")/../../.." && pwd)
work=$(mktemp -d)
pids=()

cleanup() {
    set +e
    for pid in "${pids[@]}"; do
        kill -TERM -- "-$pid" 2>/dev/null
    done
    for pid in "${pids[@]}"; do
        wait "$pid" 2>/dev/null
    done
    rm -rf -- "$work"
}
trap cleanup EXIT INT TERM

timeout 180 cargo build --quiet --manifest-path "$root/Cargo.toml" \
    --package sipx-app --package sipx-cli

mkfifo "$work/webhook-ready"
setsid python3 "$root/crates/sipx-app/tests/scripted_webhook.py" \
    --prompt "$work/prompt.wav" --outcome "$work/outcome.txt" \
    >"$work/webhook-ready" 2>"$work/webhook.log" &
webhook_pid=$!
pids+=("$webhook_pid")
IFS=' ' read -r ready webhook_url <"$work/webhook-ready"
test "$ready" = READY

{
    printf '%s\n' '[listener.edge]'
    printf '%s\n' 'protocol = "sip"' 'transport = "udp"' 'bind = "127.0.0.1:0"' 'app = "greeter"'
    printf '\n%s\n' '[app.greeter]'
    printf '%s\n' 'binding = "webhook"' "url = \"$webhook_url\"" 'signing_secrets = ["hook"]'
    printf '\n%s\n' '[app.greeter.grants]'
    printf 'play_roots = ["%s"]\n' "$work"
    printf '\n%s\n' '[app.greeter.on_failure]'
    printf '%s\n' 'timeout_ms = 1000' 'on_unreachable = { reject = 503 }'
} >"$work/host.toml"

mkfifo "$work/host-ready"
setsid env SIPX_SECRET_hook=test-secret \
    "$root/target/debug/sipx-host" "$work/host.toml" \
    >"$work/host.stdout" 2>"$work/host-ready" &
host_pid=$!
pids+=("$host_pid")
IFS=' ' read -r _ _ _ sip_address _ <"$work/host-ready"
sip_address=${sip_address%\(*}

timeout 20 "$root/target/debug/sipx" dial "sip:menu@$sip_address" \
    --dtmf 5 --duration 5 --record "$work/heard.wav" --json >"$work/call.json"
test "$(tr -d '\n' <"$work/outcome.txt")" = 5
python3 - "$work/heard.wav" <<'PY'
import sys, wave
with wave.open(sys.argv[1], "rb") as recording:
    assert recording.getnframes() > 0, "the far end heard no prompt"
PY

kill -TERM -- "-$webhook_pid"
wait "$webhook_pid" 2>/dev/null || true
pids=("$host_pid")

set +e
timeout 20 "$root/target/debug/sipx" dial "sip:menu@$sip_address" \
    --timeout 5 --duration 1 --json >"$work/unreachable.json" 2>&1
status=$?
set -e
test "$status" -eq 3
grep -Eq '503|Service Unavailable' "$work/unreachable.json"

printf '%s\n' 'document-mode shell proof passed'
