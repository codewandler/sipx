---
id: M-1
title: Implement SDP and RFC 3264 offer/answer
pillar: Media
status: done
priority:
design: docs/designs/media.md
epic: media
areas: [sipx-sdp]
note:
---

# Implement SDP and RFC 3264 offer/answer

## Goal
Parse and build SDP (RFC 8866), and implement offer/answer (RFC 3264) as a pure function over
two session descriptions so that negotiation is testable without a socket.

## Acceptance
- [x] SDP parses to a typed AST and re-serializes; unknown lines and attributes survive rather
      than being dropped.
- [x] Offer/answer is a pure function: an answer has the same number of `m=` lines in the same
      order as the offer, and a rejected stream is answered with port 0 rather than omitted.
- [x] Codec selection intersects the two lists and keeps the *offerer's* preference order.
- [x] Direction attributes are reflected correctly: `sendonly` is answered `recvonly`, and an
      absent direction means `sendrecv`.
- [x] A stream with no codec in common is rejected rather than answered with an empty list.
- [x] Failing-first test: `an_answer_keeps_the_offers_media_order_and_rejects_with_port_zero`.

## Progress
- Done. `crates/sipx-sdp/`. Negotiation is a pure function, which is what makes the awkward
  cases cheap: no common codec, an offer that already rejected a stream, a `sendonly` that must
  become `recvonly`.
- Dynamic payload types are matched by *encoding name*, not number. 96 is Speex at one end and
  Opus at the other; the numbers agreeing means nothing, and agreeing on that basis is how a
  stack accepts a codec it cannot decode.
- A stream offering only `telephone-event` is rejected: DTMF alone is not a call, and accepting
  it would establish a session that can never carry speech.

## Notes
- Offer/answer being pure is what makes the hard cases cheap to test; keep sockets out of it.
