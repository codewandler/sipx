---
id: M-49
title: Negotiate a fail-closed browser-audio profile
pillar: Media
status: backlog
priority: 7
design: docs/specs/webrtc-audio.md
epic: webrtc-audio
areas: [sipx-sdp, sipx-call, sipx-media, beta4]
predicate:
announcement: 3
note: after M-48 and M-46 · one named profile owns the complete SDP vocabulary and never downgrades silently
---

# Negotiate a fail-closed browser-audio profile

## Goal

Add one named call profile that offers and answers the complete browser-audio SDP contract and
returns a typed refusal instead of silently falling back when any mandatory element is absent.

## Acceptance

- [ ] Failing-first vectors from `M-48` drive offer and answer generation and parsing for
      `UDP/TLS/RTP/SAVPF`, RTCP multiplexing, ICE, DTLS fingerprint/setup and the RFC 7874 audio
      vocabulary.
- [ ] Codec and build-feature policy is explicit: the profile requires Opus support and reports a
      typed pre-I/O setup error when the build cannot provide it.
- [ ] An answer missing multiplexing, nomination prerequisites, fingerprint, a compatible setup
      role or an allowed codec is refused before media keys or sockets change state.
- [ ] A peer cannot select SDES or plain RTP beneath this profile, even when those modes are enabled
      elsewhere; a downgrade attempt has a named negative test.
- [ ] Re-offer, answer and ICE-restart state preserve the negotiated profile or fail explicitly;
      they cannot fall back merely because the initial call already established.
- [ ] The profile is reachable through the call API and diagnostic CLI with negotiated codec,
      keying and candidate facts in terminal and JSON results.
- [ ] RFC registry and API/reference documentation are updated with the implementation.
- [ ] `./scripts/gate.py` green.

## Progress

- Blocked on `M-48` and `M-46`.

## Notes

- Runtime packet ownership belongs to `M-50`; this story proves policy and state transitions
  without claiming that a shared component carries media yet.
