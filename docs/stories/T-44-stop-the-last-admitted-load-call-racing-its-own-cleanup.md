---
id: T-44
title: Stop the last admitted load call racing its own cleanup
pillar: Transport
status: backlog
priority: 3
design:
epic:
areas: [sipx-call, sipx-cli]
predicate:
announcement:
note: reaching the call bound requests cleanup immediately, so the call that reached it is cancelled mid-setup and `load --calls 1` can never report a connected call
---

# Stop the last admitted load call racing its own cleanup

## Goal

Let a load run's final admitted call be measured rather than cancelled, so the smallest reproducible
run an operator can type reports what happened instead of a timeout.

## Acceptance

- [ ] A `load --calls 1` run against a peer that answers reports `connected: 1`, not a timeout.
- [ ] A failing-first test proves it at the smallest bound, and a second asserts the bound still
      ends the run promptly — the fix must not turn "reached the call bound" into "wait for the
      full setup deadline".
- [ ] A process stop still cancels calls in flight immediately; only reaching a configured bound
      waits for the calls that bound admitted.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `T-42`'s adjacent findings. `sipx_call::load::run_bounded` breaks out of
  admission as soon as `admitted >= calls` and then calls `stop.request()` before joining anything
  — "A count or duration bound is also an instruction to end the calls it owns." The call that
  reached the bound is usually still setting up when that lands, and every wait in the workload
  selects on `stop.requested()` first (`biased`), so it is cancelled at once.

  Measured on 2026-08-08 at `1c7d1c4`, against a target whose second address answers:

  ```
  $ sipx load "sip:load@…" --transport tcp --rate 1 --concurrency 1 --calls 1 --timeout 5
  "outcomes":{"attempted":1,"connected":0,"failed":0,"peak_concurrency":1,"rejected":0,"timed_out":1}
  $ # the same run with --calls 2
  "outcomes":{"attempted":2,"connected":1,…,"timed_out":1}
  ```

  The second call of a two-call run loses the same race; only the earlier ones are measured. So
  every bounded run under-reports by one call, and the one-call run — the smallest thing an
  operator can type to check a target — reports nothing but a timeout.

  `T-42`'s process tests work around it by admitting two calls and asserting on the earlier one,
  with the reason stated inline; that workaround is what should be removed when this is fixed.
