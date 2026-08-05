---
id: S-38
title: Place and maintain event subscriptions
pillar: Signalling
status: in-progress
priority: 7
design: docs/designs/event-reachability.md
epic: event-reachability
areas: [sipx-ua, sipx-transport, m13, parity-wave-1]
predicate:
announcement:
note: after S-37 · reusable outbound SUBSCRIBE and NOTIFY tracking · S-24 is a consumer
---

# Place and maintain event subscriptions

## Goal

Expose the reusable endpoint path that issues SUBSCRIBE and maintains the resulting notification
dialog, without embedding any event package's application policy in the transport machinery.

## Acceptance

- [x] The public API establishes, refreshes and terminates a subscription through the state machine
      and byte vectors in [`docs/specs/event-client.md`](../specs/event-client.md), specifically
      `S37-V1` through `S37-V16`.
- [x] 401 and 407 challenges reuse endpoint credentials; refreshes use the granted expiry; initial
      and subsequent NOTIFY requests are ordered and surfaced with typed subscription state.
- [x] The dialog remote target, route set and CSeq rules are honored for every request, and a
      terminated or rejected subscription cannot be silently resurrected. Local CSeq exhaustion is
      typed and never wraps or emits a repeated request.
- [x] Every initial and target-refresh NOTIFY passes the configured trust policy and carries exactly
      one parseable Contact before it can select a dialog, change a remote target or deliver state.
- [x] Response Expires and 423 Min-Expires are fail-closed for initial, refresh and unsubscribe
      operations; an expiry-less initial NOTIFY retains a finite provisional bound.
- [x] Live subscriptions, pending notification delivery and refresh timers are bounded. Cancellation
      waits for owned work and a test observes zero residual transactions and timers.
- [x] A synthetic package proves the generic API; `S-24` consumes it for `reg` without copying the
      subscriber state machine.
- [ ] RFC registry evidence is updated with reachable Rust paths and `./scripts/gate.py` is green.

## Progress

- The generic sans-I/O client and endpoint driver implement all fourteen contract vectors. The live
  endpoint proof covers authentication, refresh, ordered delivery, unsubscribe, joined shutdown and
  zero residual transactions/timers; focused route-set and operation-serialization tests cover the
  dialog edge cases found during review.
- The synthetic package proves the package seam and `S-24` now consumes it for `reg`. Review also
  found that secure endpoint targets lost their certificate identity and WebSocket resource at the
  event boundary; V14 now preserves both through initial send, authentication and target refresh.
  Review additionally pinned the selected stream generation on both transaction boundaries, the
  route-set's first hop as the transport target, byte-exact dialog tags, bounded terminal-reason
  retry eligibility, and an async shutdown barrier that joins every driver-owned task. Re-review
  then completed route-hop scheme/transport/port/authority derivation and made the driver registry
  the atomic admission/shutdown boundary. The final full-gate item intentionally remains open for
  the integration branch.

## Required failing-first tests

The tests below cite the normative vectors rather than restating them:

- `authenticated_subscription_establishes_from_notify` — `S37-V1`.
- `notify_before_response_selects_one_dialog` — `S37-V2`.
- `notify_expiry_overrides_refresh_response` — `S37-V3`.
- `local_expiry_releases_everything` — `S37-V4`.
- `unsubscribe_waits_for_terminal_notify` — `S37-V5`.
- `stale_notify_is_refused_without_delivery` — `S37-V6`.
- `unsupported_event_is_489` — `S37-V7`.
- `shutdown_cancels_a_due_refresh_and_drains` — `S37-V8`.
- `expiryless_notify_retains_a_finite_provisional_bound` — `S37-V9`.
- `local_cseq_exhaustion_terminates_without_a_send` — `S37-V10`.
- `response_intervals_fail_closed_for_every_operation` — `S37-V11`.
- `notify_trust_and_contact_rejections_do_not_mutate` — `S37-V12`.
- `refresh_timer_n_preserves_only_the_authoritative_expiry` — `S37-V13`.
- `secure_target_identity_and_resource_survive_every_send` — `S37-V14`.
- `record_route_selects_transport_port_authority_and_generation` — `S37-V15`.
- `secure_datagram_route_is_a_typed_refusal_without_a_send` — `S37-V15`.
- `racing_shutdown_closes_admission_before_any_spawn` — `S37-V16`.
