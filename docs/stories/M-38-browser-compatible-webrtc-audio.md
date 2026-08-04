---
id: M-38
title: Complete one browser-compatible WebRTC audio path
pillar: Media
status: backlog
priority: 15
design: docs/designs/webrtc-audio.md
epic: webrtc-audio
areas: [sipx-sdp, sipx-media, sipx-call, sipx-transport, interop, docs]
predicate:
announcement:
note: epic tracker; reuse WSS, ICE, DTLS-SRTP and Opus, then prove their browser-audio composition
---

# Complete one browser-compatible WebRTC audio path

## Goal

Compose the shipped signalling, NAT, keying and codec pieces into one browser-compatible audio call
without expanding sipx into a video, data-channel, or general browser media stack.

## Acceptance

- [ ] A normative browser-audio spec cites RFC 5761, 7118, 7874, 8445, 8825, 8827, 8829, 8834 and
      8839;
      it defines one audio media section, offer/answer state, DTLS/ICE ordering, RTCP multiplexing,
      downgrade refusal, and byte-level vectors before implementation changes.
- [ ] One call profile offers and answers `UDP/TLS/RTP/SAVPF`, `a=rtcp-mux`, ICE credentials and
      candidates, a DTLS fingerprint/setup role, and the RFC 7874 audio vocabulary. A missing build
      feature or incompatible answer is a typed refusal, never a fallback to SDES or plain RTP.
- [ ] ICE and DTLS-SRTP share the selected media component: nomination chooses the DTLS peer, and
      STUN, DTLS, SRTP and SRTCP are demultiplexed on the same bound port without a bind/drop/rebind
      race. RTCP actually uses the multiplexed component rather than only advertising it.
- [ ] A bounded shell proof drives an independently implemented browser SIP endpoint in both call
      roles over WSS + ICE + DTLS-SRTP + Opus, carries non-silent audio in both directions, and
      reports the negotiated codec, keying and candidate pair. Wrong-fingerprint and weaker-answer
      negatives are immediate and non-vacuous.
- [ ] RFC registry and public fit/security pages are updated in the same change. They distinguish
      the working host/server-reflexive audio path from the still-absent TURN relay, video, browser
      API, data-channel and multi-media-section surfaces.

## Progress

Not started. The prerequisites are real and separately reachable: WSS (`T-8`, `T-9`, `T-23`),
Opus (`M-13`, `M-30`, `P-9`, `P-13`), ICE (`M-19` through `M-23`, `M-27`, `P-9`) and DTLS-SRTP
(`M-15`, `M-28`, `P-9`). The composition is not present. The call and CLI explicitly refuse
DTLS-SRTP with ICE; SDP has no `RTP/SAVPF` or `a=rtcp-mux` path; and no independently implemented
browser SIP endpoint has carried audio with sipx.

## Notes

- `M-24` remains the TURN-client story. It widens reachability but does not turn this epic into a
  requirement to operate a relay.
- This tracker owns the browser-specific proof. It does not change the diagnostic-phone release
  matrix or reopen its completed vectors.
