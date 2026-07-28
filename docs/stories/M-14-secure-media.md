---
id: M-14
title: Encrypt the media
pillar: Media
status: done
priority: 1
design:
epic: conformance
areas: [sipx-rtp, sipx-media, sipx-sdp]
note: track: media · RFC 3711 + 4568 · the largest gap in the stack
---

# Encrypt the media

## Goal
SRTP, so that a call whose signalling is encrypted does not then send the audio in the clear.

## Acceptance
- [x] SRTP protection and unprotection of RTP and RTCP (RFC 3711), with the default transform.
- [x] SDES keying (RFC 4568): `a=crypto` offered and answered, and **only over a secure
      signalling path** — offering a key over cleartext SIP hands it to anyone on the path.
- [x] A `sips:` or WSS call negotiates `RTP/SAVP`; a cleartext call does not offer SDES at all.
- [x] A replayed packet is rejected rather than decrypted (RFC 3711 §3.3.2).
- [x] The negotiation is visible: a caller can tell an encrypted call from an unencrypted one
      without reading a packet capture.
- [x] Failing-first test: `media_on_a_secure_call_is_not_readable_from_the_wire`.

## Progress
- Done. `sipx-rtp::srtp` for the transform, `sipx-sdp::crypto` for the keying, and the wiring
  through `sipx-media` and `sipx-call` so a call over `sips:` or WSS negotiates it and one over
  cleartext SIP does not.
- **Checked against RFC 3711's own Appendix B vectors**, not against sipx's arithmetic. That
  matters more here than anywhere else in the stack: a key derivation that is wrong but
  self-consistent gives two endpoints that interoperate perfectly with each other and with
  nothing else in the world, and every round-trip test would still pass.
- The rollover inference was wrong first time in a way worth recording. RFC 3711 §3.3.1 writes
  `if (SEQ - s_l > 32768)` over two 16-bit values and means **signed** subtraction. Read as
  wrapping `u16`, a packet arriving one place out of order looks 65 535 ahead, is taken for the
  previous cycle, and fails authentication — every out-of-order packet in a call, silently
  dropped. Three tests caught it.
- The rule from RFC 4568 §7.1 is enforced by the signature rather than documented:
  `Crypto::offer` takes whether the signalling is secure and returns `None` when it is not, so
  a key cannot be published by someone forgetting a check.
- Both halves or neither. A stream keyed at one end only connects and carries silence, which is
  worse than one that fails to connect — so `srtp_keys` needs our key *and* theirs.
- A session expecting SRTP refuses plain RTP. Accepting it would let an attacker downgrade the
  call with one unencrypted packet.
- Mutation-checked: offering a key over cleartext, accepting plain RTP on an encrypted session,
  and skipping the replay window each fail tests.


## Notes
- DTLS-SRTP (RFC 5764) is the follow-on, and is what a browser will insist on. SDES first
  because it is smaller and already useful.
