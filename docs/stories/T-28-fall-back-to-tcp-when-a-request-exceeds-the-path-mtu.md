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

- [x] A request whose size exceeds the configured MTU threshold on an unreliable transport is sent
      over TCP to the same destination rather than refused, per RFC 3261 §18.1.1. A failing-first
      test builds an oversized INVITE — a large SDP is the realistic case — and asserts it arrives
      over TCP.
- [x] The existing refusal at `crates/sipx-transport/src/endpoint.rs` is replaced by the fallback on
      the paths where the RFC requires it, and **retained where it does not** — the refusal is
      documented with the RFC reason today, so this story changes a behaviour that was deliberate
      and must say why in the same place.
- [x] The threshold follows §18.1.1: 1300 bytes, or the path MTU less 200 where known, and is
      derived rather than hardcoded twice.
- [x] If TCP is unavailable to that destination the request is refused with a typed error naming
      the reason, never truncated and never sent regardless.
- [x] The fallback is visible — a counter or log — since a silent transport switch is a debugging
      trap.
- [x] `docs/rfc/registry.toml` RFC 3261 row and `docs/specs/sip-transport.md` updated in the same
      commit; `rfc-report.py --check` green.
- [ ] `./scripts/gate.py` green.

## Progress
- 2026-08-05: selected as the first story in the post-beta.7 transport operations wave. The RFC
  threshold and failing-first oversized-INVITE transport proof precede the endpoint change.
- 2026-08-05: the failing-first UDP integration test did not compile against the old API because
  the path-MTU input, fallback counter and typed TCP-failure variant did not yet exist. The
  implementation now derives the threshold once, selects TCP before transaction creation, rebuilds
  a transport-owned top `Via`, keeps oversized responses on their selected transport, and retains
  the final UDP refusal as a defensive invariant.
- 2026-08-05: `cargo test -p sipx-transport --all-features` passed the complete crate suite,
  including 175 library tests and every transport integration target. Targeted all-feature clippy,
  RFC-report checking, provenance, formatting and diff checks also passed. A later filtered test
  retry after a no-behaviour helper extraction could not link because the shared filesystem had 25
  MiB free; that retry is an infrastructure non-result, and only this worktree's recoverable build
  artifacts were cleaned. Per wave instruction, the full repository gate was not run, so the story
  remains in progress and its gate acceptance item remains open.
- 2026-08-05: the first integrated main CI run exposed that an asynchronous refused TCP connection
  delivered the typed fallback error but did not increment `unsent`. The connection pool now reports
  a failed outbound dial separately from an established connection closing, preserving the concrete
  I/O cause and counting only bytes that provably never reached a socket. The focused call counter
  regression and all three oversized-request/response transport tests pass. Per wave instruction,
  the local full gate remains deliberately unrun.

## Notes
- Two independent, still-open requests against a comparable stack, and a straightforward conformance
  gap for sipx: `endpoint.rs:58-64` currently documents refusing an oversized datagram *citing this
  same RFC section*, which mandates the switch rather than the refusal.
- Interacts with connection reuse (`docs/specs/sip-transport.md` §8): the fallback opens a TCP
  connection, and whether it is poolable follows the existing key rules rather than new ones.
