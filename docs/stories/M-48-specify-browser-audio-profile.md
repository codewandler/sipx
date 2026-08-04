---
id: M-48
title: Specify the browser-audio profile and state machine
pillar: Media
status: done
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

- [x] A new spec in `docs/specs/` cites RFC 5761, 7118, 7874, 8445, 8825, 8827, 8829, 8834 and
      8839 and defines one audio media section in both offerer and answerer roles.
- [x] State tables define offer/answer, ICE nomination, DTLS role/fingerprint verification, SRTP
      key installation, media start, renegotiation, ICE restart, hangup and cancellation ordering.
- [x] The exact SDP profile is specified: `UDP/TLS/RTP/SAVPF`, `a=rtcp-mux`, ICE credentials and
      candidates, fingerprint/setup, Opus, PCMU, PCMA, comfort noise and `telephone-event`.
- [x] Fail-closed rules cover a missing Opus build feature, absent multiplexing, an incompatible
      DTLS role, wrong fingerprint, no nominated pair and an answer that selects weaker media.
- [x] One-component packet classification, queue and packet-size bounds are explicit for STUN,
      DTLS, SRTP and SRTCP; unknown or malformed datagrams have a typed/countable disposition.
- [x] Byte-level vectors cover a complete offer/answer and at least one negative per classifier
      class. Tests in the later child stories cite these vectors rather than creating a second
      contract.
- [x] TURN, video, data channels, browser APIs and multiple bundled media sections remain explicit
      omissions; the spec does not claim a general WebRTC stack.
- [x] `./scripts/gate.py` green.

## Progress

- In progress. `docs/specs/webrtc-audio.md` is the normative predecessor for `M-46`, `M-49` and
  `M-50`; it fixes the one-section SDP vocabulary, fail-closed state transitions, nominated-peer
  binding, resource bounds and the byte vectors those implementation stories must cite.
- Focused verification re-derived the offer and answer vectors as 555 and 563 CRLF-encoded octets
  with the recorded SHA-256 digests. The documentation-link scan checked 305 pages, 569 links and
  17 anchors; provenance, `git diff --check`, and `./scripts/gate.py --check` are green. The final
  acceptance item remains open for the combined-wave gate.

## Notes

- `M-38` remains the epic tracker. Splitting this contract from implementation prevents the XL
  tracker from being counted as one ordinary beta.4 story.
