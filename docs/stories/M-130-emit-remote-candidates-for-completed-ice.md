---
id: M-130
title: Emit remote candidates for a completed ICE stream
pillar: Media
status: done
priority: 3
design: docs/specs/ice.md
epic: ice
areas: [sipx-sdp, sipx-media, sipx-call]
predicate:
announcement:
note: v1 endpoint contract · implemented, focused-green and integrated-gate green
---

# Emit remote candidates for a completed ICE stream

## Goal

When the controlling ICE agent has completed nomination, put the selected remote transport
addresses in its next offer so a peer can distinguish the active pair from all candidates retained
during negotiation.

## Acceptance

- [x] `docs/specs/ice.md` defines the controlling/completed precondition, component order, address
      source, generation lifetime and restart behavior before the offer builder changes.
- [x] A controlling agent in `Completed` emits one `remote-candidates` entry per selected component
      in ascending component order, using the peer address of the nominated pair.
- [x] A controlled, checking, failed, non-ICE or restarted stream emits none; a restart cannot leak
      the previous generation's selected addresses.
- [x] The attribute is carried by every subsequent call offer that already restates ICE, including
      hold, resume, codec change and session refresh, without changing SDP defaults or nomination.
- [x] The failing-first vector
      `holding_an_ice_call_re_signals_ice_and_does_not_restart_it` exercises the public call path
      rather than only serializing a hand-built attribute.
- [x] RFC 8839 evidence and notes, focused media/call tests, clippy and docs are synchronized and
      green.
- [x] The complete repository gate is green.

## Progress

- In progress for the v1 endpoint-library content contract. The parser and typed
  `RemoteCandidate` already exist; the RFC registry records generation as the missing behavior.
- 2026-08-14: the public hold/re-offer assertion failed first against the base tree with zero
  `remote-candidates`, then passed after the running agent became the sole source of the selected
  addresses. `cargo test -p sipx-call --test ice_call`, the focused media agent tests, and
  no-dependency all-feature clippy for `sipx-media` and `sipx-call` pass. The wave gate remains.
- 2026-08-14: the coordinator's integrated 55-step repository gate passed.

## Notes

- This story does not implement trickle ICE or change nomination. It reports the result of the
  existing regular-nomination state machine.
