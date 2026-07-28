---
id: M-9
title: Make the jitter buffer adaptive
pillar: Media
status: done
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
- [x] Depth adapts to observed jitter, growing on loss and shrinking on a clean network.
- [x] Shrinking does not discard audio: the buffer drains by playing faster only where it is
      inaudible, or waits for silence.
- [x] Bounded above and below, so a pathological network cannot drive latency without limit.
- [x] Measured against the fixed buffer on the same synthetic traces: strictly fewer packets
      late on a jittery trace, no more latency on a clean one.
- [x] Failing-first test: `an_adaptive_buffer_loses_less_than_a_fixed_one_on_a_jittery_trace`.

## Progress
- Done. `JitterBuffer::adaptive(min, max)` alongside the fixed `new(depth)`, which stays as the
  control rather than being replaced — an adaptive buffer that cannot be shown to beat a
  constant is a constant with extra ways to go wrong. `sipx-media` uses it by default, bounded
  at 12 packets.
- Measured in `crates/sipx-rtp/tests/jitter_traces.rs`, both buffers on identical traces with
  the same playout clock. On a trace with recurring 95 ms spikes: **fixed 86 packets late, 513
  played; adaptive 3 late, 594 played**, ending at depth 4. On a clean trace the two are
  identical, byte for byte, and the adaptive one never leaves its floor.
- Shrinking is free at this layer, which is worth stating because the story anticipated it
  being hard: lowering the depth releases the next packet one slot sooner. Nothing is dropped
  and nothing is played faster. Time-scale modification belongs in the media layer.
- A bug the measurement caught that no unit test would have: the depth was derived as
  `min + ceil(2J / interval)`, and `ceil` of the floating-point residue left by a decaying
  exponential average is 1, not 0 — so on a network that had recovered completely the buffer
  sat permanently one packet deeper than needed and never gave it back. Deriving the depth from
  the release rule instead (`ceil(2J / interval) + 1`, clamped) has no such floor. The first
  version of the shrink test passed anyway; it asserted the depth was below the *ceiling*
  rather than below its own peak.