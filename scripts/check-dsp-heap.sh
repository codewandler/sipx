#!/usr/bin/env bash
# Measure DSP processor heap growth against the `state_bytes` each processor declares:
# `docs/specs/custom-call-dsp.md` §9.1, and the heap half of `DSP-K9` (`X-128`).
#
# Three claims, and the second is the one that needed a mechanism nobody had:
#
#   1. Every processor's peak live heap is within its declared `state_bytes` less its inline size.
#      Nine of the ten own no heap and measure exactly zero; `sipx.stutter` owns a delay line and
#      measures it to the byte, which is the first time that declaration has been checked rather
#      than trusted.
#   2. No processor allocates after `prepare` returned — §9.1's "not per frame, not per position,
#      not per observation". Nothing checked this at all before.
#   3. Every identifier in `BUILT_IN_IDS` and `NOISE_REDUCTION_IDS` appears in the run. `X-128`
#      wrote the call list by hand, `M-66` then shipped `sipx.subband_suppressor`, and the one
#      processor with adaptive state went unmeasured for two stories without the gate noticing.
#      `X-109` added the reducer and this check, so the next omission is a red run.
#
# And the harness is shown failing, per §11.3: `HeapHog` allocates per frame and `HeapLiar` owns a
# buffer it did not declare, and the run fails if either goes uncaught.
#
# Why this is a script and not `cargo test`: counting allocations needs `unsafe impl GlobalAlloc`,
# and `unsafe_code = "forbid"` covers every target of every workspace member — tests included, with
# no `allow` able to override it. `heap-probe/` therefore lives outside the workspace the way
# `wasm/` and `fuzz/` do. `heap-probe/src/main.rs` carries the full argument.
#
# `X-128` built this; `X-142` registered it as the `dsp heap` gate step and its own CI job. Cold on
# a warm sccache it costs 3 s and 74 MB of its own `target/`; warm it is under a second.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# An absent toolchain is an incomplete run, not a finding about the code — the distinction
# `gate.py` draws between exit 2 and exit 1, and the one `check-wasm-kernel.sh` already makes.
# Saying "the processors leak" because cargo is missing would be the worst kind of wrong answer.
if ! command -v cargo > /dev/null; then
    echo "dsp-heap: cargo is not installed, so nothing was measured" >&2
    echo "  this run proved nothing about the tree; it is not a finding about any processor" >&2
    exit 2
fi

cd "$root/heap-probe"

# `-D warnings` including the deliberate `unsafe`: the package sets `unsafe_code = "warn"` and the
# one allocator impl carries a scoped `#[allow]`, so a second `unsafe` anywhere in this binary
# fails the run rather than blending into the output.
export RUSTFLAGS="${RUSTFLAGS:--D warnings}"

echo "dsp-heap: measuring processor heap against declared state_bytes"
cargo run --quiet
