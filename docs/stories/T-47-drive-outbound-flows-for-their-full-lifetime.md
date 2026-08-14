---
id: T-47
title: Drive Outbound flows for their full registration lifetime
pillar: Transport
status: done
priority: 2
design: docs/specs/sip-transport.md
epic: sip-transport
areas: [sipx-ua, sipx-transport, sipx-cli]
predicate:
announcement:
note: v1 endpoint contract · lifetime owner implemented, focused-green and integrated-gate green
---

# Drive Outbound flows for their full registration lifetime

## Goal

Make a registered endpoint retain, refresh and retire its Outbound flows for as long as the
registration is live, so NAT reachability does not depend on an application rediscovering the
protocol pieces already implemented below it.

## Acceptance

- [x] The transport specification defines one bounded owner for the registration lease, each
      associated flow, its keepalive generation and its retry/backoff generation before runtime
      code changes.
- [x] A live UDP registration drives registration-associated, transaction-correlated STUN
      keepalives at the configured interval without inventing `MESSAGE-INTEGRITY` for RFC 5626
      §8's attribute-free usage, and treats a changed reflexive address as a failed flow; a
      connection-oriented registration drives the specified CRLF keepalive instead.
- [x] More than one configured outbound proxy is registered independently; one failed flow does not
      cancel, refresh or renumber its peers, and the configured maximum bounds all retained state.
- [x] Cancellation stops refresh and keepalive work, closes owned flows and joins every task before
      returning. Tests wait for an observed barrier rather than sleeping.
- [x] `sipx register` uses the same owner for a bounded command run and reports the selected flow,
      keepalive kind, failure and final cleanup without exposing credentials.
- [x] Failing-first tests prove `a_registered_udp_flow_is_kept_alive_until_the_lease_is_cancelled`
      and `one_failed_outbound_flow_does_not_end_its_peer`.
- [x] RFC 5626's registry note and the supported/experimental API declarations are narrowed to the
      behavior a caller can now reach; focused feature-off, tests, clippy, docs and the complete
      repository gate are green.

## Progress

- Filed for the v1 endpoint-library content contract. `sipx-ua` already implements
  `UserAgent::keepalive_after`, `Flows` and `Attempt`, but its crate contract records that no caller
  holds them through a live registration.
- 2026-08-14: the lifetime-owner contract was specified before runtime work. Focused implementation
  and acceptance checks are in progress; the complete repository gate remains coordinator-owned.
- 2026-08-14: the owner, one-flow CLI adoption, peer-isolation and cancellation barriers are green.
  Feature-off compilation, focused tests/clippy/rustdoc and affected repository checkers pass. The
  final acceptance row stays open solely for the coordinator's complete gate and generated board.
- 2026-08-14: the coordinator's integrated 55-step repository gate and generated checks passed.

## Notes

- This is the endpoint/UAC half only. Registrar flow tokens and proxy push buckets remain outside
  sipx's declared roles.
- Preserve the existing single-flow command behavior as the one-entry case of the owner rather than
  a second registration implementation.
