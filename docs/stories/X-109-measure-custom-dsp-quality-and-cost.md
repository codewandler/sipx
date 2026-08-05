---
id: X-109
title: Measure custom DSP quality and real-time cost
pillar: Build
status: backlog
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

- [ ] A versioned bounded corpus covers exact sample transforms, impulse/frequency response, silence,
      stationary/transient noise, speech plus noise, discontinuities and hostile amplitudes.
- [ ] Reports include effect-vector error, attenuation, speech damage, onset recovery, algorithmic
      delay, CPU, allocation/state high-water marks and deadline/glitch/drop counts.
- [ ] Thresholds are declared before the measured implementation run and split by sample rate,
      channel count, processor, execution profile and machine class.
- [ ] The same packaged conformance runner accepts built-in and external fixture processors and never
      silently omits unavailable hardware/profile cases.
- [ ] Runs are finite, supervised and leave zero calls, processor graphs, tasks and retained frames;
      raw evidence precedes generated summaries.
- [ ] Public claims link to the exact corpus/configuration/date and docs/provenance/full gate pass.

## Progress

- Backlog. M18 conformance and measurement after M-65, M-66 and M-68.
