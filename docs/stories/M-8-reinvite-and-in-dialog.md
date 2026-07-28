---
id: M-8
title: Handle re-INVITE and in-dialog requests
pillar: Media
status: ready
priority: 2
design: docs/designs/media.md
epic: media
areas: [sipx-call]
note: gap left explicitly by M3
---

# Handle re-INVITE and in-dialog requests

## Goal
Let an established call be modified: a re-INVITE that renegotiates media, from either side.

## Acceptance
- [ ] A re-INVITE inside a dialog is recognised as such rather than treated as a new call.
- [ ] The offer/answer runs again and the media session moves to the newly negotiated
      address, port and codec without dropping the call.
- [ ] `a=sendonly`/`a=inactive` in a re-INVITE puts the call on hold and `a=sendrecv` takes it
      off again.
- [ ] A re-INVITE that cannot be answered is rejected with 488 and **the existing session
      continues** — a failed renegotiation must not tear down a working call.
- [ ] The `CSeq` of a re-INVITE is greater than the last one, and one arriving out of order is
      rejected with 500 rather than applied.
- [ ] Failing-first test: `a_reinvite_moves_the_media_without_dropping_the_call`.

## Progress
- **Half of this story is already done**, brought forward by a code review: `Call::handle`
  routes in-dialog requests to their dialog, answers a BYE and stops the media, and the 2xx is
  retransmitted until acknowledged. What remains is re-INVITE.
