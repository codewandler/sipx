---
id: M-36
title: Reject zero media worker intervals before starting
pillar: Media
status: ready
priority: 6
design: docs/designs/media-runtime-safety.md
epic: media-runtime-safety
areas: [sipx-media]
predicate: 4
note: media portion of R-07 in the 2026-07-30 repository review — zero packet and mix periods kill tasks while a zero RTCP interval can hot-loop
---

# Reject zero media worker intervals before starting

## Goal

Validate media packet, RTCP and conference timing before workers start, so public zero values cannot
silently kill audio tasks or create a busy loop.

## Acceptance

- [ ] Specify valid timing ranges and their typed errors in the relevant media and conference specs
      before changing public constructors.
- [ ] Zero media packet duration, zero configured RTCP interval and zero conference mix interval are
      rejected before a socket or worker starts.
- [ ] Every public construction path reaches one validator; worker loops may rely on validated non-zero
      intervals rather than each inventing a fallback.
- [ ] Failing-first tests exercise each zero value through the public API, assert the typed error and
      prove no task, socket or timer remains active.
- [ ] Tests at the smallest valid duration prove media pacing, RTCP scheduling and conference mixing
      continue rather than terminating after the first timer tick.
- [ ] API documentation states the valid range and migration for any constructor that becomes
      fallible.

## Progress

- Filed from the media and conference cases in R-07 of
  `docs/reviews/2026-07-30T07-50-49+02-00-repository-review.md`.
- No existing story covers adversarial public timing values; happy-path media and conference stories
  use fixed positive intervals.

## Notes

- Endpoint WebSocket keepalive validation is T-27 because it belongs to endpoint construction and the
  bounded-transports epic.
