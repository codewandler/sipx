---
id: A-16
title: Specify the browser SDK contract
pillar: Application
status: done
priority:
design: docs/designs/browser-sdk.md
epic: browser-sdk
areas: [browser, wasm, javascript, m15]
predicate:
announcement:
note: M15 admission and spec gate · audio-only WASM SIP with browser-owned WebRTC
---

# Specify the browser SDK contract

## Goal

Define the WebAssembly ABI, JavaScript lifecycle, browser support and security boundary before any
browser package or demo turns an accidental interface into a promise.

## Acceptance

- [x] A normative spec distinguishes the browser SDK from `A-3`'s server-side application SDK and
      defines registration, dial, answer, hangup, call events and negotiated-media reporting.
- [x] The architecture keeps SIP, SDP, transactions and dialogs in a sans-I/O WASM kernel while the
      host supplies bytes, fired timers, monotonic time and cryptographic entropy.
- [x] The browser owns WebSocket/WSS, `RTCPeerConnection`, ICE, DTLS-SRTP, capture and render. The
      spec explicitly refuses video, data channels, SCTP and a WebRTC engine implemented in Rust.
- [x] ABI types, ownership, cancellation, reentrancy, callback ordering, memory limits and error
      mapping are specified with state tables and vectors before code.
- [x] The package names, generated-versus-handwritten boundary, supported browser policy and semantic
      versioning promise are recorded without claiming stable 1.0 APIs.
- [x] Threat analysis covers untrusted SIP bytes, hostile SDP, script-visible credentials, entropy,
      wrong fingerprints, insecure signalling, cross-origin isolation and leaked media tracks.
- [x] The current vision and public fit boundary are updated only if the accepted contract would
      actually contradict them; otherwise the native-WebRTC boundary is stated explicitly.
- [x] `./scripts/gate.py` is green.

## Progress

- The normative contract is [`docs/specs/browser-sdk.md`](../specs/browser-sdk.md) —
  `sipx.browser.v1`: kernel ABI (§4), control-plane vocabulary and call/registration state tables
  (§5), JavaScript lifecycle with the combined `established` gate (§6), package and
  supported-browser policy (§7), threat analysis (§8), and `BSDK-*` byte/state vectors (§9).
- `docs/vision.md` is deliberately unchanged: the contract keeps the media engine in the browser,
  so the vision's WebRTC non-goal is stated in the spec (§1.1) rather than amended.
- The design doc now names the spec as the contract of record. Implementation is `S-41`, `T-33`,
  `M-52`, `A-17`, `A-18`, `X-100`, which cite the spec's vector IDs.
- 2026-08-08: **already delivered; closed without re-implementation.** `docs/specs/browser-sdk.md`
  (834 lines) was written by `3686d03` on 2026-08-05, which ticked seven of eight acceptance rows
  and wrote this log — but never ran `/track:done`, so the status stayed `backlog`. The rc.5
  wave-selection pass ranked on the `status:` field alone and re-promoted it to `ready`, and it was
  dispatched to an implementor, who correctly refused to write a second spec: §11's evidence map
  binds `S-41`, `T-33`, `M-52`, `A-17`, `A-18` and `X-100` to specific section and `BSDK-*` vector
  IDs, and the spec itself forbids a child story restating its boundaries. The remaining gate row is
  satisfied — the spec is documentation-only and `./scripts/gate.py` has run green since, including
  37 steps at `3b22cec` for the rc.3 cut.

