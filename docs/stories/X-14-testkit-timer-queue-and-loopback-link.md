---
id: X-14
title: Generalize the timer queue and ship the loopback link the testkit promises
pillar: Build
status: done
priority:
design: docs/designs/sip-transport.md
epic: conformance
areas: [sipx-transport, sipx-testkit]
note: M7 · a timer queue and a lossy loopback link, both useful to sipx on their own
---

# Generalize the timer queue and ship the loopback link the testkit promises

## Goal
Make the two pieces of scheduling machinery that any sans-IO driver needs reusable: a timer queue
that does not read the clock itself, and an in-process byte link that can lose, duplicate and
delay on a seed.

## Acceptance

**The timer queue**
- [x] `TimerQueue` is generic over its key rather than fixed to `(TransactionKey, Timer)`.
- [x] `now` is a parameter of `set`, not `Instant::now()` read inside it. Today `set` computes
      `Instant::now() + after` internally, so a caller driving virtual time cannot use it at all
      — and a test that wants to assert *when* a retransmission was scheduled has to sleep.
- [x] The generation-counter cancellation discipline is unchanged and its tests still pass.

**The loopback link**
- [x] `sipx-testkit` gains the loopback transport its own crate documentation has promised since
      it was written ("a loopback transport that lets two full stacks talk inside one process with
      no sockets") and has never shipped — the module list is `certs`, `load`, `rfc4475`, `soak`.
- [x] The link is seeded and its faults are knobs: loss, duplication, reordering, latency
      distribution. The same seed replays the same trace.
- [x] Failing-first test: `a_retransmission_gets_through_a_link_that_drops_the_first_datagram` —
      the retransmission behaviour the transaction machines exist for, tested end to end without
      a socket or a sleep.

## Progress
- Done, both halves.
- **The timer queue is generic over its key and takes `now` as an argument.** It used to call
  `Instant::now()` inside `set`, which made it unusable by any driver but the one it was written
  for — and made "when was this retransmission scheduled?" a question you could only answer by
  sleeping. The endpoint reads the clock at the call site now and hands it in, which is the whole
  change: the queue has no opinion about what an instant means.
  - `clear_all`/`forget` became `clear_matching`/`forget_matching`. With an opaque key the queue
    cannot know which part of one identifies a transaction, so the caller says.
  - The generation-counter discipline is untouched and its tests still pass, adapted only where the
    signature moved.
- **The loopback link ships**, five years after the crate's own documentation started promising it.
  Loss, duplication, latency and jitter, all seeded — the same seed replays the same trace, so a
  failure found by varying the loss rate is one somebody can re-run.
  - **Reordering is not a knob**, and that is deliberate. Packets do not overtake because a path
    chose to shuffle them; they overtake because one took longer than another. Jitter produces
    reordering, and a separate probability would model the symptom instead of the cause — and would
    let a test observe an ordering no real path can produce.
  - Delivery order is tied down by a sequence counter, not left to the heap. Without it two
    datagrams scheduled for the same instant come out in an arbitrary order and a seeded trace is
    not reproducible after all.
- **The failing-first test is the point of the whole story.** A dropped INVITE, Timer A, a
  retransmission that gets through — over a socket that costs 500ms of real time and a hope that the
  loss you asked for is the loss you got. Here both the link and the queue take `now`, so it costs
  nothing and the loss is exact.
- Mutation-tested: a link that never drops, a link that ignores its seed, and a queue that reads the
  clock itself again each fail the test that names the behaviour.

## Notes
- Both items came out of the harness design in
  [sipx-clstr](https://github.com/codewandler/sipx-clstr)'s `CF-1`, which decided component by
  component what belongs in the kernel and what is cluster-specific; these two are the only pieces
  that moved ([ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md)).
  The virtual clock deliberately did **not**: sans-IO layers take fired timers by contract and
  must never see a clock, so there is no kernel `Clock` trait to add.
- Sharing the queue means the tokio driver and any other scheduler have one cancellation
  discipline between them instead of two that drift.
