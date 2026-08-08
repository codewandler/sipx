---
id: M-85
title: Refuse a header extension that disagrees with itself, at the send boundary
pillar: Media
status: backlog
priority: 5
design:
epic: media
areas: [sipx-media, sipx-rtp]
predicate:
announcement:
note: M-81 counted the packets SRTP refuses · the in-bounds case is refused by nothing, and sends media octets outside the encryption
---

# Refuse a header extension that disagrees with itself, at the send boundary

## Goal

Stop the send path from putting a caller-supplied RTP header extension on the wire when its
embedded length word disagrees with the bytes that follow it. `M-81` made the case that fails
loudly visible; this is the case that fails quietly, and the consequence is worse than a lost
packet.

## Why

`M-79` made `Encoded::extension` public and nothing validates it. `M-81` handled the extension
whose length word overstates its bytes *far enough to run past the whole packet*: `rtp_header_len`
returns `None`, `SrtpContext::protect` refuses, and the packet is now dropped and counted as
`srtp_protect_failures`.

The **in-bounds** version of the same mistake is not refused by anything. An extension that claims
nine 32-bit words and carries one still leaves the computed header inside the packet, so
`rtp_header_len` accepts it and the header/payload split lands 32 octets *inside the media*. Those
octets are then treated as header: authenticated but not encrypted under counter mode, Associated
Data under the AEAD profiles (RFC 7714 §8.2). They go out in the clear on a leg the far end
negotiated encryption for, and nothing is dropped, counted or logged, because from the transform's
point of view nothing went wrong.

Measured, not reasoned: an 8-octet extension declaring nine words, on a 160-octet µ-law payload,
protected under each of the three profiles, put a 32-octet run of plaintext µ-law on the wire —
`AeadAes256Gcm: len=196 plaintext_runs=17` for windows of 16 identical octets. The far end also
reads the payload boundary from the same length word, so it decodes 32 octets short.

**Bounded, and caller-induced rather than attacker-induced.** A relayed extension cannot do this:
`Packet::decode` slices exactly `4 + words * 4` octets and rejects a length word that runs past the
datagram, so an extension that arrived over the network is self-consistent by construction
(`crates/sipx-rtp/src/packet.rs:141`). Reaching this needs the application on *this* side to build
`Encoded::extension` by hand and get the length word wrong. Nothing about what SRTP authenticates
or encrypts is weakened — the transform does exactly what it was asked to; it is asked the wrong
question.

The plain-RTP leg has the same shape without the confidentiality edge: the packet goes out as
written and the far end rejects it, silently, with no counter on either side.

## Acceptance

- [ ] A failing-first test protects a packet whose caller-supplied extension declares more words
      than it carries but stays inside the packet, and asserts no plaintext payload octet reaches
      the wire.
- [ ] One boundary refuses it, and the choice is argued in `docs/specs/media-runtime.md`: either
      `Packet::encode` writes no extension it cannot make self-consistent — extending the length
      filter it already applies below four octets — or `send_encoded` refuses the frame with a
      typed error. If the extension is dropped rather than the packet, say what a caller is
      promised, because the extension is metadata the far end may be relying on.
- [ ] Whatever is chosen, an operator can see it happened: a counter, or a documented reason no
      counter can reach it under `docs/specs/media-runtime.md` §4's rule.
- [ ] The plain-RTP leg is covered by the same answer, or the difference is stated.
- [ ] `Encoded::extension`'s rustdoc is updated to describe what actually happens after this
      change; `M-81` documented the behaviour this story replaces.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `M-81`'s implementation, which counted the out-of-bounds case and verified
  this one empirically while checking the reachability chain. A typed refusal at `send_encoded`
  changes a published signature and reaches `sipx-app`'s realtime sink trait, which is why `M-81`
  did not take that route for the narrower bug — worth deciding once, here, for both.

## Notes

- Sites: `crates/sipx-rtp/src/packet.rs:76` (`encode`'s existing four-octet filter),
  `crates/sipx-rtp/src/srtp/mod.rs:1079` (`rtp_header_len`),
  `crates/sipx-media/src/session.rs:2425` (`send_encoded`).
- Priority above `M-81`'s because the failure is silent and touches confidentiality on an encrypted
  leg, and below anything attacker-reachable because it is not.

- 2026-08-08: renumbered from `M-84` on filing; that id was allocated in the same wave to carrying
  calibrated thresholds onto the application wire. Only the id moved. This remains the more serious
  half of what `M-81` uncovered: an extension whose length word overstates its bytes while keeping
  the computed header inside the packet is refused by nothing, and puts a plaintext run of media on
  the wire under all three protection profiles.
