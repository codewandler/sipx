---
id: T-27
title: Reject invalid endpoint runtime configuration before binding
pillar: Signalling
status: in-progress
priority: 5
design: docs/designs/bounded-transports.md
epic: bounded-transports
areas: [sipx-transport]
predicate: 4
note: R-06 and the WebSocket part of R-07 in the 2026-07-30 repository review — zero public capacities panic and zero keepalive terminates the task
---

# Reject invalid endpoint runtime configuration before binding

## Goal

Turn unusable public endpoint capacities and WebSocket intervals into typed configuration errors
before the endpoint binds sockets or starts workers.

## Acceptance

- [ ] Specify the valid ranges for endpoint queue capacities and WebSocket keepalive intervals in
      `docs/specs/sip-transport.md`.
- [ ] A request-channel capacity of zero returns a typed error instead of reaching the runtime channel
      constructor and panicking.
- [ ] A zero WebSocket keepalive interval returns a typed error instead of starting a task whose timer
      terminates immediately.
- [ ] Validation occurs before any listener binds or background task starts, and all public endpoint
      construction paths use the same validator.
- [ ] Failing-first tests use the public API with each zero value, assert the typed error and prove the
      address remains bindable afterward. The library test must not rely on catching a panic as the
      final behavior.
- [ ] Valid minimum values retain their documented behavior for TCP, TLS, WebSocket and secure
      WebSocket endpoint configurations.

## Progress

- Filed from R-06 and R-07 in
  `docs/reviews/2026-07-30T07-50-49+02-00-repository-review.md`.
- No existing story tracks invalid public endpoint configuration; the original transport stories
  test ordinary non-zero defaults.

## Notes

- Media and conference timing are tracked separately in M-36 because their constructors and worker
  ownership differ, though both stories follow the same validate-before-spawn rule.
