---
id: M-5
title: Implement dialogs and the call framework
pillar: Media
status: backlog
priority:
design: docs/designs/media.md
epic: media
areas: [sipx-call]
note:
---

# Implement dialogs and the call framework

## Goal
Turn transactions and media into calls: dialogs, answering, dialling, and audio in both
directions.

## Acceptance
- [ ] Dialog state per RFC 3261 §12, including route sets and the 2xx-ACK asymmetry.
- [ ] `answer` and `dial` establish a call with SDP offer/answer in the INVITE exchange.
- [ ] BYE ends a call from either side and tears the media down.
- [ ] Failing-first test: two sipx endpoints establish a call, one plays a WAV, the other
      records it, and the recording matches the source.

## Progress
- Not started.
