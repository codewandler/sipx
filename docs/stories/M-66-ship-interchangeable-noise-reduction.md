---
id: M-66
title: Ship interchangeable local noise reduction
pillar: Media
status: backlog
priority: 39
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [sipx-audio, dsp, noise-reduction, realtime, m18]
predicate:
announcement:
note: after M-63 · optional M-58 VAD input · provider-neutral contract and local baseline
---

# Ship interchangeable local noise reduction

## Goal

Reduce stationary/background noise through a replaceable processor contract with honest latency,
resource and speech-damage measurements.

## Acceptance

- [ ] A noise-reduction capability reports supported rates/channels, frame requirements, warm-up,
      algorithmic latency, tail, CPU/device support and whether it consumes optional VAD input.
- [ ] sipx ships a local baseline implementation while an external fixture implements the same
      processor contract and passes the same lifecycle/conformance tests.
- [ ] Adaptive state is bounded and isolated per call/direction; resets, discontinuities and noise
      profile changes are deterministic and never retain raw audio after teardown.
- [ ] Unsupported formats/devices or an unmet real-time profile produce typed refusal/bypass; no
      silent resampling, quality change, remote processing or model download occurs.
- [ ] A predeclared corpus measures noise attenuation, speech distortion, onset recovery, latency,
      CPU and memory across silence, stationary noise, transient noise and overlapping speech.
- [ ] SDK events and docs distinguish noise reduction from VAD, echo cancellation and recognition,
      and the full gate is green.

## Progress

- Backlog. Depends on M-63; VAD input is optional after M-58.
