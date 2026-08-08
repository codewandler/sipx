---
id: M-70
title: "Accept multiplexed browser offers with unused component candidates"
pillar: Media
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
- [x] A byte-exact failing-first SDP vector contains `a=rtcp-mux`, at least one viable component-one
      candidate and an extra component-two candidate, and reproduces the current
      `RtcpMuxRequired` profile error.
- [x] Validation accepts that vector, retains only component-one candidates for connectivity checks,
      and still rejects an offer with no viable component-one path, absent `a=rtcp-mux`, or an
      otherwise invalid browser profile.
- [x] Runtime facts prove one bound socket, one nominated pair and no component-two check, route or
      media worker. The fix cannot weaken source-address, ICE-generation, fingerprint or key-state
      gates.
- [ ] The independent browser proof that produced this offer shape reaches nomination, verified
      DTLS keys and bidirectional protected RTP with a finite failure bound.
- [x] Native-to-native browser-profile tests remain green, so accepting unused candidates does not
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
- Implemented: byte-pinned `BA-SDP-O2` first reproduced `RtcpMuxRequired`, then crossed the profile
  with only its component-one candidate retained. Component-two offer fallbacks are line- and
  count-bounded; component-two-only, non-mux and over-bound offers remain typed refusals, as does a
  component-two candidate in an answer after mux has been selected.
- Runtime boundary: the answering call now constructs its remote ICE description from the filtered
  browser-profile result. A call-layer mutation test proves the discarded candidate cannot enter
  the agent, while the existing browser component suite keeps its one-socket nomination,
  fingerprint, generation, DTLS and protected-media gates green.
- Proof boundary: the bounded native-browser harness now requires a third positive call whose
  browser-authored offer gains exactly one unused RTCP fallback. Its evidence validator requires
  offered components 1 and 2, a component-one mux answer, one nominated pair, DTLS keys and
  protected audio. All 14 adversarial harness self-tests pass; the real browser run, derived RFC
  report regeneration and complete repository gate remain deliberately deferred to push time.

- 2026-08-08: **held out of the rc.4 wave — its remaining proof cannot be produced locally.** The
  open row needs the independent browser session that produced the original offer shape to reach
  nomination against the fix, and `scripts/gate.py` declares `browser-audio` NOT_RUN_LOCALLY:
  "requires the hosted runner's matched native browser/WebDriver; the local gate runs its
  adversarial harness suite". What ships today is a byte-pinned SDP vector, filtered validation, a
  call-layer mutation proof and the adversarial harness suite — interoperation with that engine
  remains an inference from the captured vector rather than an observed browser call, and the rc.3
  release notes say exactly that. Closing this needs a hosted CI run, not an implementor.
