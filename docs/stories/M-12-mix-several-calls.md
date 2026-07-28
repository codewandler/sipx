---
id: M-12
title: Mix several calls
pillar: Media
status: ready
priority: 10
design: docs/designs/media.md
epic: depth
areas: [sipx-media]
note:
---

# Mix several calls

## Goal
A conference: every party hears every other party but not themselves.

## Acceptance
- [ ] N-1 mixing — each participant's own audio is excluded from their mix, or they hear
      themselves delayed, which is the single most disorienting artefact in conferencing.
- [ ] Mixing saturates rather than wraps; wrapping turns a loud moment into a loud click.
- [ ] Participants join and leave without disturbing the others.
- [ ] Failing-first test: `no_participant_hears_their_own_audio`.

## Progress
- Not started.
