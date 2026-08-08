---
id: X-128
title: Measure DSP processor heap growth
pillar: Build
status: backlog
priority:
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [testkit, sipx-audio, dsp, allocation, m18]
predicate:
announcement:
note: after M-63 · DSP-K9 reports heap growth `Unproven` because no counting allocator can be installed
---

# Measure DSP processor heap growth

## Goal

Turn the one bound the DSP conformance harness reports as unproven — heap growth inside a
processor's own state — into a measured figure, so `docs/specs/custom-call-dsp.md` §9.1's
`state_bytes` declaration is checked rather than trusted.

## Acceptance

- [ ] A bounded harness measures bytes allocated and peak live bytes across a processor's `prepare`,
      `process`, `flush`, `reset` and `cancel`, and holds them to the declared `state_bytes`.
- [ ] The measurement mechanism is stated with its cost: a counting global allocator needs `unsafe`,
      which is `forbid`den workspace-wide, so the decision between a narrowly scoped exception, a
      separate unpublished measurement crate and an external tool is written down and justified
      before it is implemented.
- [ ] `DSP-K9` reports a measured figure where the mechanism is available and keeps reporting
      `Unproven` — never a pass — where it is not, per §11.1.
- [ ] A fixture that allocates per frame is caught, alongside the existing `ScratchHog` fixture that
      only exceeds its declared scratch.
- [ ] Public API docs, the story's failing-first test and the full gate are green.

## Progress

- Backlog. Found while implementing `M-63`: the conformance harness makes the *workspace* bound
  structural — scratch and the output sink are caller-owned and sized from the declaration, so a
  processor cannot obtain more than it declared whatever it does — and measures the processor's
  inline size against `state_bytes`. Heap growth inside its own state is the remaining gap, and
  `DSP-K9` says so by name rather than passing.

## Notes

- `crates/sipx-audio/src/dsp/conformance.rs`, `check_allocation`.
- `docs/specs/custom-call-dsp.md` §9.1 states the limit and why it is stated rather than hidden;
  §11.1 states that an unprovable check is reported and never passed.
- `M-68` needs measured per-frame allocation budgets and will consume whatever this settles.

- 2026-08-08: renumbered from `X-126` on filing; that id was allocated in the same wave to the
  load-pair test that rejects a call under suite contention. Only the id moved.
