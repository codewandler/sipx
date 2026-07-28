# Design: Media

**Status:** outline · **Pillar:** Media · **Epic:** `media` · **Stories:** _to be cut_

## Why

Signalling that cannot carry audio is a curiosity. The media layer is also where the sans-IO
discipline pays off a second time: SDP negotiation is a pure function, and RTP packet handling
is pure, so the only part that needs a socket is the session that binds them.

## Approach

_To be written when the epic starts. In outline: SDP (RFC 8866) parsed into a typed AST rather
than a map of lines; offer/answer (RFC 3264) as `answer(local_caps, remote_offer) -> Result<…>`
with no I/O; RTP and RTCP (RFC 3550) as owned packet types; a jitter buffer with configurable
depth and full late/duplicate/lost accounting; G.711 µ-law and A-law and G.722 in pure Rust,
Opus behind a feature; symmetric-RTP address learning for NAT._

## Alternatives considered

- **Depend on an existing Rust RTP/SDP crate ecosystem.** Rejected: it couples the stack to
  another project's API and pulls a large dependency tree oriented around browser media, for
  code we can own in a few hundred lines.

## Risks & open questions

- Clock and pacing: sending RTP on time without a busy loop, and without drift accumulating
  over a long call.
- Where transcoding lives when a bridge joins legs with different codecs.
- Whether the jitter buffer is in the media session or the call layer — it affects who owns
  playout timing.

## Acceptance / done

Two sipx endpoints exchange G.711 audio that passes a bit-exactness check, with RTCP
statistics reported and a jitter buffer that survives injected loss and reordering.
