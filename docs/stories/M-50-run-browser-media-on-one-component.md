---
id: M-50
title: Run ICE, DTLS, SRTP and SRTCP on one nominated component
pillar: Media
status: in-progress
priority: 8
design: docs/specs/webrtc-audio.md
epic: webrtc-audio
areas: [sipx-media, sipx-rtp, sipx-call, sipx-transport, beta4]
predicate:
announcement: 4
note: beta.4 critical path · one bounded owner, nominated-peer binding and no bind/drop/rebind race
---

# Run ICE, DTLS, SRTP and SRTCP on one nominated component

## Goal

Make the nominated ICE component the single cancellation-safe owner of browser audio traffic, with
STUN, DTLS, SRTP and SRTCP classified and processed on one bound port.

## Acceptance

- [x] One bounded owner receives datagrams for the component and classifies STUN, DTLS, SRTP and
      SRTCP by the rules and byte vectors in `M-48`; malformed and unknown input is refused and
      counted without panic or unbounded allocation.
- [x] ICE nomination chooses the DTLS peer. No handshake or protected-media packet is accepted from
      a provisional SDP address or a non-nominated candidate pair.
- [x] DTLS key installation occurs only after nomination, compatible role selection and fingerprint
      verification; a mismatch yields no SRTP or SRTCP keys.
- [x] RTP and RTCP use the multiplexed component that SDP negotiated. The test proves an RTCP packet
      arriving on that port is processed rather than merely proving `a=rtcp-mux` was emitted.
- [x] There is no bind/drop/rebind window between ICE, DTLS and protected media. A deterministic
      ordering test injects a packet at every ownership transition and accounts for it.
- [x] Queues, task count, packet size and shutdown are bounded and cancellation-safe; cancellation
      leaves no receiver, timer or media task detached.
- [x] SRTCP uses `M-47`'s separate replay window, and repeated control traffic remains rejected when
      interleaved with advancing SRTP sequence numbers.
- [x] A two-sipx deterministic proof carries non-silent Opus both ways over the complete component
      before the independent endpoint is introduced.
- [ ] `./scripts/gate.py` green, including `check-fixed-sleep.py`.

## Progress

- The runtime portion is complete: one persistent owner composes ICE nomination, verified DTLS,
  SRTP and SRTCP; exact drop counters, independent replay state, live bidirectional Opus and
  cancellation in checking, handshaking and running phases are executable tests.
- The task census is tied to actual task lifetimes. It measures a preparation peak of four and a
  running peak of six, with the supervisor ending before media workers attach. The story remains
  in progress until the shared `M-49` integration tree passes the complete project gate.

## Notes

- This is the wave's largest and highest-risk story. It owns runtime composition; it does not own
  browser-peer infrastructure or public compatibility claims, which belong to `M-51`.
