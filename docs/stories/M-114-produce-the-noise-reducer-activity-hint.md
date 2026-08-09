---
id: M-114
title: Produce the activity hint a noise reducer declares it consumes
pillar: Media
status: backlog
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
- [ ] Detector latency is accounted for in positions: a hint that arrives after the speech it
      describes is stated as such, and the policy says what it does about it.
- [ ] The predeclared corpus of `docs/specs/call-dsp-noise-reduction.md` §8 is measured with the
      hint wired and unwired, and the wired figures are better on at least the overlapping-speech
      condition or the wiring is not recommended.
- [ ] A wrong or absent hint degrades to `M-66`'s unhinted behaviour rather than to a refusal, and
      no observation, event or metric presents the reducer as a detector.

## Progress

- Filed by `M-66` on 2026-08-09. Blocked on `M-67` for the control surface.
