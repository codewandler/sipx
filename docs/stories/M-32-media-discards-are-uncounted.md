---
id: M-32
title: Give the media path's discards the same counters the transport got
pillar: Media
status: ready
priority: 4
design: docs/designs/media.md
epic: media
areas: [sipx-media, sipx-transport, sipx-call]
note: X-18 counted every transport discard and refused this half rather than invent the answer — `sipx-transport` cannot depend on `sipx-media`, so it needs a shared crate underneath both or a parallel type of `ShedCounts`' shape; census below, including a DTMF digit dropped with neither a log nor a counter
---

# Give the media path's discards the same counters the transport got

## Goal
Make a discard in the media path as visible from outside the process as a discard in the transport,
so that an operator diagnosing one-way or choppy audio can read what was dropped instead of guessing.

## Acceptance
- [ ] **The layering decision is made and written down.** `sipx-transport` cannot depend on
      `sipx-media`, so media counters cannot simply join `Handle::counters`. There are two shapes and
      the story is choosing between them: a small crate underneath both carrying the counter types, or
      a parallel snapshot of `ShedCounts`' shape exposed from `sipx-media` and joined by whoever holds
      both. The repo has already started down the second road — `crates/sipx-call/src/dispatch.rs:292`
      carries a counter described in its own comment as the "same shape as `sipx_transport::ShedCounts`"
      — so *that precedent is the thing to either extend deliberately or replace deliberately.* Say
      which and why in the spec, not only in the story.
- [ ] **Every discard below is counted, and a test enumerates them** — the same standard `X-18` met for
      the transport, where the test walks the discard list so a new drop site cannot be added without
      appearing. Census taken by `X-18`, all in `crates/sipx-media/src/session.rs` unless noted:
      - Opus decode/encode: `:191`, `:235`
      - SRTP: `:1502`, `:1760`
      - SRTCP: `:2430`, `:2493`
      - foreign SSRC: `:2157`
      - **a DTMF digit dropped with neither a log line nor a counter: `:2578`** — the only one of the
        set that is currently invisible even in a trace
      - unknown payload type: `:2616`
      - `Clip::finish`: `:781`
      - five in `crates/sipx-media/src/ice/driver.rs`, two in `crates/sipx-media/src/ice/gather.rs`
- [ ] **No counter that cannot rise.** `X-18` deleted `DiscardCounts::adopted_late` rather than ship one
      structurally stuck at zero, because a counter reading zero tells an operator "this never happens",
      which is worse than silence. Where a drop site genuinely cannot reach a counter, it gets the
      written reason `X-18` established in `sip-transport.md` §12.1 instead — not a counter.
- [ ] **Where a counter can lie, it says so.** `sip-transport.md` §12.2 is the precedent: a counter that
      can be missed or double-counted under load states that where it is defined. The media path has
      real instances — a discard inside a codec callback is not on the same thread as the driver loop.
- [ ] **No metrics library, and no clock in the core.** `X-18` added counters with `std` atomics and no
      new dependency; hold that line. `sipx-sip` and `sipx-sdp` gain nothing.
- [ ] **The counters do not lie about load.** Do not assert a count after a sleep. `X-28`, `X-29` and
      `X-40` are all this failure, and alpha predicate 3 is load-bearing for the other six: wait *for* a
      count with a deadline.
- [ ] Failing-first test: name the assertion that fails while a media discard is invisible. The DTMF
      digit at `:2578` is the sharpest witness, because it is currently unobservable by any means.

## Notes
- **Filed from `X-18`'s refusal, and the refusal was right.** That story counted every transport discard
  and shipped the capture; it declined this half rather than thread `Arc<Meters>` through crates it was
  fenced out of, and it did the census so the decision could be made with the sites in hand. The
  expensive part of this story is therefore already done.
- **The DTMF digit is the one to look at first.** Every other site in the census at least emits a
  `tracing` line, so a trace can find it. A digit dropped with neither a log nor a counter is
  indistinguishable from a digit the far end never sent — and DTMF is what an IVR is listening for, so
  the failure looks like a broken menu rather than a broken stack.
- **Why this is Media rather than Build.** `X-18` is a Build story because observability was the subject;
  here the subject is the media path's own honesty about what it throws away. The counters are the means.
- Reads with `X-18` (the transport half, the snapshot shape, and §12's spec text to extend rather than
  restate) and `M-31` (the other place the media path is quietly inconsistent).
