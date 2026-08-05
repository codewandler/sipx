---
id: A-16
title: Specify the browser SDK contract
pillar: Application
status: backlog
priority: 13
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

- [ ] A normative spec distinguishes the browser SDK from `A-3`'s server-side application SDK and
      defines registration, dial, answer, hangup, call events and negotiated-media reporting.
- [ ] The architecture keeps SIP, SDP, transactions and dialogs in a sans-I/O WASM kernel while the
      host supplies bytes, fired timers, monotonic time and cryptographic entropy.
- [ ] The browser owns WebSocket/WSS, `RTCPeerConnection`, ICE, DTLS-SRTP, capture and render. The
      spec explicitly refuses video, data channels, SCTP and a WebRTC engine implemented in Rust.
- [ ] ABI types, ownership, cancellation, reentrancy, callback ordering, memory limits and error
      mapping are specified with state tables and vectors before code.
- [ ] The package names, generated-versus-handwritten boundary, supported browser policy and semantic
      versioning promise are recorded without claiming stable 1.0 APIs.
- [ ] Threat analysis covers untrusted SIP bytes, hostile SDP, script-visible credentials, entropy,
      wrong fingerprints, insecure signalling, cross-origin isolation and leaked media tracks.
- [ ] The current vision and public fit boundary are updated only if the accepted contract would
      actually contradict them; otherwise the native-WebRTC boundary is stated explicitly.
- [ ] `./scripts/gate.py` is green.

## Progress

- Backlog. First M15 story; this milestone is tracked but not selected for the current wave.
