---
id: M-70
title: "Accept multiplexed browser offers with unused component candidates"
pillar: "Media"
status: in-progress
epic: media-interoperability
areas: [sipx-sdp, sipx-media]
design: docs/designs/media-interoperability.md
note: "external review finding 9 · a second browser engine reaches SDP then fails the multiplexed profile"
---

# Accept multiplexed browser offers with unused component candidates

## Goal

Accept an otherwise valid multiplexed browser-audio offer when it includes candidates for the
unused RTCP component. Keep one runtime component: component-two candidates are not nominated,
bound or interpreted as evidence that `a=rtcp-mux` is absent.

## Acceptance

- [x] `docs/specs/webrtc-audio.md` and the relevant ICE/multiplexing spec state how RFC 5761
      `a=rtcp-mux` composes with RFC 8445/8839 candidates for components one and two.
- [ ] A byte-exact failing-first SDP vector contains `a=rtcp-mux`, at least one viable component-one
      candidate and an extra component-two candidate, and reproduces the current
      `RtcpMuxRequired` profile error.
- [ ] Validation accepts that vector, retains only component-one candidates for connectivity checks,
      and still rejects an offer with no viable component-one path, absent `a=rtcp-mux`, or an
      otherwise invalid browser profile.
- [ ] Runtime facts prove one bound socket, one nominated pair and no component-two check, route or
      media worker. The fix cannot weaken source-address, ICE-generation, fingerprint or key-state
      gates.
- [ ] The independent browser proof that produced this offer shape reaches nomination, verified
      DTLS keys and bidirectional protected RTP with a finite failure bound.
- [ ] Native-to-native browser-profile tests remain green, so accepting unused candidates does not
      create a second profile or alter the emitted answer vocabulary.
- [ ] SDP vectors, browser harness docs/evidence, RFC registry entries and the complete repository
      gate are synchronized and green.

## Review evidence

Finding 9 reached WSS, SIP/SDP and ICE start with a second independently implemented browser engine,
then failed `Profile(RtcpMuxRequired)` before nomination. The candidate-level cause is a bounded
inference until the exact SDP vector is captured by this story.

## Progress

- In progress: the validator confirms the inferred seam—any remote component-two candidate is
  currently treated as contradicting `a=rtcp-mux`, before ICE can retain the viable component-one
  path. The spec update distinguishes an initial offer's bounded fallback candidates from the
  one-component mux answer and runtime.
