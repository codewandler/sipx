#!/usr/bin/env bash
# Measure DSP processor heap growth against the `state_bytes` each processor declares:
# `docs/specs/custom-call-dsp.md` §9.1, and the heap half of `DSP-K9` (`X-128`).
#
# Two claims, and the second is the one that needed a mechanism nobody had:
#
#   1. Every built-in's peak live heap is within its declared `state_bytes` less its inline size.
#      Eight of the nine own no heap and measure exactly zero; `sipx.stutter` owns a delay line and
#      measures it to the byte, which is the first time that declaration has been checked rather
#      than trusted.
#   2. No processor allocates after `prepare` returned — §9.1's "not per frame, not per position,
#      not per observation". Nothing checked this at all before.
#
# And the harness is shown failing, per §11.3: `HeapHog` allocates per frame and `HeapLiar` owns a
# buffer it did not declare, and the run fails if either goes uncaught.
#
# Why this is a script and not `cargo test`: counting allocations needs `unsafe impl GlobalAlloc`,
# and `unsafe_code = "forbid"` covers every target of every workspace member — tests included, with
# no `allow` able to override it. `heap-probe/` therefore lives outside the workspace the way
# `wasm/` and `fuzz/` do. `heap-probe/src/main.rs` carries the full argument.
#
# `X-128` owns this. Not yet a `gate.py` step — see the story's Progress note.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root/heap-probe"

# `-D warnings` including the deliberate `unsafe`: the package sets `unsafe_code = "warn"` and the
# one allocator impl carries a scoped `#[allow]`, so a second `unsafe` anywhere in this binary
# fails the run rather than blending into the output.
export RUSTFLAGS="${RUSTFLAGS:--D warnings}"

echo "dsp-heap: measuring processor heap against declared state_bytes"
cargo run --quiet
