---
id: C-2
title: Carry media on an early dialog
pillar: Media
status: done
priority: 6
design: docs/designs/call.md
epic: call
areas: [sipx-call, sipx-media]
note: M9 · RFC 3960 gateway model · one live session crosses early and confirmed dialog state
---

# Carry media on an early dialog

## Goal
Let audio flow before the call is answered, in both directions: a UAS that sends an announcement or
a distinctive ringing tone, and a UAC that plays what it receives instead of a locally generated
tone.

## Acceptance
- [x] A UAS can put a session description in a reliable provisional response and start its media
      session on the early dialog. Today it cannot: `sipx-call` notes that "sipx never puts one in a
      provisional" and relies on that to keep RFC 3262 §3's rule about an unacknowledged provisional
      trivially satisfied. That rule now has to be honoured for real — no 2xx while a reliable
      provisional carrying a description is unacknowledged.
- [x] A UAC that answers such an offer in its PRACK (already built by `S-12`) starts receiving and
      rendering media on the early dialog rather than discarding it until the 2xx.
- [x] The application is told which it is hearing. RFC 3960 §3.2: a UAC "is supposed to generate
      ringing tones locally for its user as long as no early media is received from the UAS. If the
      UAS generates early media […] the UAC is supposed to play it rather than generate the ringing
      tone locally." sipx does not generate tones, so what it owes the application is the *signal* —
      early media has started — precise enough that the application can stop its own tone.
- [x] The early media session becomes the confirmed one on the 2xx without re-keying, re-binding or
      a gap in the stream. A second session created at answer time is a clip the user hears.
- [x] An early dialog that dies — 4xx/5xx/6xx, CANCEL, or a fork losing the race — tears its media
      session down, with no socket and no task left behind. `M-11`'s `Drop` discipline is the
      precedent.
- [x] Only the gateway model (RFC 3960 §3) is in scope, and the story says so: one offer/answer on
      the INVITE's own early dialog.
- [x] Failing-first test: `a_caller_receives_early_media_before_the_call_is_answered`.

## Progress
- Done. The state and ownership contract is `docs/specs/call-early-media.md`. A reliable
  provisional establishes the dialog and starts one negotiated `MediaSession`; `Dialing` exposes
  that session and reports `EarlyMediaStarted`, and confirmation moves the same session into
  `Call`. Early UPDATE reconfigures it without rebinding, while every failed or losing early-dialog
  path drops it. The named failing-first test proves audio arrives before the 2xx and the teardown
  test proves dropping the early dialog stops its worker.

## Notes
- RFC 3960 §3.1 is the case this story deliberately does not solve: with a forked INVITE a UAC can
  receive early media from several UASs at once and has to choose. sipx has no forking and its
  application sees each early dialog, so the choice is the application's — but the API must at least
  make it *possible* to have two early dialogs with media and pick one.
- The application server model (§4, with RFC 3959's `early-session` disposition) is out of scope. It
  needs a second offer/answer axis in SDP, and nothing has asked for it.
- Both directions matter for different reasons. The UAS direction is what carriers want (an
  announcement before answer, and therefore before billing); the UAC direction is what a user
  notices, because the alternative is silence where a network tone should be.
