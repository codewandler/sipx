---
id: M-7
title: Implement RFC 4733 DTMF events
pillar: Media
status: backlog
priority:
design: docs/designs/media.md
epic: media
areas: [sipx-rtp, sipx-media, sipx-call]
note: gap left explicitly by M3
---

# Implement RFC 4733 DTMF events

## Goal
SDP already negotiates `telephone-event` and echoes its `fmtp`, so the far end believes sipx
accepts DTMF. Nothing encodes or decodes the events, which makes that advertisement a lie.

## Acceptance
- [ ] To be detailed when picked up.

## Progress
- Not started. Recorded as an explicit gap when M3 closed rather than left implied.
