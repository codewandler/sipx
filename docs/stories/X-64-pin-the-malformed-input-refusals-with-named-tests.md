---
id: X-64
title: Pin the malformed-input refusals with named tests
pillar: Build
status: ready
priority: 2
design: docs/designs/input-hardening.md
epic: input-hardening
areas: [sipx-sip, sipx-transport, beta4]
predicate:
announcement: 2
note: three recurring input classes · properties currently asserted by design, sampled by fuzzing, pinned by nothing · beta-1
---

# Pin the malformed-input refusals with named tests

## Goal

Convert three malformed-input properties sipx already holds from design claims into named tests
that fail if the property is removed, so a refactor cannot quietly restore a defect class that
recurs across independent SIP implementations.

## Acceptance

- [ ] A request that frames correctly but omits the headers response construction reads
      (`To`, `From`, `Call-ID`, `CSeq`, the `Via` stack — RFC 3261 §8.2.6.1) yields a typed refusal
      from every public path that builds a response to it, proven by a failing-first test per path.
- [ ] A table-driven test asserts the pre-allocation bound on **every** framing path independently —
      UDP datagram, TCP and TLS stream, WS and WSS frame (RFC 7118, RFC 6455 §5.2) — from one table,
      so a transport added later without the bound fails an existing test rather than needing a new
      one. The assertion is on the typed error and on a held bound, not on absence of a panic.
- [ ] A declared length that disagrees with the bytes that follow — short body and long body — is a
      typed error or a bounded wait on every framing path; never a hang, never a read past the frame.
- [ ] Each test carries its RFC citation in a comment on the test itself, and names the property it
      pins rather than the input it sends.
- [ ] For each of the three classes, the story's Progress log records the scratch edit that removes
      the corresponding bound and the test failing against it. A test that passes with the bound
      removed does not satisfy this story.
- [ ] `./scripts/gate.py` green.

## Progress
- (not started)

## Notes
- The properties already hold: bounds are checked before allocation at
  `crates/sipx-sip/src/parser.rs:18-35`, with 64 KiB datagram and 1 MiB stream profiles. This story
  adds no defence; it adds the tests that keep them.
- `crates/sipx-testkit`'s in-process link drives the datagram and stream paths without sockets. Whether
  WS and WSS can be driven at the same level, or need a per-transport test sharing assertions through
  a helper, is the design's open question — resolve it in the story, do not skip the paths.
- Not a fuzzing story. `fuzz/fuzz_targets/` already samples unknown inputs; this pins known properties.
  See [`docs/designs/input-hardening.md`](../designs/input-hardening.md) for why they are not substitutes.
