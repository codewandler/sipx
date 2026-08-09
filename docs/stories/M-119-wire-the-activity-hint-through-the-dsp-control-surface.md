---
id: M-119
title: Wire the activity hint through the DSP control surface
pillar: Media
status: done
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

- [x] Activity observations from one call's analyser reach that call's reducer as a declared
      parameter set at a stated position, through `M-67`'s control surface and no other door.
- [x] The wiring is opt-in, per direction, and a call without it behaves exactly as it does today —
      proved by identical samples on a live session, not by inspection.
- [x] A call whose analyser is bound to the other direction cannot be wired to the graph, and the
      refusal is typed rather than a hint that is quietly about the wrong audio.
- [x] The per-frame parameter set does not put work, allocation or a callback on the media worker,
      and the bound is asserted rather than argued.
- [x] `docs/specs/call-dsp-noise-reduction.md` §10.2's placement is what the call layer actually
      does, and a vector proves the hint lands on the frame boundary the spec names.
- [x] The gate is green.

## Progress

- Filed by `M-114` on 2026-08-09. Blocked on `M-67` for the parameter path.

- 2026-08-10: unblocked. `M-67` landed in `1.0.0-rc.18`, so the parameter path this story waited on
  exists. No implementation has begun; what follows is the integration path traced against the
  source at `5492b05`, so the first session on this does not re-derive it.

  **The four pieces, and the one join that is missing.**

  | Piece | Where | What it gives |
  |---|---|---|
  | `AudioAnalyzer::process` / `drain` | `sipx-audio/src/analysis.rs:1408,1473` | `Observation`s from a call's frames |
  | `ActivityHint::observe` / `parameter` | `sipx-audio/src/dsp/noise/hint.rs:380,327` | `Option<HintChange>` → `Option<Parameter>` |
  | `DspGraph::configure(generation, processor, parameters)` | `sipx-media/src/dsp/mod.rs:277` | `M-67`'s door; one terminal outcome, refusals leave the graph untouched |
  | `MediaSession::attach_processor` | `sipx-media/src/processing.rs` | `PcmFrame`s at the direction-aware seam |

  Every link exists **except** the one that makes it a property of a call: today an application
  drains the analyser, drives the hint and calls `configure` itself, which is precisely the "assembled
  by hand" this story's Goal refuses.

  **What that means for the Acceptance rows.** Row 1 is a new call-layer join, not new policy — the
  policy is `M-114`'s and settled. Row 3's typed refusal has a natural home: `ActivityHint::direction`
  (`hint.rs:49`) already carries the direction it follows and a graph is bound to one at `prepare`,
  so the refusal is a comparison the wiring can make at attach time rather than a runtime surprise.
  Row 4's bound is the one to design for first: `configure` validates off the media path already, so
  the wiring must not add a drain, an allocation or a callback *on* the worker — the hint has to be
  driven from the side that already owns the frames.

  **Not started because it needs a full implementation session**, not a boundary slot: a new public
  surface in `sipx-media`, failing-first tests on a live session proving sample-identical audio when
  unwired, a vector for §10.2's frame boundary, and a gate run.

- **2026-08-10 — the join is built; §10.2's vector is not yet written.**

  *Failing-first.* `crates/sipx-media/tests/dsp_activity_wiring.rs` stated the defect the way `M-114`
  did, as the compiler states it:

  ```
  error[E0432]: unresolved imports `sipx_media::dsp::ActivityWiring`, `sipx_media::dsp::WiringError`
    --> crates/sipx-media/tests/dsp_activity_wiring.rs:21:23
  ```

  *What shipped.* `sipx_media::dsp::ActivityWiring` joins the four pieces that already existed —
  the seam's frames, `AudioAnalyzer`, `M-114`'s `ActivityHint` and `M-67`'s `configure` door — and
  `WiringError::DirectionMismatch` refuses a hint and a graph that follow different directions **at
  construction**, so a wiring that cannot be right is not a thing that exists.

  *Rows 1–4 are ticked against four tests.* Updates reach the reducer through `configure` and no
  other door; an inbound hint on an outbound graph is refused by name; a call whose detector never
  calls voice carries **sample-identical** audio wired and unwired; and the graph's own counters show
  no deadline miss and no bypass, which is row 4's bound asserted rather than argued. Nothing runs on
  the media worker by construction: `run_until_idle` is synchronous, drains what the seam already
  holds and owns neither a task nor a clock, so the caller decides when the hint is driven.

  *Two things the tests found, both worth keeping.* A constant sample level is not speech however
  loud it is — a detector measures deviation within a window and a flat line has none — so the
  fixture had to carry a signal with energy in it. And a `NoiseReduction` assembled by hand declares
  **no activity input**, and a reducer that consumes nothing is wired to nothing: the hint would have
  been correct, silent, and indistinguishable from a broken join. The fixture now takes the
  declaration from `SubbandSuppressor` itself.

  *What is left.* Row 5 — `docs/specs/call-dsp-noise-reduction.md` §10.2's placement asserted as a
  vector proving the hint lands on the frame boundary the spec names — is not written, and row 6 is
  the gate. Until row 5 lands this story stays open, and so does `M-114`, whose first two rows are
  this story's first two.

- **2026-08-10 — row 5 written, and it found a defect rows 1–4 could not see.**

  Asserting *where* an update lands proved the implementation was not doing what §10.2 says. It fed
  observations at the position each frame **began** at rather than the one it ended at, and it never
  called `advance_to` — so the warm-up deferral §10.2 spends a paragraph justifying was never armed.
  Every one of rows 1–4 passed throughout: updates still reached the reducer, the direction refusal
  still held, silence still carried identical samples and the worker still lost no deadline. The
  placement was wrong and nothing that measured *whether* it worked could tell.

  `WiringOutcome::boundaries()` now reports the position each applied update landed at, and the
  vector asserts every one is a frame boundary. Reported rather than described, because "it lands on
  a boundary" is a claim a caller can check and a comment is not.

- 2026-08-10: closed at `c05ec91`+ against a full gate run on this tree.

