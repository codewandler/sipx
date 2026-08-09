---
id: M-110
title: Stop an RTP packet rendering its payload
pillar: Media
status: backlog
priority: 6
design:
epic: media
areas: [sipx-rtp]
predicate:
announcement:
note: after M-107 · Packet, Rtcp and SdesItem derive Debug over Bytes; M-107 redacted Encoded at the other end of the same relay path
---

# Stop an RTP packet rendering its payload

## Goal

Give `sipx_rtp::Packet` — and the RTCP types beside it that hold `Bytes` — a `Debug` that carries
what a protocol log needs and not the call it is carrying.

## Why

`M-107` closed this on `sipx_media::Encoded`, which is the *same payload* one layer up: its
derived `Debug` rendered every octet, and for G.711 an octet is a sample. `sipx_rtp::Packet` still
does. So does `Rtcp`, and so does `SdesItem` — whose `Bytes` is a CNAME or an end-user's name and
email address (RFC 3550 §6.5), which is not audio but is personal data with the same problem.

Fixing one end of a relay path and leaving the other is the mistake this pair has already made
once in the other direction: `M-80`'s note records marking `Encoded` and `Packet`
`#[non_exhaustive]` **together**, "because they are the two ends of one relay path and marking one
would have left the other free to break the same caller in the same way for the same reason". The
same sentence applies here and `M-107` could not act on it, because `sipx-rtp` was outside the
story's crates.

**It is not enforceable the way `M-107`'s half is, and that is the point of a separate story.**
`scripts/check-audio-claims.py` recognises a sample buffer by its element type — `Vec<i16>`,
`[i16; N]` — and holds every published crate to it. A payload is `Bytes`, which in this workspace
is equally a `Call-ID`, a SIP body, a URI and a STUN attribute: around sixty reachable public types
hold one, and for nearly all of them rendering the bytes is what a protocol log is *for*. So this
is decided type by type with an argument each time, which is a review and not a rule.

## Acceptance

- [ ] A failing-first test renders a `Packet` carrying a distinctive payload and asserts no payload
      octet appears in the output, in decimal or in hex.
- [ ] `Packet`'s `Debug` carries payload type, sequence, timestamp, SSRC, marker and the payload's
      **length**, and its length is bounded independently of the packet's.
- [ ] `Rtcp` and `SdesItem` are decided on the same terms and each is either fixed or carries the
      reason its rendering is safe. `SdesItem` is the one to argue rather than assume: a CNAME is
      an identifier a log legitimately correlates on, and an RFC 3550 §6.5 `NAME`/`EMAIL` item is
      not.
- [ ] Whether the header extension's bytes belong in a record is answered either way — they are
      RFC 8285 metadata rather than media, and `M-107` rendered only their length at `Encoded`.
- [ ] `./scripts/gate.py` green.
