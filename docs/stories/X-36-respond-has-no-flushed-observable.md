---
id: X-36
title: Pin the send ordering `respond` already promises, and drop the clock that pretends to
pillar: Build
status: done
design: docs/designs/sip-transport.md
epic: conformance
areas: [sipx-transport, tests]
predicate: 3
note: respond_returns_only_once_the_response_has_been_sent cannot detect the thing it is named for — the ordering can be reversed in endpoint.rs and the test still passes, so its 50 ms bound is pure flake risk
---

# Pin the send ordering `respond` already promises, and drop the clock that pretends to

## Goal
Make `crates/sipx-transport/tests/udp.rs`'s
`respond_returns_only_once_the_response_has_been_sent` able to fail when that statement stops being
true, and remove the 50 ms wall-clock bound that currently stands in for a check it does not
perform.

## Acceptance
- [~] **A test that fails when the ordering is reversed.** `crates/sipx-transport/src/endpoint.rs:1387-1391`
      deliberately signals success *after* `perform`, and says why at the site: *"After performing, so
      a caller that exits on return has already put the response on the wire."* Nothing pins it.
      Moving `let _ = sent.send(Ok(()));` ahead of `self.perform(...)` leaves the test **passing** and
      the whole crate green — verified, 10 test-result lines, 0 failed. Whatever this story writes must
      go red under that mutation.
- [x] **The 50 ms bound goes.** It buys no detection power at any value: `perform` → `transmit` runs
      inline in the endpoint task (`endpoint.rs:1450`), so on a `current_thread` runtime the datagram
      is always out before the test task is polled again. The bound carries only the flake risk
      `X-28`'s sweep ranked as the most likely next `dns.rs`.
- [x] **The comment block at `udp.rs:472-486` is deleted, not edited around it.** It is a careful
      argument that happens to be false, which is the most durable kind of wrong comment. Its central
      claim — *"a queued send would be flushed by the send loop within a packet interval. Any bound
      generous enough to survive load would therefore pass against the very defect the test exists to
      catch"* — refutes itself: a packet interval is **20 ms** (`sipx-media/src/session.rs:265`,
      *"20 ms is universal"*), which is inside 50 ms, so a 50 ms bound was never generous enough to be
      the thing the argument describes.
- [x] Decide and record whether the ordering is a **public guarantee of `respond`** or an internal
      detail the test may reach behind. `docs/designs/sip-transport.md` is the place. The distinction
      matters: a documented guarantee is something applications may build on, and this one already has
      a reason written at the site — an application told its 200 went out when it did not leaves the
      caller timing out while the callee believes the call is up.
- [x] Failing-first test: reverse the two lines in `endpoint.rs:1387-1391`, run the suite, and quote it
      passing. That is the defect. Then name the test that catches it.

## Progress
- **Done, and the first Acceptance item is marked `[~]` because it was answered with something
  stronger than it asked for.** It asked for a test that fails when the ordering is reversed. There
  is no such test, and there cannot be: on a `current_thread` runtime sending on the oneshot does not
  yield, so `perform` completed before the waiting task was ever polled and the datagram was out
  whichever order the two lines were in. That is *why* the original test passed under mutation.
- **So the guarantee is structural.** `perform` now returns a `Performed`, and the `Ok` that `respond`
  reports is obtainable only by consuming it. Reversing the statements is a compile error, verified by
  doing it: `error[E0425]: cannot find value \`performed\` in this scope`. A compile error is a
  stronger pin than a red test, and it cannot rot.
- The 50 ms bound is replaced by a 10 s deadline, which is a bound on *failure* in `X-29`'s sense
  rather than a window to measure in. `respond_returns_only_once_the_response_has_been_sent` passes,
  and its name is now true.
- The decision that the ordering is a **public guarantee of `respond`** is recorded in
  `docs/designs/sip-transport.md`, with the reason: the alternative is the failure the code already
  names at its `NoTransaction` branch — telling an application its 200 went out while the caller
  heard nothing.
- Implemented by the coordinator rather than an implementor: all delegation was unavailable
  (org spend limit), and this story blocks alpha predicate 3.
- **This story was first filed with the wrong premise, by me, and the correction is the point.** The
  original text accepted `X-29`'s rationale that at this site *"the bound **is** the assertion"* and
  asked for a new `flushed` observable on `respond` so the clock could be removed. An independent
  review refuted that, and not by argument — by mutating `endpoint.rs` and showing the test passes
  either way. No observable is missing: `respond` already returns only after the datagram is out, on
  purpose, with the reason documented. What is missing is a test that can tell.
- Worth keeping in view: the false rationale was written at the site, ticked an Acceptance item in
  `X-29`, was accepted by me at integration, was repeated in `X-29`'s merge commit, and was then
  copied into this story as a criterion. Five places, one unchecked sentence. It was caught by
  someone running the mutation rather than reading the prose.

## Notes
- **Why the original framing was appealing and still wrong.** Every *other* site `X-29` converted
  waits for an event whose timing is a property of the machine, so widening the wait costs nothing.
  This one looked different because it appeared to distinguish *already on the wire* from *merely
  queued*. It does not distinguish them, because it cannot observe the difference at all.
- **`try_recv_from` was considered and rejected by `X-29`** on the grounds that loopback delivery
  completes in a softirq the kernel may defer under load, so a non-blocking read can legitimately see
  nothing. That reasoning is independent of the error above and still stands — do not reach for it as
  the fix.
- **Alpha predicate 3** is *"a red gate means a defect"*. This was the last enumerated site where a
  red gate could still mean "the machine was busy", and it turns out to be worse than that: a green
  gate here means nothing either.
- Priority 4 rather than the 6 this story was first filed at. It is still one test, but it is now a
  test that asserts nothing while claiming to assert an invariant with a real consequence, rather than
  a documented exception.
- Adjacent, from the same review and deliberately not folded in here: `sipx-media/src/session.rs:3184`
  and `:3456` poll `packets_received()` while their assertions read `stats()`, which are genuinely
  different observables (`received.fetch_add` at `:2258`, the stats update in `note_arrival` at
  `:2301-2313`). It is sound today and the site says so, but it states the guarantee without naming
  its precondition — no suspension point between the two, plus `current_thread`. Inserting a 20 ms
  sleep between them makes `a_session_reports_the_loss_it_saw` fail with `extended_highest_sequence
  left: 9 right: 10`, which is exactly the failure the comment claims the loop prevents.
