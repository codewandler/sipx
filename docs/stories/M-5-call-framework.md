---
id: M-5
title: Implement dialogs and the call framework
pillar: Media
status: done
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
- [x] Dialog state per RFC 3261 §12, including route sets and the 2xx-ACK asymmetry.
- [x] `answer` and `dial` establish a call with SDP offer/answer in the INVITE exchange.
- [x] BYE ends a call from either side and tears the media down.
- [x] Failing-first test: two sipx endpoints establish a call, one plays a WAV, the other
      records it, and the recording matches the source.

## Progress
- Done. `crates/sipx-call/`: dialogs, `dial`, `answer`, `hang_up`.
- The dialog tests pin the three things that go wrong: the tags swap by role, the two sides
  number their requests independently, and the caller reverses the route set while the callee
  does not.
- The end-to-end test plays a WAV through a real call and asserts the recording is bit-exact
  after G.711 — a stronger claim than "close enough", and one a dropped or reordered packet
  would break. It also asserts the recording is loud enough to be the tone, so a test that
  recorded silence of the right length could not pass.
- **Not done: re-INVITE and in-dialog request handling.** A call can be established and ended;
  it cannot be modified, and an incoming BYE is not yet routed to its dialog. Filed as `M-8`.
