---
id: T-28
title: Fall back to TCP when a request exceeds the path MTU
pillar: Transport
status: in-progress
priority: 15
design: docs/designs/demand.md
epic: demand
areas: [sipx-transport]
predicate:
announcement:
note: RFC 3261 §18.1.1 · sipx currently refuses where the RFC says switch transport
---

# Fall back to TCP when a request exceeds the path MTU

## Goal

Send a request that does not fit a datagram over TCP instead of refusing it, as RFC 3261 §18.1.1
requires.

## Acceptance

- [ ] A request whose size exceeds the configured MTU threshold on an unreliable transport is sent
      over TCP to the same destination rather than refused, per RFC 3261 §18.1.1. A failing-first
      test builds an oversized INVITE — a large SDP is the realistic case — and asserts it arrives
      over TCP.
- [ ] The existing refusal at `crates/sipx-transport/src/endpoint.rs` is replaced by the fallback on
      the paths where the RFC requires it, and **retained where it does not** — the refusal is
      documented with the RFC reason today, so this story changes a behaviour that was deliberate
      and must say why in the same place.
- [ ] The threshold follows §18.1.1: 1300 bytes, or the path MTU less 200 where known, and is
      derived rather than hardcoded twice.
- [ ] If TCP is unavailable to that destination the request is refused with a typed error naming
      the reason, never truncated and never sent regardless.
- [ ] The fallback is visible — a counter or log — since a silent transport switch is a debugging
      trap.
- [ ] `docs/rfc/registry.toml` RFC 3261 row and `docs/specs/sip-transport.md` updated in the same
      commit; `rfc-report.py --check` green.
- [ ] `./scripts/gate.py` green.

## Progress
- 2026-08-05: selected as the first story in the post-beta.7 transport operations wave. The RFC
  threshold and failing-first oversized-INVITE transport proof precede the endpoint change.

## Notes
- Two independent, still-open requests against a comparable stack, and a straightforward conformance
  gap for sipx: `endpoint.rs:58-64` currently documents refusing an oversized datagram *citing this
  same RFC section*, which mandates the switch rather than the refusal.
- Interacts with connection reuse (`docs/specs/sip-transport.md` §8): the fallback opens a TCP
  connection, and whether it is poolable follows the existing key rules rather than new ones.
