---
id: M-15
title: Key SRTP with DTLS
pillar: Media
status: ready
priority: 3
design:
epic: conformance
areas: [sipx-media]
note: M6 · RFC 5764 · M-14 unblocked it
---

# Key SRTP with DTLS

## Goal
Keying that does not require trusting the signalling path, and the only keying a browser will
accept.

## Acceptance
- [ ] DTLS-SRTP handshake over the media path, with the fingerprint carried in SDP
      (RFC 5763 / 8122).
- [ ] The fingerprint in the SDP is checked against the certificate presented, or the media is
      dropped — an unchecked fingerprint makes the whole exchange decorative.
- [ ] Works with the WebSocket transports, since that is the combination browsers use.
- [ ] Failing-first test: `a_mismatched_fingerprint_stops_the_media`.

## Progress
- Not started. Blocked by `M-14`.
