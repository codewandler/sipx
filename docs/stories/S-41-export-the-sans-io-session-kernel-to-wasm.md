---
id: S-41
title: Export the sans-I/O session kernel to WebAssembly
pillar: Signalling
status: backlog
priority: 14
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

- [ ] The normalized WASM feature set builds the selected crates without sockets, async runtime,
      filesystem, OS clock or native-only transitive dependencies.
- [ ] Bytes, fired timers, monotonic time and cryptographic entropy cross the ABI defined by A-16;
      malformed host input returns typed errors and no network byte can panic the module.
- [ ] The ABI exposes outbound bytes, timer requests and typed call/session events without leaking
      internal Rust layouts or requiring JavaScript to drive transaction invariants.
- [ ] Cancellation releases handles, pending timers and queued events deterministically; repeated
      create/drop cycles stay inside the specified memory bound.
- [ ] The spec's byte and state vectors run against native Rust and WASM and produce identical events
      and wire bytes.
- [ ] The browser build remains feature-gated and does not change the MSRV or default native package
      graph; all feature combinations and the gate are green.

## Progress

- Backlog. Depends on A-16.
