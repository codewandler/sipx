---
id: M-51
title: Prove browser audio against an independent endpoint
pillar: Media
status: done
design: docs/specs/webrtc-audio.md
epic: webrtc-audio
areas: [interop, ci, website, sipx-cli, beta4]
predicate:
announcement: 5
note: beta.4 product proof · both SIP roles, non-silent Opus both ways, immediate fingerprint and downgrade negatives
---

# Prove browser audio against an independent endpoint

## Goal

Demonstrate the complete browser-audio profile at the public product boundary against an
independently implemented browser SIP endpoint, and publish exactly the boundary that proof earns.

## Acceptance

- [x] A bounded shell proof runs in CI in both call roles over WSS + ICE + DTLS-SRTP + multiplexed
      RTP/RTCP + Opus and carries non-silent audio in both directions.
- [x] The proof reports the negotiated codec, DTLS-SRTP profile, setup role and nominated candidate
      pair, and asserts those facts rather than inferring success from process exit alone.
- [x] Wrong-fingerprint, missing-nomination and weaker-answer negatives fail immediately and
      non-vacuously at the layer named by the profile; the peer is shown capable of succeeding in
      the paired positive case.
- [x] The peer workload and every background process are bounded and cancellation-safe, with one
      cleanup path that terminates and waits for the entire process group.
- [x] Public fit, security, getting-started and comparison pages describe the proven host or
      server-reflexive audio path and continue to exclude TURN-required networks, video, browser
      APIs, data channels and a general WebRTC stack.
- [x] RFC registry evidence and the public development narrative name the independent proof without
      turning the peer implementation into design authority.
- [x] `M-38` closes only when this proof and every preceding child are done.
- [x] `./scripts/gate.py` green with the independent-peer job represented in the local gate contract
      or `NOT_RUN_LOCALLY` with a precise reason.

## Progress

- The public-API sipx proof endpoint, independent native-browser page, WebDriver controller,
  process-group-safe runner, CI job, structured evidence validator, three browser-driven negatives,
  RFC evidence and public boundary pages are wired. Ten adversarial harness tests now cover
  identity, completeness, fact reversal, bounded output and both timeout and normal-exit cleanup.
  The complete local browser proof passed both positive roles and all three negatives. The hosted
  native-browser job then passed at commit `7258f1f` in workflow run `30947782300`, job
  `92121949350`, using the runner's matched browser and WebDriver. The complete local implementation
  gate also passed every substantive implementation, feature, MSRV, packaging and documentation
  step; its only pre-closure finding was the expected generated maturity-page drift resolved while
  closing this wave.
- Reopened after exact release-candidate workflow run `30950648054`, job `92131573628`. Under
  hosted-runner contention the sipx role stopped waiting for its first browser method at ten
  seconds even though the harness still owned a two-minute role budget. The browser then reported
  `open exceeded 10000 ms` because its WSS peer had already exited. The correction must make first
  method arrival the readiness condition under the enclosing operation bound; widening another
  independent startup timer is not sufficient. A paused-time regression failed first with an
  eleven-second browser start, then passed after the separate timer was removed. The complete local
  gate passed 36 of 36 steps, and hosted workflow run `30951724369`, job `92135156289`, completed
  the native-browser proof on repair commit `f40eadd`.

## Notes

- This story owns compatibility evidence, not implementation. A two-sipx call from `M-50` cannot
  satisfy it.
