---
id: M-14
title: Encrypt the media
pillar: Media
status: ready
priority: 2
design:
epic: conformance
areas: [sipx-rtp, sipx-media, sipx-sdp]
note: RFC 3711 + 4568; the largest gap in the stack
---

# Encrypt the media

## Goal
SRTP, so that a call whose signalling is encrypted does not then send the audio in the clear.

## Acceptance
- [ ] SRTP protection and unprotection of RTP and RTCP (RFC 3711), with the default transform.
- [ ] SDES keying (RFC 4568): `a=crypto` offered and answered, and **only over a secure
      signalling path** — offering a key over cleartext SIP hands it to anyone on the path.
- [ ] A `sips:` or WSS call negotiates `RTP/SAVP`; a cleartext call does not offer SDES at all.
- [ ] A replayed packet is rejected rather than decrypted (RFC 3711 §3.3.2).
- [ ] The negotiation is visible: a caller can tell an encrypted call from an unencrypted one
      without reading a packet capture.
- [ ] Failing-first test: `media_on_a_secure_call_is_not_readable_from_the_wire`.

## Progress
- Not started, and it is the most conspicuous gap in the stack: `sips:` and WSS work today, the
  TLS policy has no way to disable verification anywhere, and then the audio goes out in
  plaintext.

## Notes
- DTLS-SRTP (RFC 5764) is the follow-on, and is what a browser will insist on. SDES first
  because it is smaller and already useful.
