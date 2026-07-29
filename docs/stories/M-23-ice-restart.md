---
id: M-23
title: Recognise and act on an ICE restart
pillar: Media
status: backlog
priority: 
design: docs/designs/media.md
epic: ice
areas: [sipx-media, sipx-call]
note: ice · RFC 8839 §4.4.1.1.1 · after M-22 · a restart that goes silent is worse than none
---

# Recognise and act on an ICE restart

## Goal
Survive a mid-call address change: a re-offer whose `ice-ufrag` and `ice-pwd` both changed
starts a new ICE session while the old pair keeps carrying audio.

## Acceptance
- [ ] **Both** `ice-ufrag` and `ice-pwd` changed is a restart (RFC 8839 §4.4.1.1.1); one alone is not,
      and the same value moving between session and media level is explicitly not.
- [ ] A restart regenerates the tiebreaker, re-gathers, rebuilds the checklists, and may redetermine
      the role.
- [ ] Media keeps flowing on the previously selected pair until the new session selects one. A
      restart that goes silent is worse than no restart.
- [ ] `c=0.0.0.0` is not used for hold; hold stays `a=inactive`/`a=sendonly` (RFC 3264).
- [ ] Failing-first test: `a_reoffer_that_changes_both_ufrag_and_pwd_restarts_ice_without_dropping_audio`.

## Progress
- Not started. Cut from `M-16`'s proposed split; the Acceptance above is that proposal verbatim.

## Notes
- The spec is [`docs/specs/ice.md`](../specs/ice.md), written by `M-16` before any code. Read the
  sections its Acceptance names rather than re-deriving them from the RFCs.
- `M-16` is the tracker for this epic and stays open until every child is done.
