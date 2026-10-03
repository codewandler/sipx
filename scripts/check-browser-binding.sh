#!/usr/bin/env bash
# Prove the browser signalling binding: that it drives the session kernel over the browser's
# WebSocket API under the bounds `docs/specs/browser-signalling.md` states, and that it does so
# against the module that actually ships rather than only against a fake.
#
# Two claims, and the second is not implied by the first:
#
#   1. The binding's decisions are the specified ones — WSS by default, cleartext only under the
#      development policy and only to loopback, three bounded queues, a reconnect budget that ends
#      typed, timers that re-enter only as fired-timer inputs, and a cancellation that leaves no
#      timer, handler or subscription behind. Every one of these is driven through injected fakes,
#      so each is decided at an exact instant and no case waits on wall-clock time.
#   2. Those decisions still hold with the compiled `sipx_browser.wasm` underneath them. A fake
#      kernel agrees with whatever the case scripted; only the real one can say that the bytes the
#      binding writes are the kernel's serialisation, and that "the peer disagrees about where
#      messages end" is the real parser's verdict.
#
# `T-33` owns both. The module is built here rather than assumed: `scripts/check-wasm-kernel.sh`
# builds the same artifact with the same flags into the same directory, so whichever runs second
# pays nothing for it.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

status=0

step() {
    printf '  %-46s ' "$1"
}

require() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "browser-binding: $1 is required and was not found" >&2
        # Exit 2, not 1: a missing tool means the run is incomplete, not that the tree has a
        # finding. `scripts/check-wasm-kernel.sh` and the gate make the same distinction.
        exit 2
    fi
}

require node
require python3
python3 scripts/generate-browser.py --check
python3 scripts/test-browser-generation.py

if ! rustup target list --installed 2>/dev/null | grep -qx wasm32-unknown-unknown; then
    echo "browser-binding: the wasm32-unknown-unknown target is not installed;" \
        "run: rustup target add wasm32-unknown-unknown" >&2
    exit 2
fi

# `--max-memory` is a link argument rather than a source attribute because the maximum belongs to
# the module: `docs/specs/browser-sdk.md` §4.1 declares 32 MiB, and 33554432 bytes is 512 pages.
step "the browser module builds"
module="wasm/target/wasm32-unknown-unknown/release/sipx_browser_wasm.wasm"
if (cd wasm && RUSTFLAGS="-C link-arg=--max-memory=33554432" \
    cargo build --quiet --release --target wasm32-unknown-unknown); then
    echo "ok"
else
    echo "FAILED"
    status=1
fi

step "the binding answers to its contract"
if [ -f "$module" ]; then
    if output="$(node browser/test/run.mjs --module "$module" 2>&1)"; then
        echo "ok"
    else
        echo "FAILED"
        printf '%s\n' "$output" | sed 's/^/      /'
        status=1
    fi
else
    # Not a skip. Without the module the compiled-kernel cases do not run, and a run that quietly
    # checked half of what it names is the failure this script exists to prevent.
    echo "FAILED (no module was built)"
    status=1
fi

step "the native media adapter holds its contract"
if node --test browser/test/media.test.mjs browser/test/client.test.mjs browser/test/package-contract.test.mjs browser/test/command-errors.test.mjs; then
    echo "ok"
else
    echo "FAILED"
    status=1
fi

if [ "$status" -eq 0 ]; then
    echo "browser binding: the signalling transport holds against the shipped kernel"
fi
exit "$status"
