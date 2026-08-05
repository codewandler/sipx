---
id: X-100
title: Prove the packaged browser SDK
pillar: Build
status: backlog
priority: 19
design: docs/designs/browser-sdk.md
epic: browser-sdk
areas: [browser, ci, package, website, m15]
predicate:
announcement:
note: M15 exit · clean consumer, supported browser matrix, both SIP roles and fail-closed negatives
---

# Prove the packaged browser SDK

## Goal

Make the installable artifact and public demo—not workspace source—the subject of a bounded browser
matrix that carries the M15 product claim.

## Acceptance

- [ ] CI installs the exact packed JavaScript artifact and WASM into a clean consumer and serves the
      exact built demo artifact; no test resolves a workspace source import.
- [ ] Every browser in A-16's support policy registers over WSS and completes non-silent Opus audio
      in caller and answerer roles with deterministic fake-media signals.
- [ ] Assertions cover selected codec, RTCP multiplexing, nominated ICE pair, DTLS role, verified
      fingerprint, track cleanup and zero residual socket/timer/session handles.
- [ ] Independent negative cases reverse wrong fingerprint, insecure signalling, missing nomination,
      weaker media, over-limit signalling and cancellation during setup; each fails for the named
      reason before a connected event.
- [ ] Runs have finite setup, call, teardown and suite deadlines; browser processes are supervised as
      a process group and always awaited after cleanup.
- [ ] Package hashes, browser versions and demo SHA are retained as artifacts and the public support
      statement is generated from the same matrix.
- [ ] The complete release gate is green; external package publication remains a separate authorized
      release action.

## Progress

- Backlog. Final M15 story after A-18.
