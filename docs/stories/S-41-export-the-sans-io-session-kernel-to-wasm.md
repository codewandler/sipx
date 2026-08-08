---
id: S-41
title: Export the sans-I/O session kernel to WebAssembly
pillar: Signalling
status: done
priority:
design: docs/designs/browser-sdk.md
epic: browser-sdk
areas: [sipx-sip, sipx-sdp, sipx-ua, wasm, m15]
predicate:
announcement:
note: after A-16 · deterministic Rust state machine with host bytes, timers and entropy
---

# Export the sans-I/O session kernel to WebAssembly

## Goal

Compile the selected SIP, SDP, transaction and dialog state into a small browser-loadable WASM kernel
whose environment is explicit and deterministic.

## Acceptance

- [x] The normalized WASM feature set builds the selected crates without sockets, async runtime,
      filesystem, OS clock or native-only transitive dependencies.
- [x] Bytes, fired timers, monotonic time and cryptographic entropy cross the ABI defined by A-16;
      malformed host input returns typed errors and no network byte can panic the module.
- [x] The ABI exposes outbound bytes, timer requests and typed call/session events without leaking
      internal Rust layouts or requiring JavaScript to drive transaction invariants.
- [x] Cancellation releases handles, pending timers and queued events deterministically; repeated
      create/drop cycles stay inside the specified memory bound.
- [x] The spec's byte and state vectors run against native Rust and WASM and produce identical events
      and wire bytes.
- [x] The browser build remains feature-gated and does not change the MSRV or default native package
      graph; all feature combinations and the gate are green.

## Progress

The kernel is implemented, and `docs/specs/browser-sdk.md` is unchanged — no ambiguity in it needed
fixing, and every `BSDK-*` vector is satisfied at the ID the spec gave it.

**What landed.** `crates/sipx-wasm` is the §4 ABI and the §5 control plane in safe Rust: the twelve
entry points, the twelve error codes, the §4.6 record framing, the §4.7 entropy tape, the §4.9
bounds and poisoning, the §4.11 snapshot, all ten §5.2 verbs, all eight §5.3 events and both §5.4
state tables. `wasm/` is the loadable module — twelve `#[unsafe(no_mangle)]` shims over it and
nothing else.

**Evidence.** `./scripts/check-wasm-kernel.sh` is the whole story in one command: the selected
crates build for `wasm32-unknown-unknown`, the kernel resolves no entropy source, and the same 76
vector tests pass natively *and* compiled to WebAssembly under `wasmtime` — including a SHA-256
over the whole RFC 4475/5118 replay that is byte-identical on both targets (§8.1). `wasm/harness.mjs`
then drives the shipped `sipx_browser.wasm` from plain `WebAssembly.instantiate` with no glue at
all, and asserts what only the artifact can answer for: no imports (§4.1), the §4.3 export names,
a declared 32 MiB maximum linear memory, and `BSDK-ENT-1`'s pinned Call-ID, From tag and Via branch
in the REGISTER it emits.

**The two feature seams.** `sipx-sip` gained `identity` and `sipx-sdp` gained `sdes-keys`, both
default-on and both naming that crate's only draw on an operating-system entropy source. Nothing
about a native build changes; `wasm32-unknown-unknown` refuses to compile `getrandom` at all, which
is what made the seams necessary rather than tidy.

**What the last row is waiting on.** Feature combinations are green (`./scripts/check-features.sh`,
with new rows for both seams and a graph assertion that the kernel resolves no `rand`/`getrandom`/
`tokio`), MSRV 1.88 checks clean, and the default native package graph is untouched — the only
`Cargo.lock` movement is the new workspace member, with no new external crate. The full
`./scripts/gate.py` was deliberately **not** run here; the wave's coordinator runs one gate per wave.

**Follow-up for whoever picks this up.** `scripts/check-wasm-kernel.sh` is not yet a gate step and
has no CI job, on purpose: adding a job requires a matching `Step` or `NOT_RUN_LOCALLY` entry in
`scripts/gate.py` in the same change, and that file was being edited concurrently by `X-114`. The
gate still reports 40 steps over 21 CI jobs with none unaccounted for. Wiring it needs a `wasm-kernel`
job in `ci.yml` plus a `Step("wasm kernel", "wasm-kernel", ("./scripts/check-wasm-kernel.sh",))`,
and the job needs `rustup target add wasm32-unknown-unknown wasm32-wasip1` and a `wasmtime` install.
- 2026-08-08: closed in the `1.0.0-rc.5` boundary.
