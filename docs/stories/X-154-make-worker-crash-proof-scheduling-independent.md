---
id: X-154
title: Make worker-process proofs scheduling-independent
pillar: Build
status: done
priority: 0
design: docs/specs/call-dsp-graph.md
epic: release
areas: [media, tests, release]
predicate:
announcement:
promotion:
note: preserve strict process-boundary evidence without assuming an operating-system scheduling order
---

# Make worker-process proofs scheduling-independent

## Goal

Make the supervised DSP process proofs assert the non-blocking frame-deadline contract rather than
an operating-system scheduling order, while retaining exact evidence for the process boundary,
malformed-result classification, worker loss, reaping, no restart and uninterrupted RTP.

## Acceptance

- [x] A failing-first parallel reproduction demonstrates that the first worker result may either
      meet or miss its non-blocking frame deadline while the process crashes on the next request.
- [x] The crash fixture budgets both permitted deadline histories before requiring `WorkerLost`.
- [x] The test accepts either permitted second-frame output but still strictly proves
      `WorkerLost`, process reaping, post-crash RTP continuity and a clear detach barrier.
- [x] PID and malformed-result proofs wait for their observable runtime events and treat intervening
      scheduling misses as the pass-through behavior the specification permits.
- [x] The all-features DSP worker test passes in a bounded 100-run parallel stress repetition.
- [x] Focused `sipx-media` tests, formatting and clippy pass.
- [x] The complete local acceptance gate passes.

## Progress

- 2026-08-14: the stable-v1 integration gate reproduced a parallel scheduling history in which
  request zero's result missed its non-blocking frame deadline. The worker-loss behavior remained
  correct; the test alone passed 30/30, the serial suite passed 30/30, and the parallel suite
  reproduced two timing-sensitive assertions in 50 bounded runs.
- 2026-08-14: the first repair held but a stronger repetition exposed the same assumption in the
  malformed-result proof at run 88/100. The final tests wait on PID-bearing audio and the runtime's
  malformed-result counter, retain the strict crash assertions, and pass 100/100 parallel worker
  binaries plus all 24 all-features `sipx-media` test targets. Formatting, clippy and the fixed-wait
  checker also pass; the coordinator owns the complete gate and final status transition.
- 2026-08-14: the integrated post-fix tree passed all 55 gate steps in 14m04s; this story closes.
