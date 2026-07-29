---
id: M-24
title: Gather a relayed candidate from a configured relay
pillar: Media
status: backlog
priority: 
design: docs/designs/media.md
epic: ice
areas: [sipx-media]
note: ice · RFC 8656 · after M-22 · the third RFC that made M-16 impossible as one story
---

# Gather a relayed candidate from a configured relay

## Goal
The last resort ICE keeps for when neither host nor reflexive candidates reach: allocate on
a configured TURN relay and offer the relayed candidate. Running a relay stays out of scope.

## Acceptance
- [ ] RFC 8656 Allocate, Refresh, CreatePermission and Send/Data against a configured relay with
      long-term credentials. **This is a third RFC, and it is why `M-16` could not be one story.**
- [ ] The relayed candidate's type preference is 0, and its `raddr`/`rport` are the mapped address
      from the Allocate response (RFC 8839 §5.1).
- [ ] Allocations kept alive until ICE completes (§5.1.1.4).
- [ ] A relay that is unreachable or refuses degrades to the other candidate types rather than
      failing the call.
- [ ] Failing-first test: `a_relayed_candidate_is_offered_when_a_relay_is_configured`.

## Progress
- Not started. Cut from `M-16`'s proposed split; the Acceptance above is that proposal verbatim.

## Notes
- The spec is [`docs/specs/ice.md`](../specs/ice.md), written by `M-16` before any code. Read the
  sections its Acceptance names rather than re-deriving them from the RFCs.
- `M-16` is the tracker for this epic and stays open until every child is done.
