---
id: M-9
title: Make the jitter buffer adaptive
pillar: Media
status: ready
priority: 7
design: docs/designs/media.md
epic: depth
areas: [sipx-rtp]
note:
---

# Make the jitter buffer adaptive

## Goal
Let the buffer trade latency for loss according to the network it is actually on, rather than a
constant chosen at compile time.

## Acceptance
- [ ] Depth adapts to observed jitter, growing on loss and shrinking on a clean network.
- [ ] Shrinking does not discard audio: the buffer drains by playing faster only where it is
      inaudible, or waits for silence.
- [ ] Bounded above and below, so a pathological network cannot drive latency without limit.
- [ ] Measured against the fixed buffer on the same synthetic traces: strictly fewer packets
      late on a jittery trace, no more latency on a clean one.
- [ ] Failing-first test: `an_adaptive_buffer_loses_less_than_a_fixed_one_on_a_jittery_trace`.

## Progress
- Not started. `M-2` implemented the fixed buffer deliberately first, so this has something to
  be measured against.
