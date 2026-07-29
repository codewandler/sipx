---
id: X-29
title: Stop asserting after a sleep in sipx-call and sipx-transport
pillar: Build
status: ready
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
- [x] `dns::tests::an_expired_entry_is_not_returned` (`crates/sipx-transport/src/dns.rs:553`) stops
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
- [x] No assertion is weakened to achieve it. `X-28`'s standard: the check stays character for
      character, only the wait changes. A test that stops proving what it proved is not a fix.
- [x] Tests whose assertion is *negative* — that nothing arrived — may keep a fixed window, and say
      so at the site. A window can only make an empty-assertion pass, so the failure mode is a
      missed regression rather than a flake; `X-28` established that split and it should hold here.
- [ ] Failing-first evidence: the chosen sites failing under artificial load, quoted from a real
      run. `X-28`'s method reproduces it — pin a few hundred spinners to a **single core**, because
      saturating all cores does not starve a `current_thread` runtime and does not reproduce this.

## Progress
- **Coordinator, at integration:** merged as a partial and left `ready`. The one site with a real
  red gate against it is fixed and the gate is green at 18 steps; the remaining ~16 sites are listed
  below. **Take `udp.rs:473` first, not the list in order** — the implementor measured the
  `quality.rs` sites as far inside their margins, so `X-28`'s risk ranking should not be trusted as
  a work order. A 50 ms bound on a *positive* socket read is the one plausibly near its edge.
- **My fault, recorded so the next agent does not inherit the confusion:** `c38c381` on this branch
  is mine, not the implementor's. I committed its half-finished `quality.rs` to preserve work after
  an account limit killed the run, while it was in fact still resumable — which broke its isolation
  and made its own measurements contradict each other. Preserving an interrupted agent's work is
  right; doing it on a branch that may still have a live writer is not.

**Partial. Two of the seven files are done; the `sipx-call` half is untouched.**

Done:
- `sipx-transport/src/dns.rs` — the site that actually turned a gate red. Split into two stores: a
  60 s TTL for the precondition read (which no longer races anything) and the real 50 ms TTL for the
  expiry, now *waited for* in a deadline loop. Both assertions read character for character as
  before.
- `sipx-media/tests/quality.rs` — the three drains at `:69/:100/:226` now wait on
  `packets_received()` reaching the number the test sent, via a local `until(within, what,
  condition)` helper (the deadline-loop shape this story asks for). `:118`
  (`the_round_trip_is_absent_until_a_report_comes_back`) keeps its fixed window and now says at the
  site why: the assertion is negative, so a window can only make it pass. The 5 ms/3 ms spacing
  inside the send loops is left and labelled as pacing rather than waiting.

Not done — still fixed `sleep`-then-assert:
- `sipx-call/tests/call.rs` ×9 (`:382`, `:568`, `:591`, `:601`, `:683`, `:769`, `:823`, `:996`,
  `:1240`), `events.rs:233`, `playback.rs:184/225` and `:100`'s `hearing()`.
- `sipx-transport/tests/udp.rs:473` — the tightest bound in the workspace, and the one most likely
  to be the next `dns.rs`.
- `sipx-transport/tests/backpressure.rs:91`, `sipx-media/tests/session.rs:3162/3439`.

**On the failing-first evidence, which this story asks for and which I could not produce for the
`quality.rs` sites.** 250 spinners pinned to a single core, `quality.rs` at its pristine content,
binary verified pristine by `strings`: 3/3 passes. The test dilates only ~3.5× under that load
(0.7 s → 2.5 s), because it spends 380 of its ~700 ms *asleep* — and a wall-clock sleep does not
need the CPU it is being denied. The 300 ms drain is not racing the network, it is racing a
`current_thread` runtime draining a loopback socket buffer, which is microseconds of work. So these
three sites are real instances of the pattern but are **not** near their margins, and X-28's
estimate of their risk looks high. The conversion is still right — the window is doing no work the
condition cannot do better — but it should not be sold as a flake that was caught.
`dns.rs:553` is the opposite case and needs no artificial reproduction: it has a *real* red gate
against it, recorded in this story's own `note:`.

**Two process failures a resuming agent should know about.** (1) The first worktree for this story
(`.claude/worktrees/agent-a3d4e12c9f957ef1b`) was deleted mid-task while the disk was at 100%,
taking uncommitted work with it; the `dns.rs` fix was recovered only because it happened to be in a
dropped stash whose commit object survived. (2) An external actor committed `c38c381` onto
`impl/X-29` *while this agent was working in it*, capturing a half-finished `quality.rs`. Commit
early on this story; do not assume the worktree or the branch is yours alone.

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
