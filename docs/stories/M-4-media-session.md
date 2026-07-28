---
id: M-4
title: Implement the media session
pillar: Media
status: done
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
- [x] Sockets are bound from the negotiated SDP, with RTCP on the odd port.
- [x] Symmetric RTP: send to where packets arrive from, which is what works through a NAT.
- [x] Playback from a source and recording to a sink, paced by the packet clock.
- [x] A media session ends cleanly when the call does, with no leaked sockets or tasks.
- [x] Failing-first test: `audio_played_into_a_session_arrives_at_the_far_end`.

## Progress
- Done. `crates/sipx-media/`.
- Symmetric RTP latches the observed source — but only after the packet has *parsed*. Latching
  on any datagram would let anyone who can guess the port redirect a call's media with a single
  byte, which is a hijack rather than NAT traversal.
- `MediaPort` exists because of an ordering constraint: an SDP offer must name the port audio
  will arrive on, but the codec and remote address are unknown until the answer. Binding twice
  instead fails with `AddrInUse`, which is how the type came to exist.
- The stop signal is a flag *and* a notify. `notify_waiters` only wakes tasks already parked,
  so a loop blocked on its channel when a call is hung up would keep sending audio into a
  torn-down call.
