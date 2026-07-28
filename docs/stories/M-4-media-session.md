---
id: M-4
title: Implement the media session
pillar: Media
status: backlog
priority:
design: docs/designs/media.md
epic: media
areas: [sipx-media]
note:
---

# Implement the media session

## Goal
Bind RTP sockets from a negotiated SDP and move audio between the network and the application.

## Acceptance
- [ ] Sockets are bound from the negotiated SDP, with RTCP on the odd port.
- [ ] Symmetric RTP: send to where packets arrive from, which is what works through a NAT.
- [ ] Playback from a source and recording to a sink, paced by the packet clock.
- [ ] A media session ends cleanly when the call does, with no leaked sockets or tasks.
- [ ] Failing-first test: `audio_played_into_a_session_arrives_at_the_far_end`.

## Progress
- Not started.
