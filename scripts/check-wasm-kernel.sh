#!/usr/bin/env bash
# Prove the browser kernel: that it compiles for WebAssembly without an operating system under it,
# that it behaves identically there and natively, and that the shipped module is the shape
# `docs/specs/browser-sdk.md` §4 promises.
#
# Four separate claims, because three of them can hold while the fourth fails:
#
#   1. `sipx-sip` and `sipx-sdp` build for `wasm32-unknown-unknown` with their default features
#      off. This is the one that catches a new dependency reaching an operating-system entropy
#      source, a clock or a socket — the failure mode is a `getrandom` build error, and it is the
#      reason the `identity` and `sdes-keys` feature seams exist.
#   2. The §9 vector suite passes natively.
#   3. The **same** suite passes compiled to WebAssembly and run under a WebAssembly runtime, so
#      "identical events and wire bytes" is a comparison rather than two separate assertions.
#      `wasm32-wasip1` is used for this and only this: a test harness needs to print, and the
#      shipped module (claim 4) imports nothing at all.
#   4. The shipped `wasm32-unknown-unknown` module has the §4.3 exports, imports nothing (§4.1),
#      declares a 32 MiB maximum linear memory, and can be driven end to end through linear memory
#      with no glue.
#
# `S-41` owns all four. Not yet a `gate.py` step — see the story's Progress note.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

export RUSTFLAGS="${RUSTFLAGS:--D warnings}"
status=0

step() {
    printf '  %-46s ' "$1"
}

require() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "wasm-kernel: $1 is required and was not found" >&2
        # Exit 2, not 1: a missing tool means the run is incomplete, not that the tree has a
        # finding. The gate makes the same distinction.
        exit 2
    fi
}

require node
require wasmtime

for target in wasm32-unknown-unknown wasm32-wasip1; do
    if ! rustup target list --installed 2>/dev/null | grep -qx "$target"; then
        echo "wasm-kernel: the $target target is not installed; run: rustup target add $target" >&2
        exit 2
    fi
done

# 1. The normalized WASM feature set.
step "core crates build for wasm32-unknown-unknown"
if cargo build --quiet -p sipx-sip -p sipx-sdp -p sipx-wasm \
    --no-default-features --target wasm32-unknown-unknown; then
    echo "ok"
else
    echo "FAILED"
    status=1
fi

# The claim is not that it builds. A kernel that built while still resolving an entropy source
# would compile perfectly and violate §4.7's no-fallback rule, so the assertion is on the resolved
# graph rather than on the exit code of a build.
step "the kernel resolves no entropy source"
if cargo tree --quiet -p sipx-wasm --edges normal --prefix none 2>/dev/null |
    grep -qE '^(getrandom|rand|tokio|mio|socket2) '; then
    echo "FAILED"
    cargo tree -p sipx-wasm --edges normal --prefix none 2>/dev/null |
        grep -E '^(getrandom|rand|tokio|mio|socket2) ' | sed 's/^/      /'
    status=1
else
    echo "ok"
fi

# 2 and 3. The same suite, both targets.
step "vectors pass natively"
if cargo test --quiet -p sipx-wasm --all-features >/dev/null 2>&1; then
    echo "ok"
else
    echo "FAILED"
    cargo test -p sipx-wasm --all-features 2>&1 | tail -40 | sed 's/^/      /'
    status=1
fi

step "the same vectors pass in WebAssembly"
if CARGO_TARGET_WASM32_WASIP1_RUNNER=wasmtime \
    cargo test --quiet -p sipx-wasm --target wasm32-wasip1 >/dev/null 2>&1; then
    echo "ok"
else
    echo "FAILED"
    CARGO_TARGET_WASM32_WASIP1_RUNNER=wasmtime \
        cargo test -p sipx-wasm --target wasm32-wasip1 2>&1 | tail -40 | sed 's/^/      /'
    status=1
fi

# 4. The shipped artifact.
#
# `--max-memory` is a link argument rather than a source attribute because the maximum belongs to
# the module, not to the Rust: §4.1 declares 32 MiB, and 33554432 bytes is 512 pages.
step "the browser module builds"
module="wasm/target/wasm32-unknown-unknown/release/sipx_browser_wasm.wasm"
if (cd wasm && RUSTFLAGS="-C link-arg=--max-memory=33554432" \
    cargo build --quiet --release --target wasm32-unknown-unknown); then
    echo "ok"
else
    echo "FAILED"
    status=1
fi

if [ -f "$module" ]; then
    step "the module answers to §4"
    if output="$(node wasm/harness.mjs "$module" 2>&1)"; then
        echo "ok"
    else
        echo "FAILED"
        printf '%s\n' "$output" | sed 's/^/      /'
        status=1
    fi
else
    step "the module answers to §4"
    echo "SKIPPED (no module was built)"
    status=1
fi

if [ "$status" -eq 0 ]; then
    echo "wasm kernel: the browser build is identical to the native one"
fi
exit "$status"
