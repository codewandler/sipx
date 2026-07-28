---
id: M-12
title: Mix several calls
pillar: Media
status: done
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
- [x] N-1 mixing — each participant's own audio is excluded from their mix, or they hear
      themselves delayed, which is the single most disorienting artefact in conferencing.
- [x] Mixing saturates rather than wraps; wrapping turns a loud moment into a loud click.
- [x] Participants join and leave without disturbing the others.
- [x] Failing-first test: `no_participant_hears_their_own_audio`.

## Progress
- Done. `sipx_audio::mix` for the arithmetic — testable without a socket in sight — and
  `sipx_media::Conference` for the clock, the membership and the N-1 routing.
- **A conference is a clock, not a chain of forwards.** A bridge can forward each packet as it
  arrives because there is exactly one place to send it; a mixer has to decide *when* a frame is
  complete while waiting on N participants who will not arrive together. So one task ticks at
  the packet interval and a participant who has said nothing contributes silence — the
  alternative, waiting for everyone, makes the whole conference as late as its worst connection.
- Three parties in every test, because two is a bridge: with two, "everyone else" and "the other
  one" are the same set, and an implementation that simply echoed to the other party would pass.
- The tests record for a fixed span rather than until idle, and that is a property of the
  design rather than a workaround. A mixed stream is continuous: every participant gets a frame
  every 20 ms whether anyone is talking or not, so waiting for a gap waits forever. The first
  version of the test did exactly that and hung.
- Unlike a bridge, a conference cannot pass bytes through — mixing happens on samples, so every
  leg is decoded in and encoded out. Not an optimisation left for later: adding two µ-law codes
  is not adding two amplitudes.
