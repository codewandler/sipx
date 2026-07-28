---
id: M-7
title: Implement RFC 4733 DTMF events
pillar: Media
status: done
priority: 1
design: docs/designs/media.md
epic: media
areas: [sipx-rtp, sipx-media, sipx-call]
note: gap left explicitly by M3
---

# Implement RFC 4733 DTMF events

## Goal
Carry DTMF as RFC 4733 named telephone events, so the `telephone-event` sipx already
advertises in every offer becomes a promise it keeps.

## Acceptance
- [x] The four-byte event payload encodes and decodes: event code, end bit, volume, duration.
- [x] Digits `0`–`9`, `*`, `#` and `A`–`D` map to event codes 0–15 and back.
- [x] A tone is sent as a run of packets sharing one timestamp, with the duration increasing
      and the marker bit on the first — a receiver uses the timestamp to know it is one tone,
      so a per-packet timestamp turns one keypress into many.
- [x] The end of a tone is signalled with the end bit, sent three times per RFC 4733 §2.5.1.3,
      and a receiver reports the digit exactly once however many copies arrive.
- [x] Events do not reach the audio path, and audio does not reach the event path.
- [x] Failing-first test: `a_dtmf_digit_survives_a_media_session`.

## Progress
- Done. `crates/sipx-rtp/src/dtmf.rs`, routed through `sipx-media` and negotiated in
  `sipx-call`.
- The payload type is read from the description's own `rtpmap`, not assumed. `telephone-event`
  is a *dynamic* type: 101 is what sipx offers, not what everyone uses, and assuming it would
  send keypresses on whatever number the far end put to another purpose.
- One bug the acceptance test caught: the three end retransmissions were each getting a fresh
  RTP timestamp, so one keypress arrived as three digits. The packets of a tone are now tagged
  with the keypress they belong to — the send loop cannot tell from an end packet alone whether
  more are coming, and the shared timestamp is the only thing marking them as one press.
- Audio and DTMF share one paced queue, because they share one clock and one sequence number
  space. A separate path would have to interleave them anyway and would get the numbering
  wrong the first time both were busy.
