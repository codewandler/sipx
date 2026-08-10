---
id: M-127
title: Bound frame_samples where the plan is validated
pillar: Media
status: ready
priority: 4
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [sipx-media, dsp, bounds]
predicate:
announcement:
note: after M-122 · the capability's ceiling is checked and the graph's own frame sizing is not
---

# Bound `frame_samples` where the plan is validated

## Goal

Refuse a graph whose frame sizing exceeds the contract's maximum at the moment the plan is
validated, rather than discovering it when a worker refuses the `Hello` built from it.

## Context

`M-122` bounded the ceiling a *peer* declares: a `Hello` naming `max_samples` above
`MAX_FRAME_SAMPLES` is now refused, so a runtime can no longer be told to size its buffers from a
number the other side chose. Before that, a peer could declare 4,294,967,295.

Fixing that exposed the mirror image on this side. `supervised::pump` computes its `max_samples` as
`u32::try_from(frame_samples).unwrap_or(u32::MAX)` from a `usize` that `GraphBounds::validate` never
bounds — only the *capability's* `max_frame_samples` is checked against the contract maximum. So a
configuration whose `frame_samples` exceeds 65,536 now emits a `Hello` its own worker refuses.

That is already the better failure: `WorkerLost` beats a silent multi-gigabyte allocation. But it is
a value refused two layers away from where it was accepted, reported as a lost worker, which reads
like an environment problem rather than a plan that was never admissible. `M-122`'s implementor
named this as the real fix and correctly placed it outside that story's fence.

The house rule this restores: a bound belongs at the door where the value is admitted, and a
validation that checks a declaration while ignoring the sizing derived from it is a validation with
a hole in it.

## Acceptance

- [ ] `GraphBounds::validate` refuses a `frame_samples` outside the contract's range, naming the
      value and the bound, and the refusal is typed like its neighbours.
- [ ] A failing-first test shows the current tree admitting a plan whose sizing is out of contract
      and failing later as a lost worker; after, it is refused at validation with nothing spawned.
- [ ] The supervised path keeps its own refusal — defence in depth, since the two doors admit values
      from different places — and a test pins that removing the plan-side check does not silently
      restore the old behaviour.
- [ ] No live-path behaviour changes for a plan that was already admissible.
- [ ] The gate is green.

## Progress

- Filed 2026-08-10 by the `M-122` implementor, from an adjacent finding while bounding the peer's
  declared ceiling.
