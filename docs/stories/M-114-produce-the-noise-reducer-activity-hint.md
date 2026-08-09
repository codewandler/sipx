---
id: M-114
title: Produce the activity hint a noise reducer declares it consumes
pillar: Media
status: in-progress
priority: 41
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [sipx-media, dsp, noise-reduction, vad, m18]
predicate:
announcement:
note: after M-66 and M-67 · the hint is declared and honoured; nothing in the workspace sets it
---

# Produce the activity hint a noise reducer declares it consumes

## Goal

Give the `voice_active` hint `M-66` declares an in-stack producer, so that a reducer's optional VAD
input is something a call can actually supply rather than a parameter only an application can reach.

## Context

`M-66` shipped `ActivityInput::Optional { parameter }` and `SubbandSuppressor`'s `voice_active`
flag: while it is set the noise floor does not adapt, which is the one thing the estimator cannot do
for itself. `NoiseReduction::validate` holds the claim to the schema, so the declaration cannot lie
about it.

What is missing is a producer. `M-58`'s analyser emits activity observations on the same call's
audio, and `docs/specs/custom-call-dsp.md` §10 already settles *how* the two may meet — "It may
consume activity as a declared parameter, never as a call" — but nothing in `sipx-media` carries an
observation from the analyser to a stage's parameter. Until something does, the hint is only
reachable by an application driving `M-67`'s control surface by hand, and `M-66`'s own documentation
says a hint that is wrong is worse than no hint, so a producer has to be measured before it is
recommended.

This is deliberately **not** `M-66`'s scope and **not** `M-67`'s: `M-66` owns the optional input,
`M-67` owns typed parameter control, and this owns the policy that decides when to set the flag and
proves it does not make speech worse.

## Acceptance

- [ ] Activity observations from one call's analyser reach that call's reducer as a declared
      parameter set at a stated position, through `M-67`'s control surface and no other door.
- [ ] The wiring is opt-in, per direction, and a call without it behaves exactly as it does today —
      proved by identical samples, not by inspection.
- [x] Detector latency is accounted for in positions: a hint that arrives after the speech it
      describes is stated as such, and the policy says what it does about it.
- [x] The predeclared corpus of `docs/specs/call-dsp-noise-reduction.md` §8 is measured with the
      hint wired and unwired, and the wired figures are better on at least the overlapping-speech
      condition or the wiring is not recommended.
- [x] A wrong or absent hint degrades to `M-66`'s unhinted behaviour rather than to a refusal, and
      no observation, event or metric presents the reducer as a detector.
- [ ] The gate is green.

## Progress

- Filed by `M-66` on 2026-08-09. Blocked on `M-67` for the control surface.

- **2026-08-09 — the policy, measured; the wiring still waits on `M-67`.**

  **Failing-first.** `cargo test -p sipx-audio --test dsp_noise_reduction --all-features` at
  `413f19c9e67708b82d7c3bfe48b8c3cb494af014`:

  ```
  error[E0432]: unresolved imports `sipx_audio::dsp::noise::ActivityHint`,
                `sipx_audio::dsp::noise::HintCause`, `sipx_audio::dsp::noise::HintPolicy`
    --> crates/sipx-audio/tests/dsp_noise_reduction.rs:36:5
     |
  36 |     ActivityHint, ActivityInput, HintCause, HintPolicy, HostRequirement, ...
     |     ^^^^^^^^^^^^                 ^^^^^^^^^  ^^^^^^^^^^ no `HintPolicy` in `dsp::noise`
  ```

  That is the defect stated as a compiler error: `M-66` declared the input and nothing in the
  workspace produced it.

  **What shipped.** `sipx_audio::dsp::noise::ActivityHint` (re-exported from `sipx_media::dsp`) —
  a sans-I/O policy that turns drained `analysis::Observation`s into the `voice_active` parameter
  the reducer's own declaration names. No clock, no allocation after construction, no call into
  either side; every decision is a comparison between caller-supplied positions and counts
  `HintPolicy` declares in samples. Recorded normatively as `docs/specs/call-dsp-noise-reduction.md`
  §10, vectors NR-V18..NR-V22.

  **The policy, and the measurement that decided it.** §8's four conditions plus one more
  (`sustained`: `overlapping`'s near talker with its gate held on, because §5.6's fifth row reasons
  about continuous speech and §8 has none), on `D8` at 160 positions per frame against the §11.1
  reference analysis profile. Attenuation / distortion in thousandths:

  | Condition | Unwired | Wired | Hinted | Unprot. | Overheld |
  |---|---|---|---|---|---|
  | `silence` | 1,000 / 0 | 1,000 / 0 | 0 | 0 | 0 |
  | `stationary` | 298 / 707 | 298 / 707 | 0 | 0 | 0 |
  | `transient` | 377 / 643 | 397 / 631 | 1,600 | 160 | 1,600 |
  | `overlapping` | 859 / 177 | 855 / 173 | 11,168 | 1,120 | 0 |
  | `sustained` | 611 / 392 | 594 / 407 | 11,168 | 1,120 | 0 |

  One row improves, two are sample-identical, two get worse. The improvement is on the condition
  §5.6's third row reasons about, which is the acceptance's bar — and the margin is 4 thousandths,
  so §10.5 recommends the wiring as opt-in and off by default rather than as an improvement.

  **The measurement changed the design twice.**

  1. *The warm-up deferral.* The first policy set the hint at the first `VoiceStarted`, which on
     `overlapping` lands at position 160 — inside the reducer's 1,024-position warm-up — freezing
     the floor §5.2 seeds from the first position of the epoch. That seed came from speech, which is
     exactly the floor §5.4's warm-up exists to distrust: distortion went from 177 to **497**. The
     policy now arms inside the warm-up and sets at the first boundary past the reducer's own
     declared `warm_up_positions`. 177 → 173.
  2. *Which direction to fail toward.* I had reasoned from §5.6's prose that a false inactive was
     the expensive direction and had the producer hold the hint through a `Lost` marker. The
     measurement says the opposite: on `overlapping`, a hint wrongly pinned true costs **420**
     thousandths of extra distortion and a hint wrongly absent costs **4**. Every uncertainty now
     resolves toward *not* hinting — `Lost` clears, the unrefreshed hold clears at 4,000 positions,
     a reset clears, and `release_positions` defaults to 0 so nothing is added to the analyser's
     hangover.

  **Detector latency.** Stated in positions and counted, not described. The leading edge is late by
  at least the detector's window `W` because a window's verdict does not exist until the window
  completes, and the policy cannot correct it — a reducer with zero declared latency has no
  lookahead. `unprotected_positions()` accumulates it. The trailing edge is late by at least the
  hangover; `overheld_positions()` accumulates that, and it is the expensive direction with no lever.

  **What waits on `M-67`.** Rows 1 and 2. `sipx-media` has no mid-call parameter path today —
  `GraphPlan::with_built_in` applies parameters once, before attachment (`crates/sipx-media/src/dsp/graph.rs:606`) —
  and that path is `M-67`'s Acceptance row 2 verbatim. Row 2's "identical samples" half **is** proved
  (`silence` and `stationary` are sample-identical wired and unwired, not merely equal in measure);
  its "opt-in, per direction" half is the wiring, so the row stays unticked. `ActivityHint` carries
  the direction it follows so the eventual wiring cannot cross an inbound detector to an outbound
  graph.

  **Owed CHANGELOG sentence** (fenced; for the coordinator to write):

  > Added `ActivityHint`, a deterministic sample-counted producer for the `voice_active` hint the
  > subband suppressor declares it consumes, with the corpus measurement that recommends it as
  > opt-in rather than by default (`M-114`).

  **Follow-up.** A story is owed for the `sipx-media` wiring once `M-67` lands — use `M-119`.

- 2026-08-10: shipped in `1.0.0-rc.18` and **left in-progress**. The policy, its vectors and
  its measured corpus are in the release; rows 1 and 2 are the `sipx-media` wiring, which
  waited on `M-67`. `M-67` landed in this same candidate, so the wiring is now unblocked and
  is filed as `M-119` — this story closes when that lands and its two rows can be ticked
  against real samples rather than against inspection.
