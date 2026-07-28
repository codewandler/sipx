---
id: S-4
title: Implement the incremental message parser and fuzz it
pillar: Signalling
status: done
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
- [x] Datagram parsing: one message per datagram. _Corrected while writing the spec: trailing
      octets are **ignored**, not an error. RFC 3261 §18.3 calls them spurious noise and
      RFC 4475 §3.1.1.8 makes a valid-message test of exactly this. They are dropped rather
      than forwarded._
- [x] Stream parsing: incremental framing across arbitrary chunk boundaries, driven by
      `Content-Length` (RFC 3261 §7.5), including a message split at every possible byte
      offset.
- [x] Absent, duplicate, non-numeric or oversized `Content-Length` are each handled per the
      decision recorded in `docs/specs/sip-parser.md`.
- [x] Line folding, leading whitespace and CRLF-vs-LF tolerance behave as specified.
- [x] Every RFC 4475 valid case parses and round-trips; every invalid case is rejected with a
      specific typed error, never a generic one.
- [x] Failing-first test: `stream_parser_survives_split_at_every_offset`.
- [x] `cargo fuzz` targets for the datagram and stream parsers; a smoke run in CI, and a
      documented longer run before release. No panics, no unbounded allocation.

## Progress
- Done. `crates/sipx-sip/src/{message,parser}.rs`, plus the corpus and property suites.
  Taken before `S-3` because typed headers need the `Headers` container the parser builds.
- The corpus classification needed two corrections, both found by running it:
  - `badinv01` (§3.1.2.1) is *not* a structural fault. Its `Via` is
    `SIP/2.0/UDP 192.0.2.15;;,;,,`, which frames as an ordinary header line; the stray
    separators violate the Via grammar. The identical value under an unknown header name is
    legal — `wsinv`, a valid message, carries `UnknownHeaderWithUnusualValue: ;;,,;;,;`.
  - `baddn` (§3.1.2.15) illustrates a display-name fault but its archive file, alone among the
    fifty, has no terminating blank line, so it fails while framing. We stayed strict: a
    missing terminator is indistinguishable from a truncated message. The display-name fault
    needs its own hand-built test in `S-3`.
- Status lines cannot use the request line's strict SP rule: a reason phrase may contain
  spaces, so the line is cut at the first two only.
- `dblreq` is the one valid case that does not round-trip to equal bytes, and must not: the
  trailing octets are noise the RFC forbids forwarding.

## Notes
- A message declaring a `Content-Length` larger than a configured maximum must be rejected
  without allocating that much — this is the obvious remote memory-exhaustion vector.
