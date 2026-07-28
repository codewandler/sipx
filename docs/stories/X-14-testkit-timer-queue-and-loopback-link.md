---
id: X-14
title: Generalize the timer queue and ship the loopback link the testkit promises
pillar: Build
status: ready
priority: 7
design: docs/designs/sip-transport.md
epic: conformance
areas: [sipx-transport, sipx-testkit]
note: track: test infrastructure · two items, both useful to sipx on their own
---

# Generalize the timer queue and ship the loopback link the testkit promises

## Goal
Make the two pieces of scheduling machinery that any sans-IO driver needs reusable: a timer queue
that does not read the clock itself, and an in-process byte link that can lose, duplicate and
delay on a seed.

## Acceptance

**The timer queue**
- [ ] `TimerQueue` is generic over its key rather than fixed to `(TransactionKey, Timer)`.
- [ ] `now` is a parameter of `set`, not `Instant::now()` read inside it. Today `set` computes
      `Instant::now() + after` internally, so a caller driving virtual time cannot use it at all
      — and a test that wants to assert *when* a retransmission was scheduled has to sleep.
- [ ] The generation-counter cancellation discipline is unchanged and its tests still pass.

**The loopback link**
- [ ] `sipx-testkit` gains the loopback transport its own crate documentation has promised since
      it was written ("a loopback transport that lets two full stacks talk inside one process with
      no sockets") and has never shipped — the module list is `certs`, `load`, `rfc4475`, `soak`.
- [ ] The link is seeded and its faults are knobs: loss, duplication, reordering, latency
      distribution. The same seed replays the same trace.
- [ ] Failing-first test: `a_retransmission_gets_through_a_link_that_drops_the_first_datagram` —
      the retransmission behaviour the transaction machines exist for, tested end to end without
      a socket or a sleep.

## Progress
- Not started.

## Notes
- Both items came out of the harness design in
  [sipx-clstr](https://github.com/codewandler/sipx-clstr)'s `CF-1`, which decided component by
  component what belongs in the kernel and what is cluster-specific; these two are the only pieces
  that moved ([ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md)).
  The virtual clock deliberately did **not**: sans-IO layers take fired timers by contract and
  must never see a clock, so there is no kernel `Clock` trait to add.
- Sharing the queue means the tokio driver and any other scheduler have one cancellation
  discipline between them instead of two that drift.
