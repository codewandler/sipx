---
id: M-48
title: Specify the browser-audio profile and state machine
pillar: Media
status: ready
priority: 1
design: docs/designs/webrtc-audio.md
epic: webrtc-audio
areas: [sipx-sdp, sipx-media, sipx-call, interop, docs, beta4]
predicate:
announcement: 3
note: beta.4 starts here · normative profile, ordering, downgrade refusals, resource bounds and byte-level vectors before code
---

# Specify the browser-audio profile and state machine

## Goal

Turn the proposed browser-audio design into a normative, bounded contract that the negotiation,
runtime and independent-peer stories can implement without inventing policy in parallel.

## Acceptance

- [ ] A new spec in `docs/specs/` cites RFC 5761, 7118, 7874, 8445, 8825, 8827, 8829, 8834 and
      8839 and defines one audio media section in both offerer and answerer roles.
- [ ] State tables define offer/answer, ICE nomination, DTLS role/fingerprint verification, SRTP
      key installation, media start, renegotiation, ICE restart, hangup and cancellation ordering.
- [ ] The exact SDP profile is specified: `UDP/TLS/RTP/SAVPF`, `a=rtcp-mux`, ICE credentials and
      candidates, fingerprint/setup, Opus, PCMU, PCMA, comfort noise and `telephone-event`.
- [ ] Fail-closed rules cover a missing Opus build feature, absent multiplexing, an incompatible
      DTLS role, wrong fingerprint, no nominated pair and an answer that selects weaker media.
- [ ] One-component packet classification, queue and packet-size bounds are explicit for STUN,
      DTLS, SRTP and SRTCP; unknown or malformed datagrams have a typed/countable disposition.
- [ ] Byte-level vectors cover a complete offer/answer and at least one negative per classifier
      class. Tests in the later child stories cite these vectors rather than creating a second
      contract.
- [ ] TURN, video, data channels, browser APIs and multiple bundled media sections remain explicit
      omissions; the spec does not claim a general WebRTC stack.
- [ ] `./scripts/gate.py` green.

## Progress

- Not started. This is the specification predecessor for `M-46`, `M-49` and `M-50`.

## Notes

- `M-38` remains the epic tracker. Splitting this contract from implementation prevents the XL
  tracker from being counted as one ordinary beta.4 story.
