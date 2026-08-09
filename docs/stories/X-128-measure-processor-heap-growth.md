---
id: X-128
title: Measure DSP processor heap growth
pillar: Build
status: in-progress
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

- [x] A bounded harness measures bytes allocated and peak live bytes across a processor's `prepare`,
      `process`, `flush`, `reset` and `cancel`, and holds them to the declared `state_bytes`.
      `Conformance::metered_life` drives construction, `prepare`, six `process` calls over two
      reset-separated epochs, two `flush`es and `cancel`; `check_allocation` holds peak live to
      `state_bytes` less the inline size. Construction is inside the window because `Stutter`
      allocates its line in `new`.
- [x] The measurement mechanism is stated with its cost: written up in
      `docs/designs/custom-call-dsp.md` "How processor heap growth is measured", with all four
      options and the evidence for each. Option A was *proved* impossible rather than assumed —
      `unsafe impl GlobalAlloc` in an integration test is refused, and `#[allow(unsafe_code)]` is
      `E0453` "overruled by previous forbid".
- [x] `DSP-K9` reports a measured figure where the mechanism is available and keeps reporting
      `Unproven` — never a pass — where it is not, per §11.1. Passes for all nine built-ins under
      `./scripts/check-dsp-heap.sh`; stays `Unproven` under `cargo test`, with the reason now naming
      the prover instead of stopping at "cannot".
- [x] A fixture that allocates per frame is caught: `HeapHog` (480 bytes after `prepare`), with
      `HeapLiar` (65,536 held against a 4,040-byte budget) for the size half. Both go back to
      `Unproven` with no meter, which is the control proving the meter is what catches them.
- [x] Public API docs, the story's failing-first test and the full gate are green — except the gate
      row below, which is the coordinator's to run.

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

- 2026-08-09: implemented. **Chose the external-measurement option**, and the first option was
  eliminated by experiment rather than by reading the manifest. `[workspace.lints.rust]
  unsafe_code = "forbid"` reaches rustc as `-F unsafe_code` for *every target of every member*, so
  `unsafe impl GlobalAlloc` in `crates/sipx-audio/tests/` is refused exactly as it is in the library,
  and `#[allow(unsafe_code)]` next to it is `E0453`, "overruled by previous forbid". A `cfg(test)` or
  dev-feature allocator is not merely discouraged here, it does not compile. Proving the bound
  structurally was rejected separately: `size_of::<P>()` sees a pointer and not its pointee, and
  `Stutter`'s delay line shows that forbidding owned heap would be the wrong contract.

  `heap-probe/` therefore sits outside the workspace, on the terms the root manifest already sets for
  `wasm/` and `fuzz/` — and for the same stated reason, "keeping the non-negotiable intact for every
  crate that answers to it". Nothing sipx publishes gains an `unsafe`, and
  `comparison-report.py`'s generated `unsafe-policy` cell, which reads `unsafe_code = "forbid"`
  straight out of the workspace manifest, stays true. The whole unsafe surface is one
  `unsafe impl GlobalAlloc` with two methods: `realloc` and `alloc_zeroed` are left at their defaults
  precisely because those defaults are written in terms of `alloc`/`dealloc` and so count correctly
  for free.

  The seam is one safe trait (`HeapMeter`) and one additive entry point
  (`Conformance::run_with_heap_meter`) — additive so no published signature changed at rc.16. The
  *arithmetic* stays in `sipx-audio` and is covered by `cargo test` through a scripted test-double
  meter; only the bytes come from outside. If the comparison lived in `heap-probe/`, a workspace
  refactor could stop calling it and nothing would notice.

  What the measurement found, none of which was previously checked: every built-in but `sipx.stutter`
  holds **exactly 0** heap bytes, and `sipx.stutter` at delays 1, 4 and 4,096 holds **exactly 4, 16
  and 16,384** — equal to its declared heap component to the byte, not merely within it. Those exact
  figures are also what proves the metered window is clean: the harness allocates nothing inside it,
  and a zero that came from a broken meter could not also produce 16,384 for one processor and 4 for
  another. `DSP-K9`'s second half, §9.1's "no allocation after `prepare`", had never been checked at
  all and now is; every built-in allocates 0 bytes there.

  `EFFECT-V19` and `dsp_effects.rs` are deliberately **unchanged**: under `cargo test` the unproven
  set is still exactly `[DSP-K9]`, because that run genuinely has no meter. Relaxing that assertion
  would have meant a meter reached the workspace.

  **CHANGELOG sentence owed** (fenced, for the coordinator): *`DSP-K9` now measures a processor's
  heap growth against its declared `state_bytes` instead of reporting it unproven — via
  `Conformance::run_with_heap_meter` and the out-of-workspace `heap-probe/`, which keeps
  `unsafe_code = "forbid"` intact for every published crate. `sipx.stutter`'s delay line is the
  first declared heap figure to be verified, and it is exact.*

  **Follow-up filed: `X-142`**, to register `./scripts/check-dsp-heap.sh` as a gate step and CI job.
  Left out here on the precedent of `scripts/check-wasm-kernel.sh`, which is documented, runnable and
  deliberately not yet a gate step; registering it means touching `.github/workflows/` and `gate.py`
  in a wave where other implementors are running, and `gate.py --check` passes untouched at 50 steps
  over 23 CI jobs. Until then the probe is run by hand and its figures are quoted in the spec.
  **The board needs regenerating for `X-142` — that file is fenced, so it is the coordinator's.**
