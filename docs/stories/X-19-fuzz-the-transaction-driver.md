---
id: X-19
title: Fuzz the transaction driver, not only the parser
pillar: Build
status: backlog
priority:
design: docs/designs/sip-core.md
epic: sip-core
areas: [sipx-sip, sipx-testkit]
note: M12 · four fuzz targets, all of them parsers; the timing half of the north star is untested
---

# Fuzz the transaction driver, not only the parser

## Goal
Fuzz *sequences* — messages and fired timers driven into the transaction layer — so the half of the
north star about adversarial **timing** is tested the way the half about adversarial input already is.

## Acceptance
- [ ] A fuzz target drives `TransactionLayer` with a sequence decoded from the fuzzer's bytes:
      incoming messages, application requests and fired timers, in any order the decoder can produce.
      The existing four targets (`parse_datagram`, `parse_stream`, `parse_uri`, `roundtrip`) all stop
      at the parser.
- [ ] Sequences are structured, not raw bytes reinterpreted as SIP. A fuzzer that spends its budget
      producing unparseable messages is re-testing `S-4`; this one must reach the state machines, so
      the input is a decoded program over a small vocabulary of events.
- [ ] The invariants asserted are the ones a state machine can violate without panicking, because a
      panic-only oracle would find almost nothing here. At minimum: no transaction outlives its
      terminal state, no timer fires for a key that has been removed, the store never grows without
      bound over a bounded sequence, and no state is reachable that the RFC 3261 §17 tables (as
      amended by RFC 6026) do not name.
- [ ] The corpus is seeded from something meaningful rather than from nothing — the sequences the
      existing FSM table tests already encode are the obvious seed, the way CI seeds the parser targets
      from the RFC 4475 corpus.
- [ ] It runs in the existing `fuzz smoke` CI job under the same time budget, with the corpus
      committed and proven unmodified, exactly as the parser targets are.
- [ ] Any crash or invariant violation is minimised, committed as a regression test in
      `crates/sipx-sip`, and fixed in its own story. The fuzzer is the instrument.
- [ ] Failing-first test: the minimised regression test for the first sequence it finds — and if it
      finds none in its first campaign, `a_seeded_event_sequence_replays_the_same_transaction_trace`,
      which proves the harness is driving what it claims to drive.

## Progress
- Not started. `fuzz/fuzz_targets/` holds four targets, all of them parsers. The transaction machines
  are covered by table tests, which prove the transitions the tables name and nothing about the
  sequences nobody thought of.

## Notes
- This is exactly the kind of testing the sans-IO design was chosen to make possible: the transaction
  layer takes fired timers as inputs and produces effects, so a fuzzer needs no clock, no socket and
  no runtime. If it turns out to be awkward, the awkwardness is a finding about the API.
- It pairs with `X-14`'s deterministic pieces — a generic timer queue and a seeded loopback link —
  which arrive in M7. Those make a *driver-level* campaign possible too, over two whole stacks; that
  is a bigger story and this one deliberately stays inside `sipx-sip`.
- The last acceptance item is written the way it is on purpose. A fuzzing story whose failing-first
  test is a bug it has not found yet cannot be started, and one that promises to find a bug cannot be
  finished honestly.
