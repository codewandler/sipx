---
id: M-127
title: Bound frame_samples where the plan is validated
pillar: Media
status: in-progress
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

- [x] `GraphBounds::validate` refuses a `frame_samples` outside the contract's range, naming the
      value and the bound, and the refusal is typed like its neighbours.
- [x] A failing-first test shows the current tree admitting a plan whose sizing is out of contract
      and failing later as a lost worker; after, it is refused at validation with nothing spawned.
- [x] The supervised path keeps its own refusal — defence in depth, since the two doors admit values
      from different places — and a test pins that removing the plan-side check does not silently
      restore the old behaviour.
- [x] No live-path behaviour changes for a plan that was already admissible.
- [ ] The gate is green.

## Progress

- Filed 2026-08-10 by the `M-122` implementor, from an adjacent finding while bounding the peer's
  declared ceiling.

- **Done bar the gate row** (2026-08-10, `impl/M-127`). `GraphBounds::validate` now takes the
  session's actual `frame_samples` and refuses a value outside `1..=max_frame_samples` as
  `FrameSamplesOutOfRange { value, bound }`. Admission passes that sizing through before prepare,
  allocation or worker spawn; the supervised worker's independent oversized-`Hello` refusal is
  unchanged.

  Failing-first, the new GRAPH-28 live-session test on the old path spawned the reference worker,
  which refused `max_samples = 66,048`; after three bounded frames the graph reported
  `WorkerLost`, and the test failed with `an out-of-contract graph was admitted and failed later as
  WorkerLost`. With the plan-side check present the same test returns the typed sizing refusal
  before it can obtain a worker pid. The portable bound test also pins both ends, `0` and `65,537`,
  and the existing worker-protocol test continues to pin the second door.

  Focused evidence: `cargo test -p sipx-media --all-features` passes; the crate itself is clean
  under `cargo clippy -p sipx-media --all-features --all-targets --no-deps -- -D warnings`;
  provenance, story-closure and whitespace checks pass. The integration coordinator owns the one
  complete wave gate and the final row.
