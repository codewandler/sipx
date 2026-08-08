---
id: T-44
title: Stop the last admitted load call racing its own cleanup
pillar: Transport
status: in-progress
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

- [x] A `load --calls 1` run against a peer that answers reports `connected: 1`, not a timeout.
- [x] A failing-first test proves it at the smallest bound, and a second asserts the bound still
      ends the run promptly — the fix must not turn "reached the call bound" into "wait for the
      full setup deadline".
- [x] A process stop still cancels calls in flight immediately; only reaching a configured bound
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

- 2026-08-08: implemented on `impl/T-44`. One line moved, and it is the whole defect:
  `run_bounded`'s `stop.request()` between the admission loop and the drain. Reaching a `calls` or
  `duration` bound now closes admission and nothing else — what ends the run is the `JoinSet`
  emptying, which is the calls themselves ending. An interruption is untouched: it already holds
  the signal when the loop observes it, so the calls in flight see it the moment it is made.

  **Failing-first, at `2d2f576`** (this worktree, its own target directory):

  ```
  $ cargo test -p sipx-call --all-features --lib load::
  test load::tests::the_call_that_reached_the_bound_is_measured_rather_than_cancelled ... FAILED
  assertion `left == right` failed: the smallest run that can be asked for reports the call it
  placed: Outcome { attempted: 1, succeeded: 0, failures: {Timeout: 1}, setup: [], … }
    left: 0
   right: 1

  $ cargo test -p sipx-cli --all-features --test cli
  test a_one_call_load_run_reports_the_call_its_peer_answered ... FAILED
  assertion `left == right` failed: generated-media:
    load:      {…"outcomes":{"attempted":1,"connected":0,…,"timed_out":1}…}
    responder: {…"counts":{"admitted":1,"established":1,"completed":1,…}…}
    left: Number(0)
   right: 1

  test a_load_run_that_reaches_no_address_reports_what_it_attempted ... FAILED  (--calls 1)
  test load_reaches_a_target_whose_first_address_is_dead ... FAILED             (--calls 1)
  ```

  The process test asserts the peer's summary beside the generator's on purpose: the responder
  admitted, established *and completed* the dialog while `load` reported a timeout for it, so the
  two records disagreed about a call that demonstrably happened.

  Measured before the fix, three runs each, against `sipx load-responder --calls 1`:
  `--mode generated-media --calls 1` gave `connected: 0, timed_out: 1` every time; `--calls 2` gave
  `connected: 1, timed_out: 1` — the under-report by one, exactly. `--mode signalling --calls 1`
  gave `connected: 1` and was **green at the base**, which is worth recording rather than glossing:
  it survives only because `cancel_signalling_invite` waits for the INVITE's first observation and a
  peer whose first message is the 200 hands that back. One provisional response, or a first
  candidate that refuses, and it lost the call too — which is precisely the shape the story's own
  measurement caught over TCP.

  Three behaviours changed, each deliberate:

  1. **A bound waits for the calls it admitted.** With `--call-duration` set, those calls now serve
     their configured holding period instead of being truncated at the bound; a run's wall time can
     therefore grow by up to one call duration. That is the number the operator asked for.
  2. **The drain has two phases, funded by one budget.** If the calls a bound admitted overrun
     `cleanup`, they are asked to end — the same signal an interruption sends — and the
     acknowledgement is funded for `cleanup` again before anything is aborted. Overrunning costs a
     call its holding period, not its protocol teardown, and the worst case is two budgets rather
     than one. `cleanup_ms` in the summary is unchanged and still names the budget.
  3. **`T-42`'s workaround is gone.** Both of its process tests are back at `--calls 1` and both
     were red at the base with that value.

  No published count changes meaning. `attempted`, `connected`, `rejected`, `timed_out`, `failed`
  and `peak_concurrency` all count what they always counted; the last admitted call simply now lands
  in the column that describes it.

  Owed to `CHANGELOG.md` (not edited here — the coordinator owns that file): *`load` no longer
  cancels the call that reached its `--calls` or `--duration` bound: a bound closes admission and
  the calls it admitted are waited for, so `--calls 1` reports the call it placed and no bounded run
  under-reports by one. A process stop still ends calls in flight at once.*

  The board is not regenerated here, and the `gate` row is left unticked — this worktree ran the
  scoped verification the dispatch named, not `./scripts/gate.py`.
