---
id: M-8
title: Handle re-INVITE and in-dialog requests
pillar: Media
status: done
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
- [x] A re-INVITE inside a dialog is recognised as such rather than treated as a new call.
- [x] The offer/answer runs again and the media session moves to the newly negotiated
      address, port and codec without dropping the call.
- [x] `a=sendonly`/`a=inactive` in a re-INVITE puts the call on hold and `a=sendrecv` takes it
      off again.
- [x] A re-INVITE that cannot be answered is rejected with 488 and **the existing session
      continues** — a failed renegotiation must not tear down a working call.
- [x] The `CSeq` of a re-INVITE is greater than the last one, and one arriving out of order is
      rejected with 500 rather than applied.
- [x] Failing-first test: `a_reinvite_moves_the_media_without_dropping_the_call`.

## Progress
- Done. `Call::on_reinvite` and `Call::reinvite` in `crates/sipx-call/src/call.rs`.
- The rule that shapes it: a renegotiation that fails leaves the call running. A re-INVITE
  tries to change something that already works, so 488 and carry on is right — tearing the
  call down because the new offer was unusable would lose a call that was fine a moment ago.
- The media session is only rebuilt when the address or codec actually changed. Some peers
  send a re-INVITE every thirty seconds as a keep-alive, and restarting an unchanged session
  would drop packets each time for nothing.
- Hold is a direction rather than a separate state, so `sendonly`/`inactive` and the way back
  fall out of the same code path.
- **Earlier half**, brought forward by a code review: in-dialog routing, BYE handling and 2xx
  retransmission until acknowledged., brought forward by a code review: `Call::handle`
  routes in-dialog requests to their dialog, answers a BYE and stops the media, and the 2xx is
  retransmitted until acknowledged. What remains is re-INVITE.
