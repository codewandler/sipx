---
id: M-49
title: Negotiate a fail-closed browser-audio profile
pillar: Media
status: done
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

- [x] Failing-first vectors from `M-48` drive offer and answer generation and parsing for
      `UDP/TLS/RTP/SAVPF`, RTCP multiplexing, ICE, DTLS fingerprint/setup and the RFC 7874 audio
      vocabulary.
- [x] Codec and build-feature policy is explicit: the profile requires Opus support and reports a
      typed pre-I/O setup error when the build cannot provide it.
- [x] An answer missing multiplexing, nomination prerequisites, fingerprint, a compatible setup
      role or an allowed codec is refused before media keys or sockets change state.
- [x] A peer cannot select SDES or plain RTP beneath this profile, even when those modes are enabled
      elsewhere; a downgrade attempt has a named negative test.
- [x] Re-offer, answer and ICE-restart state preserve the negotiated profile or fail explicitly;
      they cannot fall back merely because the initial call already established.
- [x] The profile is reachable through the call API and diagnostic CLI with negotiated codec,
      keying and candidate facts in terminal and JSON results.
- [x] RFC registry and API/reference documentation are updated with the implementation.
- [x] `./scripts/gate.py` green.

## Progress

- Implementation complete pending the full repository gate. The pure SDP boundary holds O1/A1,
  every named negative, restart preservation, a byte-pinned completed native-browser offer and the
  browser answer's RFC 4733 default. `MediaPolicy::browser_audio()` reaches both call/CLI roles,
  hands the retained component to `M-50`'s ICE-before-DTLS owner, and reports the nominated facts
  from the running session. The offerer validates the complete offer/answer relation before generic
  codec settlement or ICE description acceptance, including the offered payload order and every
  candidate line; malformed, peer-reflexive and relayed extras cannot hide beside a valid host.
  Reliable-provisional media entry points are pinned to a typed pre-I/O refusal, including an
  attempted weaker-keying override. The two-sipx WSS process test proves composition; independent
  endpoint proof remains `M-51` and is not claimed here.

## Notes

- Runtime packet ownership belongs to `M-50`; this story proves policy and state transitions
  without claiming that a shared component carries media yet.
