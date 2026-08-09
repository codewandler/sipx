---
id: X-109
title: Measure custom DSP quality and real-time cost
pillar: Build
status: in-progress
priority: 41
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [testkit, dsp, benchmark, documentation, m18]
predicate:
announcement:
note: after M-65/M-66/M-68 · exact effects, quality, cost, isolation and packaged conformance
---

# Measure custom DSP quality and real-time cost

## Goal

Turn effect correctness, noise-reduction quality and real-time safety into reproducible evidence
rather than qualitative claims.

## Acceptance

- [x] A versioned bounded corpus covers exact sample transforms, impulse/frequency response, silence,
      stationary/transient noise, speech plus noise, discontinuities and hostile amplitudes.
- [ ] Reports include effect-vector error, attenuation, speech damage, onset recovery, algorithmic
      delay, CPU, allocation/state high-water marks and deadline/glitch/drop counts.
- [x] Thresholds are declared before the measured implementation run and split by sample rate,
      channel count, processor, execution profile and machine class.
- [x] The same packaged conformance runner accepts built-in and external fixture processors and never
      silently omits unavailable hardware/profile cases.
- [x] Runs are finite, supervised and leave zero calls, processor graphs, tasks and retained frames;
      raw evidence precedes generated summaries.
- [ ] Public claims link to the exact corpus/configuration/date and docs/provenance/full gate pass.

## Progress

- Backlog. M18 conformance and measurement after M-65, M-66 and M-68.

### 2026-08-09 — the measurements this epic owed, and the one it did not take

Two figures were owed and one was refused. `M-65` left "measured frequency response, CPU and
allocation" to this story and `M-66` left row 5 unticked for "CPU and memory". **Frequency response
and memory are now measured. CPU is not, and the reason is on this machine rather than in the
code.**

**Frequency response — `sipx_audio::dsp::response`, and it needs no quiet box.** A packaged
magnitude sweep, built the way the conformance harness is built: it takes a *factory* rather than a
processor, so it runs a built-in and an application-supplied processor through the same door. The
probe is a shipped 65-entry quarter-wave Q15 table, the figure is
`round(1000·√(Σy²/Σx²))` over one 256-position block after eight settling blocks, and **every step
is integer arithmetic** — so a response is the same number on every machine, at any load, in debug
and in release. `docs/specs/call-dsp-effects.md` §10 is the recorded table; `tests/dsp_response.rs`
asserts all of it exactly.

What it found:

- **`cutoff_hz` is the half-power point, to the thousandth.** Both one-pole filters measure exactly
  707 — `⌊1000/√2⌉` — at 125, 250, 500, 1,000 and 2,000 Hz. §8.2 describes the coefficient table and
  says nothing about where the corner lands; until this run "cutoff" was a parameter name.
- **§8.4's owed number.** The extracted band reaches **667 thousandths of unity**, so `band_gain`
  4.0 peaks at **3,001** and 2.0 at **1,667** — `1000 + (band_gain − 1000)·667/1000` to the
  thousandth. A *cut* is shallower still: 336 measured where the same model says 333, because the
  band is not exactly in phase with what it is subtracted from. §8.4 now carries both.

**Predicted before the run, and separated from what was recorded after it.** Eleven `predicted_*`
and refusal tests were written and passing before a single figure was read out — identity and
polarity at unity, monotone one-pole slopes, a flat unity peaking filter, a peak strictly below its
`band_gain`, stereo equal to mono, the curve moving with the sample rate, and a typed refusal for
latency, tail, non-preserving length, a refused format and an out-of-range bin. The `recorded_*`
tests carry integers taken from the run and locked. The two are named apart on purpose.

**Shown able to fail.** The sweep is calibrated by a pure gain, which comes back at exactly 500 and
2,000; and it is shown catching a nonlinearity — the same peaking filter measures identically at
amplitude 1,024 and 8,192 and **differently** at 16,384, where its own lift reaches the clamp and
3,001 becomes 2,451. That is also why the corpus amplitude is a quarter of full scale.

**Memory — `X-128`'s mechanism, extended rather than duplicated.** `heap-probe/` now measures
`sipx.subband_suppressor`: **256 declared, 256 inline, 0 bytes of heap at peak, 0 allocated after
`prepare`**. It had never been measured, because `X-128` wrote that probe's processor list by hand
and `M-66` shipped two stories later. The list is now checked against `BUILT_IN_IDS` and
`NOISE_REDUCTION_IDS`, and **the check was shown failing**: with the reducer's line removed the run
prints `sipx.subband_suppressor ships and was not measured` and exits 1.

**CPU — the harness is built, and the figure is not taken.** `crates/sipx-audio/examples/dsp_cost.rs`
measures nanoseconds per thousand positions and parts per million of one core at 8,000 Hz, per
processor over `docs/specs/call-dsp-noise-reduction.md` §8's four conditions at their exact lengths.
It refuses three ways: a debug build, a one-minute load average above a tenth of the machine's
cores, and its own control workload drifting more than 10% between the start of the run and the end.
**Both load guards fired on this box** — a shared twenty-core machine running four other
implementors — so no figure is recorded anywhere:

```
dsp-cost: the one-minute load average is 7.61 against a ceiling of 2.00 (10% of 20 cores)
  — nothing was measured
dsp-cost: the control cost 112730 ns before the run and 146295 ns after — 29% apart against
  a 10% tolerance, so the box did not stay still and nothing was measured
```

The 10% ceiling is itself evidence-derived: it started at 25%, and three runs *inside* that ceiling
put `sipx.gain` at 9,044, 12,275 and 11,247 ns per thousand positions — a 36% spread on a figure
whose purpose is being compared with next month's.

**To take it, on an idle box:**

```sh
cargo build --release -p sipx-audio --example dsp_cost
./target/release/examples/dsp_cost                 # exits 2 and measures nothing if the box is busy
./target/release/examples/dsp_cost --json > docs/measurements/dsp-cost.json
```

Nothing was written to `docs/measurements/`. The shape of the answer, from `--under-load` runs that
are **not** measurements and are not recorded: the effects sit in the tens of ppm of one core and
the reducer in the hundreds, roughly three times more expensive on signal than on silence. Those
numbers are in this note's git history only, and a reader who wants them should run the command.

**One corpus, two generators, one set of checksums.** An example cannot import a test helper, so the
cost harness carries its own copy of §8's recurrences. `NR-V18` pins both to four FNV-1a checksums
— and found a real divergence on the day it was written: the example spliced `transient` with
`i16::MIN` where §8 uses `-i16::MAX`.

**Rows ticked, and the three not.**

- Row 1 (corpus) — `docs/specs/call-dsp-effects.md` §10's probe corpus plus §8's four conditions;
  exact transforms and hostile amplitudes were already `M-65`'s EFFECT-V1..V19 and the sweep adds
  frequency response and the clamp case.
- Row 2 (reports) — **unticked.** Attenuation, speech damage, onset recovery, algorithmic delay and
  allocation/state high-water marks are all measured; **CPU is not**, and deadline/glitch/drop counts
  belong to a graph run under load, which is `M-68`'s and is not attempted here.
- Row 3 (thresholds) — the `predicted_*` set is declared before the run and split by sample rate
  (8,000 and 16,000), channel count (mono and stereo), processor and execution profile; machine
  class is what `dsp_cost` reads and stamps.
- Row 4 (packaged runner) — the sweep takes any `FrameProcessor` through a factory and **refuses by
  name** rather than omitting: `LatencyNotZero`, `TailNotZero`, `LengthNotPreserving`,
  `FrameCeilingTooLow`, `BinOutOfRange`, `Format`, `Process`, `ShortOutput`.
- Row 5 (finite, supervised, no residue) — every run here is a bounded number of positions with no
  task, socket or call anywhere near it; raw evidence (`check-dsp-heap.sh`'s per-processor lines,
  the sweep's per-bin points) precedes every summary.
- Row 6 (public claims) — **unticked.** It needs the full gate pass, which is the integrator's, and
  a recorded cost figure, which is untaken.

**Gate row left unticked.** `./scripts/gate.py` was not run here — one gate per wave, by the
coordinator. What was run in this worktree, all green: `cargo test -p sipx-audio -p sipx-media
--all-features`, `cargo clippy` on both `--all-targets --all-features --no-deps -- -D warnings`,
`cargo fmt --all --check`, `cargo doc` on both `--no-deps --all-features`,
`./scripts/check-dsp-heap.sh`, `./scripts/check-audio-claims.py --check`,
`./scripts/check-fixed-sleep.py --check`, `./scripts/check-provenance.sh`,
`./scripts/check-app-surface.py --check`, `./scripts/check-cfg-callers.py --check`,
`./scripts/check-docs-links.py`, `./scripts/check-story-closure.py`.

**Owed CHANGELOG sentence** (the coordinator writes it; this story may not touch that file):
*Added: a packaged magnitude-response sweep (`sipx_audio::dsp::response`) that measures any
processor's frequency response in exact integer arithmetic, and `crates/sipx-audio/examples/dsp_cost.rs`,
which measures per-processor CPU and refuses to report one on a loaded machine. `docs/specs/call-dsp-effects.md`
§10 records the measured response of the three filters — `cutoff_hz` is the half-power point exactly,
and a `band_gain` of 4.0 peaks at 3.001 rather than 4. `./scripts/check-dsp-heap.sh` now covers
`sipx.subband_suppressor` (0 bytes of heap) and fails if a shipped processor is missing from it.*

**Follow-up filed:** `X-143` — take the quiet-box cost run and record it under `docs/measurements/`.

- 2026-08-10: shipped in `1.0.0-rc.18` and **left in-progress**. The response sweep, the
  memory figures and the allocation high-water marks are in the release. Rows 2 and 6 stay
  unticked because per-processor CPU is still untaken: the harness exists and refuses to
  report a figure from a loaded machine, which is the honest outcome on this box rather
  than a gap in the code. The story closes when a quiet machine runs it.
