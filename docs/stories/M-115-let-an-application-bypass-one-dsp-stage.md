---
id: M-115
title: Let an application bypass one DSP stage without replacing the chain
pillar: Media
status: backlog
priority: 41
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [sipx-media, app-sdk, dsp, m18]
predicate:
announcement:
note: after M-67 · `BypassCause::Requested` is in the spec and nothing sets it
---

# Let an application bypass one DSP stage without replacing the chain

## Goal

Give `BypassCause::Requested` a producer, so that an application can take one stage out of a live
chain and put it back without replacing the whole chain and opening a new epoch.

## Why this is a story and not part of `M-67`

`docs/specs/call-dsp-graph.md` §5.3 has listed `BypassCause ::= Requested | …` since `M-64`, and
nothing in this workspace sets `Requested`: every bypass the runtime produces is a failure under
§6.1's miss budget. `M-67` built the application door — discovery, ordered chains, sample-boundary
parameters, typed events — and deliberately did **not** add a bypass verb, because its Acceptance
asks for bypass *events* rather than a bypass *request*, and a fourth verb would have widened a
surface that already carries three.

The gap is the same shape as `M-114`'s: a value the contract names with no producer behind it. It is
the defect `M-98`, `M-99` and `M-103` each closed once for the application contract, and it should
be closed here deliberately rather than left to be noticed.

The narrowing that makes this non-trivial: **a bypass is not currently reversible.** §6.1's bypass
is terminal for the stage — its input passes through for the rest of the generation, and an
application restores it by replacing the chain (`M-67` states this). A *requested* bypass an
application can undo is a second, reversible operation on the same word, and this story has to say
which one `Requested` means before it implements it.

## Acceptance

- [ ] `docs/specs/call-dsp-graph.md` §6.1 says whether a requested bypass is reversible, and §10
      says how an application asks for one and what it costs the audio.
- [ ] A live stage can be bypassed and reported `Bypassed { cause: Requested }` at a named position,
      under the same take a frame needs, with the frames after it carrying the discontinuity §6.2
      requires.
- [ ] The request names a generation and a stage index and is refused — changing nothing — for a
      stale generation, an unknown index, or a stage already in that state.
- [ ] `contains_overrun()` is unchanged by a bypass: containment is a property of which stages are
      installed, not of which are contributing.
- [ ] A verb and a completion event, if the application contract grows them, are held by
      `spec_tables.rs` to the same producer rule `M-67`'s five rows are.
- [ ] The full gate is green.

## Progress

- Backlog. Filed by `M-67` on 2026-08-09, which left the word without a producer on purpose.
