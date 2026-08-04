---
id: M-51
title: Prove browser audio against an independent endpoint
pillar: Media
status: in-progress
priority: 9
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

- [ ] A bounded shell proof runs in CI in both call roles over WSS + ICE + DTLS-SRTP + multiplexed
      RTP/RTCP + Opus and carries non-silent audio in both directions.
- [ ] The proof reports the negotiated codec, DTLS-SRTP profile, setup role and nominated candidate
      pair, and asserts those facts rather than inferring success from process exit alone.
- [ ] Wrong-fingerprint, missing-nomination and weaker-answer negatives fail immediately and
      non-vacuously at the layer named by the profile; the peer is shown capable of succeeding in
      the paired positive case.
- [ ] The peer workload and every background process are bounded and cancellation-safe, with one
      cleanup path that terminates and waits for the entire process group.
- [ ] Public fit, security, getting-started and comparison pages describe the proven host or
      server-reflexive audio path and continue to exclude TURN-required networks, video, browser
      APIs, data channels and a general WebRTC stack.
- [ ] RFC registry evidence and the public development narrative name the independent proof without
      turning the peer implementation into design authority.
- [ ] `M-38` closes only when this proof and every preceding child are done.
- [ ] `./scripts/gate.py` green with the independent-peer job represented in the local gate contract
      or `NOT_RUN_LOCALLY` with a precise reason.

## Progress

- The public-API sipx proof endpoint, independent native-browser page, WebDriver controller,
  process-group-safe runner, CI job, structured evidence validator, three browser-driven negatives,
  RFC evidence and public boundary pages are wired. Ten adversarial harness tests now cover
  identity, completeness, fact reversal, bounded output and both timeout and normal-exit cleanup.
  The story remains in progress until the hosted runner's matched browser and WebDriver complete
  both positive roles and all three negatives; local ChromeDriver discovery selected a mismatched
  browser binary and therefore cannot substitute for that result.

## Notes

- This story owns compatibility evidence, not implementation. A two-sipx call from `M-50` cannot
  satisfy it.
