---
id: M-24
title: Gather a relayed candidate from a configured relay
pillar: Media
status: done
priority: 32
design: docs/designs/media.md
epic: ice
areas: [sipx-media]
note: ice · RFC 8656 · after M-22 · the third RFC that made M-16 impossible as one story
---

# Gather a relayed candidate from a configured relay

## Goal
The last resort ICE keeps for when neither host nor reflexive candidates reach: allocate on
a configured TURN relay and offer the relayed candidate. Operating the relay server stays out of
scope.

## Acceptance
- [x] RFC 8656 Allocate, Refresh, CreatePermission and Send/Data against a configured relay with
      long-term credentials. **This is a third RFC, and it is why `M-16` could not be one story.**
- [x] The relayed candidate's type preference is 0, and its `raddr`/`rport` are the mapped address
      from the Allocate response (RFC 8839 §5.1).
- [x] Allocations kept alive until ICE completes (§5.1.1.4).
- [x] A relay that is unreachable or refuses degrades to the other candidate types rather than
      failing the call.
- [x] Failing-first test: `a_relayed_candidate_is_offered_when_a_relay_is_configured`.
- [x] `./scripts/gate.py` green.

## Progress
- 2026-08-14: implementation started. The existing gathering path already prices and serialises
  relayed candidates, but owns no TURN transaction, allocation, permission, or data path. The work
  therefore extends the bound-media-socket driver rather than creating a competing socket owner.
- 2026-08-14: implementation complete, coordinator gate pending. The bound socket now performs
  long-term Allocate, retains and refreshes allocations, creates and refreshes permissions, and
  wraps/unwraps Send/Data indications. Runtime requests have a finite retransmission ladder and
  correlate authenticated responses by base, transaction and operation; relayed checks wait in a
  bounded queue until permission success. `A-41` remains the owner of the high-level call-policy
  seam that supplies relay credentials.
- 2026-08-14: RFC 8489 authentication hardening complete. Challenges negotiate and echo the
  server's bounded PASSWORD-ALGORITHMS list in server preference order, select MD5 only for the
  permitted legacy form or SHA-256 otherwise, enforce OpaqueString credentials, honour USERHASH,
  verify the negotiated integrity form, and reject nonce-feature downgrade and stale-realm changes.
- 2026-08-14: the coordinator's integrated 55-step repository gate passed.

## Notes
- The spec is [`docs/specs/ice.md`](../specs/ice.md), written by `M-16` before any code. Read the
  sections its Acceptance names rather than re-deriving them from the RFCs.
- `M-16` is the tracker for this epic and stays open until every child is done.
