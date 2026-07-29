---
id: X-29
title: Stop asserting after a sleep in sipx-call and sipx-transport
pillar: Build
status: in-progress
priority: 3
design: docs/designs/media.md
epic: conformance
areas: [sipx-call, sipx-transport, sipx-media, tests]
note: found by X-28's sweep, then confirmed by a real red gate — dns.rs:553 raced a 50ms TTL against the scheduler and failed a gate that had nothing to do with the diff being merged
---

# Stop asserting after a sleep in sipx-call and sipx-transport

## Goal
Remove the second family of load-dependent tests `X-28`'s sweep found — a fixed
`tokio::time::sleep`, then an assertion that a message, a socket read or a counter has arrived — so
a red gate keeps meaning what `X-28` made it mean in the media path.

## Acceptance
- [ ] `dns::tests::an_expired_entry_is_not_returned` (`crates/sipx-transport/src/dns.rs:553`) stops
      racing a 50 ms TTL against real scheduling. **This one is not hypothetical**: it failed a full
      gate run on 2026-07-29 while three implementor worktrees were compiling, on its *first*
      assertion — the entry expired before the immediate read — and passed 5/5 in isolation
      immediately after. It is the tightest deadline in the workspace after `udp.rs:473`.
- [ ] The `sleep`-then-assert sites `X-28` enumerated are fixed or explicitly left with a reason,
      the way `X-28` handled the sites it left. The list is in that story and is not repeated here;
      it spans `sipx-call/tests/call.rs`, `events.rs`, `playback.rs`,
      `sipx-transport/tests/udp.rs` and `backpressure.rs`, and `sipx-media/tests/quality.rs`.
- [ ] **The cure is poll-until-condition, not `X-28`'s wait-for-count.** `X-28` fixed a
      *quantity* of audio, which is why counting worked. These sites wait for an *event* — an ACK
      arrived, a sequence advanced, a counter moved — so the shape is a deadline loop on the
      condition, with the deadline a bound on failure rather than a window to measure in.
- [ ] No assertion is weakened to achieve it. `X-28`'s standard: the check stays character for
      character, only the wait changes. A test that stops proving what it proved is not a fix.
- [ ] Tests whose assertion is *negative* — that nothing arrived — may keep a fixed window, and say
      so at the site. A window can only make an empty-assertion pass, so the failure mode is a
      missed regression rather than a flake; `X-28` established that split and it should hold here.
- [ ] Failing-first evidence: the chosen sites failing under artificial load, quoted from a real
      run. `X-28`'s method reproduces it — pin a few hundred spinners to a **single core**, because
      saturating all cores does not starve a `current_thread` runtime and does not reproduce this.

## Progress
- Not started.

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
