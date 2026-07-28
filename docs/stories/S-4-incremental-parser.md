---
id: S-4
title: Implement the incremental message parser and fuzz it
pillar: Signalling
status: backlog
priority:
design: docs/designs/sip-core.md
epic: sip-core
areas: [sipx-sip]
note: the correctness bar for M1
---

# Implement the incremental message parser and fuzz it

## Goal
Parse SIP messages from both datagram and stream transports, correctly and without ever
panicking, and prove it with the RFC 4475 corpus and a fuzzer.

## Acceptance
- [ ] Datagram parsing: one message per datagram, with trailing bytes an error.
- [ ] Stream parsing: incremental framing across arbitrary chunk boundaries, driven by
      `Content-Length` (RFC 3261 §7.5), including a message split at every possible byte
      offset.
- [ ] Absent, duplicate, non-numeric or oversized `Content-Length` are each handled per the
      decision recorded in `docs/specs/sip-parser.md`.
- [ ] Line folding, leading whitespace and CRLF-vs-LF tolerance behave as specified.
- [ ] Every RFC 4475 valid case parses and round-trips; every invalid case is rejected with a
      specific typed error, never a generic one.
- [ ] Failing-first test: `stream_parser_survives_split_at_every_offset`.
- [ ] `cargo fuzz` targets for the datagram and stream parsers; a smoke run in CI, and a
      documented longer run before release. No panics, no unbounded allocation.

## Progress
- Not started.

## Notes
- A message declaring a `Content-Length` larger than a configured maximum must be rejected
  without allocating that much — this is the obvious remote memory-exhaustion vector.
