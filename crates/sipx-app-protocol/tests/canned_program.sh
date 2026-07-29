#!/usr/bin/env bash
#
# The epic's end-to-end proof: run the interpreter over a real call and assert the outcome.
#
#   crates/sipx-app-protocol/tests/canned_program.sh
#
# `examples/canned_program.rs` places a real SIP call between two loopback endpoints and drives
# the callee entirely from `sipx.app.v1` instructions — answer, play, gather, hang up — with no
# host and nothing on the wire but this workspace. This script is what turns that from a demo into
# a check: it asserts the trace the run must produce, in order, and fails if any line is missing.
#
# It is a shell script rather than a `#[test]` on purpose. The claim being made is that the
# contract is demonstrable *from a shell*, by somebody who has cloned the repository and has no
# host, no app server and no account anywhere.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$here/../../.."

log="$(mktemp -t canned_program.XXXXXX)"
trap 'rm -f "$log"' EXIT

echo "running the canned program over a real call..."
if ! timeout 120 cargo run --quiet --package sipx-app-protocol --features call \
    --example canned_program >"$log" 2>&1; then
    echo "FAIL: the example did not finish cleanly" >&2
    cat "$log" >&2
    exit 1
fi

# Every line the run must produce, in the order it must produce them. `expect` walks the log
# forward, so this asserts a sequence and not merely a set: a hang-up before the gather resolved
# would pass a set check and is exactly the failure worth catching.
remaining="$(cat "$log")"
expect() {
    local want="$1"
    local before="$remaining"
    remaining="$(printf '%s\n' "$before" | sed -n "/$want/,\$p" | tail -n +2)"
    if [ "$remaining" = "$before" ] || ! printf '%s\n' "$before" | grep -q "$want"; then
        echo "FAIL: expected /$want/ after the lines already matched" >&2
        echo "--- full trace ---" >&2
        cat "$log" >&2
        exit 1
    fi
    echo "  ok: $want"
}

# The call arrives and the app is asked what to do about it (§5.1: `seq` starts at 1).
expect 'deliver seq=1 event=call.incoming'
# answer → play → gather → hang up, which is the canned program in the order it was written.
expect 'effect answer'
expect 'call said call.answered'
expect 'effect play p1'
expect 'call said call.playback.finished'
# Real RFC 4733 keypresses, collected by the interpreter rather than by the driver.
expect 'call said call.dtmf 4'
expect 'call said call.dtmf 2'
# §5.3: the completion event names the app's own instruction, and carries what was collected.
expect 'deliver seq=[0-9]* event=call.gather.finished'
expect 'gather digits=42 reason=max'
# And the program's last instruction ends the call.
expect 'effect hangup cause=hangup'
expect 'ended cause=hangup'
expect 'OK'

echo "PASS: the contract ran end to end over a real call, with no host"
