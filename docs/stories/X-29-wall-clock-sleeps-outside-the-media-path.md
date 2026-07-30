---
id: X-29
title: Stop asserting after a sleep in sipx-call and sipx-transport
pillar: Build
status: done
design: docs/designs/media.md
epic: conformance
areas: [sipx-call, sipx-transport, sipx-media, tests]
predicate: 3
note: found by X-28's sweep, then confirmed by a real red gate — dns.rs:553 raced a 50ms TTL against the scheduler and failed a gate that had nothing to do with the diff being merged
---

# Stop asserting after a sleep in sipx-call and sipx-transport

## Goal
Remove the second family of load-dependent tests `X-28`'s sweep found — a fixed
`tokio::time::sleep`, then an assertion that a message, a socket read or a counter has arrived — so
a red gate keeps meaning what `X-28` made it mean in the media path.

## Acceptance
- [x] `dns::tests::an_expired_entry_is_not_returned` (`crates/sipx-transport/src/dns.rs:553`) stops
      racing a 50 ms TTL against real scheduling. **This one is not hypothetical**: it failed a full
      gate run on 2026-07-29 while three implementor worktrees were compiling, on its *first*
      assertion — the entry expired before the immediate read — and passed 5/5 in isolation
      immediately after. It is the tightest deadline in the workspace after `udp.rs:473`.
- [x] The `sleep`-then-assert sites `X-28` enumerated are fixed or explicitly left with a reason,
      the way `X-28` handled the sites it left. The list is in that story and is not repeated here;
      it spans `sipx-call/tests/call.rs`, `events.rs`, `playback.rs`,
      `sipx-transport/tests/udp.rs` and `backpressure.rs`, and `sipx-media/tests/quality.rs`.
- [x] **The cure is poll-until-condition, not `X-28`'s wait-for-count.** `X-28` fixed a
      *quantity* of audio, which is why counting worked. These sites wait for an *event* — an ACK
      arrived, a sequence advanced, a counter moved — so the shape is a deadline loop on the
      condition, with the deadline a bound on failure rather than a window to measure in.
- [x] No assertion is weakened to achieve it. `X-28`'s standard: the check stays character for
      character, only the wait changes. A test that stops proving what it proved is not a fix.
- [x] Tests whose assertion is *negative* — that nothing arrived — may keep a fixed window, and say
      so at the site. A window can only make an empty-assertion pass, so the failure mode is a
      missed regression rather than a flake; `X-28` established that split and it should hold here.
- [ ] Failing-first evidence: the chosen sites failing under artificial load, quoted from a real
      run. `X-28`'s method reproduces it — pin a few hundred spinners to a **single core**, because
      saturating all cores does not starve a `current_thread` runtime and does not reproduce this.
      **Not achieved, and the premise is wrong** — `X-28`'s method does not transfer to this family.
      263 attempts across four sites under 600–1200 single-core spinners produced zero failures; see
      the Progress note for the numbers and for why the load that matters here is memory and IO
      pressure rather than CPU. `dns.rs:553`'s evidence remains the real red gate in this story's
      `note:`.

## Progress
- **Coordinator, resolving this section at the final integration.** The earlier note here said
  "Partial. Two of the seven files are done", and named `udp.rs:473` as the site to take first. Both
  were true when written and neither is now: every site is converted or left with a reason, and
  `udp.rs:473` turned out to be the one site that *cannot* be made load-proof without a transport
  change, for the reason recorded below. The obsolete text is dropped rather than left to mislead.
- **My fault, kept because it is still true and the next agent should not inherit the confusion:**
  `c38c381` on this branch is mine, not an implementor's. I committed a half-finished `quality.rs` to
  preserve work after an account limit killed the run, while it was in fact still resumable — which
  broke its isolation and made its own measurements contradict each other. Preserving an interrupted
  agent's work is right; doing it on a branch that may still have a live writer is not. It happened
  again in the other direction on the third attempt: that implementor found `649911d` committed
  underneath it by another writer holding the same branch in a second worktree.

**Every enumerated site is now either converted or left with a reason at the site, and the gate is
green: `18 steps, all green`.** Two earlier attempts were interrupted by an account limit; this is
the third, and it carries both forward — the `dns.rs` deadline loop from `impl/X-29`, and the
`quality.rs` conversion that only ever existed on `rescue/X-29-wip`.

### Why the clock was the wrong instrument, restated

The sweep's family is "sleep, then assert an arrival". Three different cures turned out to be
right, and which one applies is decided by *what the wait is actually for*:

1. **A happens-before already exists** — delete the wait. Five `call.rs` sites.
2. **An arrival with no ordering to lean on** — deadline loop on the condition. Eleven sites.
3. **A negative assertion, or a window that is itself the measurement** — keep the window, say so
   at the site. Six sites.

### Converted to a deadline loop on the condition — 11 sites

- `sipx-transport/src/dns.rs:553` — the site that actually turned a gate red. Two stores now: a
  60 s TTL for the precondition read, which proves the entry was stored without racing anything,
  and the real 50 ms TTL for the expiry, which is *waited for* rather than slept past.
- `sipx-media/tests/quality.rs` ×3 (`:69/:100/:226`) — wait on `packets_received()` reaching the
  number the test hand-sent, via a local `until(within, what, condition)`.
- `sipx-media/src/session.rs` ×2 (`:3184`, `:3456`) — the same, for the two tests asserting
  `cumulative_lost` and `extended_highest_sequence` *exactly*. A straggler there does not degrade
  the answer, it reports loss nobody injected.
- `sipx-call/tests/call.rs` ×4 (`:383` ACK-then-BYE, `:767` the CANCEL, `:821` its branch, `:994`
  the invitation) — the sites whose far end is a recorder task behind a real socket.
- `sipx-transport/tests/backpressure.rs:91` — wait for the receive loop to have shed something.
- `sipx-call/tests/playback.rs:198` — the one *positive* assertion among the stop bounds, that the
  far end received every packet this side counted as sent.

`playback.rs:100`'s `hearing()` was already the right shape and only needed its deadline widened
from 2 s to 10 s, so that it is a bound on failure rather than a number close to the honest answer.

### Waits deleted outright, because the ordering already existed — 5 sites

`call.rs:566`, `:589`, `:599`, `:681`, `:1238`. All five sleep after `callee.reinvite(…).await`, and
none of them needed to. `reinvite` returns only once the 200 has come back (`call.rs:1015`), and
`on_reinvite` applies the direction and records the remote CSeq *before* it responds
(`call.rs:520-530` then `:564`) — inside a `handle` call across which the test's pump holds the
call's mutex. So `caller.lock().await` on the next line **is** the synchronisation. The sleeps were
guessing at a happens-before the exchange already guarantees; converting them to deadline loops
would have kept a clock in a place that needs none.

### Left as fixed windows, deliberately — 6 sites

- `playback.rs:198/239` — the 600 ms windows. Every assertion they bound is negative (no more than
  `STOP_BOUND_PACKETS` went out; the whole clip did not). Load can only push the count up, which is
  the direction that fails, and there is nothing to poll for: the claim *is* that nothing further
  happens.
- `quality.rs:118` `the_round_trip_is_absent_until_a_report_comes_back` — same shape, nothing came
  back.
- `events.rs:233` — orders two inputs the test injects, and **nothing observable sits between
  them**: a raw 180 moves no counter and sends nothing, and the `Call` whose event queue would show
  it does not exist until `dial` returns, after both. Widened 100 ms → 2 s, exactly as `X-28` did
  for the two sites it could not count.
- `udp.rs:473` — the one enumerated site left near its margin, and the interesting one. There the
  50 ms bound **is** the assertion: it separates "already on the wire" from "merely queued", and a
  queued send would be flushed by the send loop within a packet interval. Any bound generous enough
  to survive load would therefore also pass against the defect the test exists to catch — the
  weakening this story forbids. Removing the clock entirely (`try_recv_from`, strictly stronger) was
  considered and rejected: loopback delivery completes in a softirq the kernel may defer under load,
  so a non-blocking read can legitimately see nothing, trading a rare flake for a commoner one.
  Making this site load-proof needs an observable `respond` does not expose — a counter, or a
  `flushed` signal — which is a change to the transport rather than to its test, and so belongs in
  its own story rather than being bodged in here.
- The 3–20 ms spacing inside the various send loops is left throughout and relabelled as *pacing*
  rather than waiting: it spreads hand-sent packets into a stream, and load lengthening it changes
  nothing any of these tests assert.

### On the failing-first evidence, and a correction to the story's premise

`X-28`'s method — a few hundred spinners pinned to one core — does **not** reproduce most of this
family, and the reason is worth writing down because it changes how the remaining risk should be
read: **a `tokio::time::sleep` dilates with the load.** The sleeping task is not competing for the
CPU it is being denied, so under 900 single-core spinners the `dns.rs` test's 130 ms of sleeps
became 3.0 s of wall time — the window grew ~20×, and so did the work it was covering. Measured,
base binaries, 900–1200 spinners pinned to one core:

| site | attempts | failures |
|---|---|---|
| `dns.rs::an_expired_entry_is_not_returned` (50 ms TTL) | **240** | **0** |
| `call.rs::a_2xx_the_caller_cannot_use_is_still_acknowledged` (300 ms) | 10 | 0 |
| `backpressure.rs::a_request_dropped_for_backpressure_is_counted` (300 ms) | 10 | 0 |
| `quality.rs` ×3 (300 ms), from the first attempt | 3 | 0 |

The 240 `dns.rs` attempts were 600 spinners plus 30 concurrent copies of the test, all pinned to one
core — far heavier than the conditions under which it actually failed. **So this Acceptance item is
left unticked: no site in this family was reproduced failing to order, including the one that has
really failed.**

That is worth stating precisely rather than dressed up, because a first guess of mine was wrong too.
`dns.rs` looked structurally different — its deadline is `Instant::now()` against a real 50 ms TTL
rather than a sleep, so real time does not dilate while the work racing it is starved — and that
asymmetry is real, but it is evidently not sufficient: 240 attempts under worse CPU starvation than
the incident produced nothing. The distinguishing feature of the incident was not CPU contention at
all. It was **three concurrent compilations**: memory pressure, page-cache eviction and IO wait, so a
process can stall on a major fault for tens of milliseconds without being CPU-starved at all. CPU
spinners do not create that stall, which is why they do not reproduce this, and why `X-28`'s method —
correct for its own story, where the starved thing was a `current_thread` runtime doing real work —
does not transfer here.

So the honest position: the conversions are right on their merits — a window is doing no work the
condition cannot do better, and eleven sites now fail with a message naming what did not happen
instead of flaking — but the evidence for the family is *one real red gate*, recorded in this story's
own `note:`, not a reproduction. A successor should not spend another day on spinners. If this
family is ever to be falsified on demand, the load to build is a memory-and-IO one — several
concurrent `cargo build`s, or a deliberate page-cache thrash — not a busy loop.

### Two process failures a resuming agent should know about

(1) The first worktree for this story (`.claude/worktrees/agent-a3d4e12c9f957ef1b`) was deleted
mid-task while the disk was at 100%, taking uncommitted work with it; the `dns.rs` fix was recovered
only because it happened to be in a dropped stash whose commit object survived. (2) An external
actor committed `c38c381` onto `impl/X-29` *while an agent was working in it*, capturing a
half-finished `quality.rs`. Commit early on this story; do not assume the worktree or the branch is
yours alone.

**And a third, for whoever runs the gate next.** Free space on this machine hit zero twice during
this attempt. Cargo's symptom is a *misleading* `ENOENT` — `taskset: failed to execute
target/debug/deps/…: No such file or directory` — which reads exactly like a code error. It cost a
falsification run that reported `FAILED 30 of 30` when the binary had simply been deleted underneath
it. Copy any binary you intend to run repeatedly out of `target/` first, and check `df -h .` before
believing a build error.

## Notes
- **Found by `X-28`'s sweep and then confirmed the hard way.** `X-28` classified 46 sites, converted
  30, and named this second family as out of its own scope — correctly, since the cure differs and
  the sites live outside the media path it owned. Within the hour, `dns.rs:553` turned a green
  merge red during `X-28`'s own integration gate.
- **That incident is the argument for the story.** The gate reported three failed steps — `test`,
  `msrv` and `app contract end to end` — and every one was load-induced: `msrv` and the app
  contract passed on re-run untouched, and the single test failure was in `sipx-transport`, a crate
  the merged diff never opened. The merge was very nearly reverted for a defect it did not have,
  which is exactly the cost `X-28` was filed to stop paying: *a test that fails at random trains
  everyone to re-run the gate instead of reading it.*
- Priority 3 rather than X-28's 4, because the media path is now clean and these are what is left
  standing between a red gate and a real signal — and because the failure has now been observed in
  the integration loop, not just reasoned about.
- `playback.rs:100`'s `hearing()` is already a deadline loop and is the closest thing in the tree to
  the shape this story wants; it is a reasonable model to generalise, though its deadline is a
  positive-arrival wait rather than a bound on failure.
