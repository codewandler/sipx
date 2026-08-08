---
id: M-45
title: Hold incoming audio in a jitter buffer
pillar: Media
status: ready
priority: 4
design: docs/designs/demand.md
epic: demand
areas: [sipx-rtp, sipx-media]
predicate:
announcement:
note: two independent field reports of seconds of added delay · characterise the current buffer before changing it
---

# Hold incoming audio in a jitter buffer

## Goal

Make incoming audio play out smoothly under network jitter, loss and reordering, with a buffer whose
depth is bounded and observable rather than growing into latency.

## Acceptance

- [ ] **Characterise the current behaviour first.** `sipx-rtp` has a jitter buffer; Progress records
      what it does today under jitter, loss, reordering and duplication, measured, before any change.
      A story that changes it without that baseline cannot show an improvement.
- [ ] A failing-first test drives a synthetic packet stream with controlled jitter and reordering
      through the buffer and asserts the play-out sequence, using virtual time — **not** a wall-clock
      sleep, per `scripts/check-fixed-sleep.py` and `docs/designs/media.md`.
- [ ] Buffer depth is bounded and does not grow monotonically under sustained jitter. The reported
      field failure is seconds of accumulated delay, so the test asserts a bound on added latency,
      not merely that audio is continuous.
- [ ] Late and duplicate packets are discarded rather than played, and sequence wrap is handled.
- [ ] Loss is concealed by a stated, documented strategy — silence insertion is acceptable if it is
      written down; an undocumented gap that produces a click is not.
- [ ] Buffer depth, discards and concealment events are visible through the existing counters, per
      the rule that shedding must never be invisible.
- [ ] `./scripts/gate.py` green, including `check-fixed-sleep.py`.

## Progress
- (not started)

## Notes
- Two independent field reports in the demand survey, one comparing unfavourably against a desktop
  softphone. It is the difference between "audio works" and "audio sounds right", and it is the kind
  of defect that never appears on a LAN.
- Related but distinct from `M-43`: this is play-out timing, that is format conversion.
