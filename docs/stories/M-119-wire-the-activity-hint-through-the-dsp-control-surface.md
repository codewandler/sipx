---
id: M-119
title: Wire the activity hint through the DSP control surface
pillar: Media
status: backlog
priority: 42
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [sipx-media, sipx-call, dsp, noise-reduction, vad, m18]
predicate:
announcement:
note: after M-114 and M-67 · the producer and its measurement exist; the per-call wiring needs M-67's parameter path
---

# Wire the activity hint through the DSP control surface

## Goal

Carry `M-114`'s `ActivityHint` from one call's analyser to that call's reducer over `M-67`'s
parameter path, so the hint a reducer declares is something a call can supply rather than something
an application assembles by hand.

## Context

`M-114` delivered the half that could be delivered without `M-67`: the policy, its sample-exact
vectors and the corpus measurement that decides whether the hint is worth setting at all —
`docs/specs/call-dsp-noise-reduction.md` §10. What it could not deliver is the wiring, and the
reason is mechanical rather than a matter of taste. `sipx-media` has no mid-call parameter path:
`GraphPlan::with_built_in` applies a parameter set once, before the plan is attached
(`crates/sipx-media/src/dsp/graph.rs:606`), and there is no second door. Setting a per-frame flag
through that door is impossible, and `M-67`'s Acceptance row 2 — "parameter updates are finite,
validated off the media path and applied at a declared sample boundary" — is exactly the door
required.

`M-114` left its first two Acceptance rows unticked for this, and both are inherited below rather
than restated: this story owns them and nothing else. The policy is settled, the measurement is
recorded, and §10.5's recommendation — **opt-in, per direction, off by default** — is what the
wiring has to make expressible rather than reopen.

Two things `M-114` already established that this must not contradict. `ActivityHint` carries the
direction it follows, so wiring an inbound detector to an outbound graph must not typecheck into
silence. And a stream the detector never calls voice must produce **sample-identical** audio wired
and unwired — `M-114` proves that at the `sipx-audio` seam for §8's `silence` and `stationary`, and
the same claim has to hold end to end on a call.

## Acceptance

- [ ] Activity observations from one call's analyser reach that call's reducer as a declared
      parameter set at a stated position, through `M-67`'s control surface and no other door.
- [ ] The wiring is opt-in, per direction, and a call without it behaves exactly as it does today —
      proved by identical samples on a live session, not by inspection.
- [ ] A call whose analyser is bound to the other direction cannot be wired to the graph, and the
      refusal is typed rather than a hint that is quietly about the wrong audio.
- [ ] The per-frame parameter set does not put work, allocation or a callback on the media worker,
      and the bound is asserted rather than argued.
- [ ] `docs/specs/call-dsp-noise-reduction.md` §10.2's placement is what the call layer actually
      does, and a vector proves the hint lands on the frame boundary the spec names.
- [ ] The gate is green.

## Progress

- Filed by `M-114` on 2026-08-09. Blocked on `M-67` for the parameter path.
