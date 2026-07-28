---
id: M-8
title: Handle re-INVITE and in-dialog requests
pillar: Media
status: backlog
priority:
design: docs/designs/media.md
epic: media
areas: [sipx-rtp, sipx-media, sipx-call]
note: gap left explicitly by M3
---

# Handle re-INVITE and in-dialog requests

## Goal
A call can be established and ended but not modified, and an incoming BYE is not routed to its
dialog — so the far end hanging up is not yet noticed.

## Acceptance
- [ ] To be detailed when picked up.

## Progress
- **In-dialog request handling is done**, brought forward by a code review: `Call::handle`
  routes an in-dialog request to its dialog, answers a BYE and stops the media. Without it an
  incoming BYE reached nothing and the local session kept sending RTP into a call the far end
  had torn down.
- Also done, from the same review: the 2xx is now retransmitted on the T1 backoff until the ACK
  arrives. The transaction layer absorbs retransmitted *requests* but does not resend the
  response — that is the transaction user's job, and over UDP one lost 200 meant the caller
  gave up while this side held an established call.
- **Still to do: re-INVITE.** A call cannot yet be modified once established.
