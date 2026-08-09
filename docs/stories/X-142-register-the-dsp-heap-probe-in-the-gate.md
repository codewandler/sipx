---
id: X-142
title: Register the DSP heap probe in the gate
pillar: Build
status: backlog
priority:
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [ci, gate, sipx-audio, dsp, allocation]
predicate:
announcement:
note: after X-128 · `check-dsp-heap.sh` measures what `DSP-K9` could not, and nothing runs it automatically
---

# Register the DSP heap probe in the gate

## Goal

Make the heap measurement `X-128` built run by itself. `./scripts/check-dsp-heap.sh` holds every
built-in processor's heap to its declared `state_bytes` and catches two fixtures that break it, and
today it runs only when somebody remembers. A measurement nobody runs decays into a claim.

## Acceptance

- [ ] `./scripts/check-dsp-heap.sh` is a `gate.py` step and has a CI job, with `gate.py --check`
      accounting for it the way it accounts for every other step.
- [ ] The step reports an unavailable toolchain as exit `2` (incomplete) rather than `1` (finding),
      matching `check-wasm-kernel.sh` and the gate's own distinction.
- [ ] The added CI time is measured and stated in the story, not estimated. `heap-probe/` resolves
      its own lock and builds its own copy of `sipx-audio`, so this is not free.
- [ ] A deliberately broken declaration fails the new gate step, proved by running it — not by
      reasoning that it would.

## Progress

- Backlog. Filed by `X-128`'s implementor, which deliberately stopped short of registration:
  `scripts/check-wasm-kernel.sh` set the precedent that a prover can be documented, runnable and not
  yet a gate step, and registering means editing `.github/workflows/` and `gate.py` — shared files,
  in a wave with other implementors in flight. `gate.py --check` passed untouched at 50 steps over
  23 CI jobs, so nothing is currently inconsistent; what is missing is automation.

- Worth deciding here rather than assuming: whether this becomes its own CI job or joins an existing
  one. `heap-probe/` is outside the workspace and pulls its own dependency graph, which is the same
  cost `fuzz` and `wasm` pay, and the run itself is milliseconds — the build dominates entirely.

## Notes

- `heap-probe/src/main.rs` states what the measurement does and does not promise.
- `docs/designs/custom-call-dsp.md`, "How processor heap growth is measured", has the argument for
  why the probe is outside the workspace. Do not relitigate it here; `unsafe_code = "forbid"` is not
  negotiable and `X-128` proved by experiment that no in-workspace door exists.
- `docs/specs/custom-call-dsp.md` §9.1 and §11.3 carry the normative half.
