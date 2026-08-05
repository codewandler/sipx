---
id: X-103
title: Remove endpoint responder hot-path congestion
pillar: Build
status: in-progress
priority: 1
design: docs/designs/comparative-load.md
epic: comparative-load
areas: [call, transport, performance, load, m14]
predicate:
announcement:
note: profile before tuning; exceed the current peer endpoint baseline without weakening correctness or cleanup
---

# Remove endpoint responder hot-path congestion

## Goal

Find and remove the endpoint responder bottleneck exposed by the neutral load profile, while keeping
the sans-I/O boundary, protocol behavior, bounded ownership and correctness predicates unchanged.

## Acceptance

- [ ] Reproducible profiling at the first unstable rates identifies CPU time, allocation/memory
      growth, queue pressure, retransmission amplification and the responsible hot paths before an
      optimization is selected.
- [ ] Focused benchmarks cover the hot path at the narrowest useful layers: parsing, transaction
      ingress/egress, dialog dispatch and responder orchestration as applicable to the profile.
- [ ] Peak RSS, CPU, descriptors/tasks and loopback I/O bytes/packets are measured; process I/O or
      syscall counts are included when the host exposes them without changing the capacity run.
- [ ] A failing-first regression benchmark or deterministic structural assertion protects the
      chosen optimization from accidental reversal.
- [ ] The optimized endpoint supports every rate through the current peer endpoint capacity point
      at 5/5 repetitions under the same machine, driver, seed, policy and profile, or the remaining
      gap is retained as measured evidence with a newly scoped follow-up story.
- [ ] No optimization introduces `unsafe`, network-input panics, an unbounded queue/task, a fixed
      sleep standing in for ordering, or I/O in a sans-I/O core crate.
- [ ] The full gate and the refreshed comparative-load evidence are green.

## Progress

- 2026-08-05: the retained pre-change run first became unstable at 256 calls/s, failed every
  repetition at 512 and completed only 22,479 of 61,440 offered dialogs in every 1,024 calls/s
  repetition. Those top-rate repetitions used 73.8–75.2 CPU seconds, peaked at 405–438 MiB RSS,
  timed out about 14,383 transactions, reported about 24,579 internal errors and required
  supervised termination. The 512 calls/s records show queue pressure, admission refusal and up to
  78,526 request retransmissions rather than hiding overload as lower offered load.
- A 15-second CPU capture at 1,024 calls/s (`perf.data` SHA-256
  `d96e84299b5513a49add9e4cf25cc08c4c3c8cfa5d011217f58026c217e4ba62`) attributed 19.85% of
  sampled cycles to `TransactionKey` equality, 19.74% to `Calls::reserve`, 4.42% to endpoint output
  execution and 38.24% to the receive-side libc path. The adjacent failed attempt saturated all
  8,192 active slots, admitted no complete dialog, amplified into 61,515 request retransmissions
  and 3,991 response retransmissions, and refused 23,994 invitations. That evidence selected four
  bounded changes: amortized dead-route sweeping, exact fixed-vocabulary timer removal, a bounded
  UDP receive queue/batch, and fair alternation between dispatch and worker completion.
- The review added two correctness guards around those changes: a forgotten timer key cannot revive
  a stale heap entry when a peer reuses the transaction key, and event capacities above Tokio's
  channel limit are typed pre-I/O refusals. A valid BYE delivered before its earlier ACK is answered
  immediately but surfaced in logical ACK/BYE order, preventing scheduler reordering from becoming
  a false dialog failure.
- The optimized dirty tree passed the 20-dialog preflight, the 100-dialog qualification and five
  bounded review repetitions at 1,024 calls/s. Every repetition offered, established and completed
  61,440 dialogs, drained to zero active dialogs, transactions, timers, endpoint tasks and retained
  events, and exited its process group without escalation. Setup p99 ranged from 0 to 3 ms, peak
  RSS from 426.6 to 471.8 MiB, combined CPU from 12.96 to 17.50 seconds and active-dialog high water
  from 90 to 251. Four repetitions recorded no retransmissions; the fifth recorded 92 request and
  35 response retransmissions without an error or incomplete dialog. These temporary records are
  provisional: five retained repetitions and the full ladder must be captured from an immutable
  revision before they may replace the published baseline.
- 2026-08-05: immutable run `056391cbff0d7d2345a61a11c93e15b3` retained all six rates at 5/5
  repetitions through 1,024 calls/s from revision `82fe7c8f85cf39a949d272db786b28372478961a`.
  All 604,800 measured dialogs completed and every repetition observed zero endpoint state before
  unforced process-group exit.
- 2026-08-05: an earlier incomplete run exposed a measuring-instrument defect: an invalid
  successful-coded packet was added to validated response totals before being classified as
  `invalid_message`. A failing-first regression now keeps malformed or out-of-order packets solely
  in the error evidence; the incomplete generated directory was discarded.
