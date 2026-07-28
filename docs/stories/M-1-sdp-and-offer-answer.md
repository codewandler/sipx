---
id: M-1
title: Implement SDP and RFC 3264 offer/answer
pillar: Media
status: backlog
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
- [ ] SDP parses to a typed AST and re-serializes; unknown lines and attributes survive rather
      than being dropped.
- [ ] Offer/answer is a pure function: an answer has the same number of `m=` lines in the same
      order as the offer, and a rejected stream is answered with port 0 rather than omitted.
- [ ] Codec selection intersects the two lists and keeps the *offerer's* preference order.
- [ ] Direction attributes are reflected correctly: `sendonly` is answered `recvonly`, and an
      absent direction means `sendrecv`.
- [ ] A stream with no codec in common is rejected rather than answered with an empty list.
- [ ] Failing-first test: `an_answer_keeps_the_offers_media_order_and_rejects_with_port_zero`.

## Progress
- Not started.

## Notes
- Offer/answer being pure is what makes the hard cases cheap to test; keep sockets out of it.
