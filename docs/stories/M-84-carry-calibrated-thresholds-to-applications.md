---
id: M-84
title: Carry calibrated thresholds onto the application wire
pillar: Media
status: backlog
priority: 26
design: docs/designs/call-audio-analysis.md
epic: call-audio-analysis
areas: [sipx-call, sipx-app, app-sdk, audio-analysis, vad, m16]
predicate:
announcement:
note: after M-60 · the effective thresholds are Rust-only; an app-protocol client cannot see them
---

# Carry calibrated thresholds onto the application wire

## Goal

Let an application on the app protocol see what a call's voice-activity detection is measuring
against, and when it changed, so a calibrated threshold is as observable to an SDK client as it is
to a Rust caller.

## Acceptance

- [ ] A failing-first test proves an app-protocol client can read the effective thresholds of a call
      with detection running, and gets nothing for a call without it.
- [ ] Threshold movement is observable without polling: either a typed event or a field on an
      existing one, specified in `docs/specs/app-contract.md` as a compatible `v1` addition and
      derived-tested against that spec the way `call.voice.started` is.
- [ ] The wire surface carries no audio and no field an application could reconstruct audio from,
      and the check that would catch a regression there is named.
- [ ] Generated bindings and docs are updated and the full gate is green.

## Progress

- Backlog. Filed by `M-60`, which stopped at the Rust surface deliberately.

## Notes

- `M-60` shipped calibration with two inspection points: `sipx_audio::analysis::AudioAnalyzer::
  thresholds()` and `sipx_call::Call::voice_thresholds()`, both `&self`, both returning a `Copy`
  `EffectiveThresholds` carrying no audio (`docs/specs/call-audio-processing.md` §12.9). Neither is
  reachable from the app protocol, so an SDK client can be told voice started but not what the
  decision was made against.
- `Observation::ThresholdUpdated` already exists and is deterministic, so the event half of this is
  carriage rather than new analysis — the shape `M-58` used for `call.voice.started` /
  `call.voice.ended` applies directly (`docs/specs/app-contract.md` §5.3, §4).
- Weigh the event against the read: a threshold that settles is silent, so an event stream costs
  nothing in steady state, but a client that only ever wants "what now?" is better served by a read.
  `M-60`'s call-side channel is already latest-value, which suits both.

- 2026-08-08: renumbered from `M-80` on filing. `M-80` had already been allocated in the same wave,
  to the `#[non_exhaustive]` decision for `Encoded` and `Packet`. Only the id moved.
