---
id: M-71
title: "Deliver negotiated DTMF receive events through scenario"
pillar: Media
status: done
epic: media-interoperability
areas: [sipx-rtp, sipx-call, sipx-cli]
design: docs/designs/media-interoperability.md
note: "external review finding 10 · digits send successfully but no typed receive event arrives"
---

# Deliver negotiated DTMF receive events through scenario

## Goal

Carry negotiated RFC 4733 telephone events from RTP ingress through the call event stream to the
remote scenario actor, so a successfully sent digit is observable exactly once at the public
automation boundary.

## Acceptance

- [x] The RTP/media spec pins receive state for start, continuation, duration growth, end-bit
      retransmission, duplicates, reordering, timeout and source/payload changes using RFC 4733
      byte vectors.
- [x] Payload type is taken from negotiated SDP and tested with a non-101 dynamic value. Packets on
      an unnegotiated payload cannot become digits.
- [x] Failing-first two-scenario process proof negotiates telephone events in both directions,
      sends `1234`, observes sender completion, and reproduces the current remote `call.dtmf` wait
      expiry.
- [x] The corrected receiver emits one ordered typed event per digit with the negotiated digit and
      duration facts; continuation and repeated end packets cannot duplicate events.
- [x] The existing bounded RTP ingress and call-event queues remain the only handoff path. A stalled
      scenario consumer cannot block RTP/RTCP work or grow an unbounded digit buffer.
- [x] Call replacement, hold/resume and teardown reset receive state deliberately; events from a
      prior stream or call cannot leak into the next correlation scope.
- [x] Sender behavior, interrupt-on-digit playback, hold/resume controls, malformed-packet tests and
      the complete repository gate are green.

## Review evidence

Finding 10 negotiated `telephone-event/8000`, completed `send_dtmf`, then timed out waiting for the
remote typed event in the same harness that successfully observed hold and resume.

## Progress

- Failing-first output retained the sender's completed `send_dtmf`, emitted no remote `call.dtmf`,
  expired the remote waits and ended both scenario streams as failed. With the shared call-owned
  media pump, the same two processes emit exactly `1234` with 100 ms wire-derived durations.
- PT 96 byte vectors cover start, continuation, end, reordering, repeated final reports and a fired
  silence input. Media tests prove PT 101 cannot create a digit when 96 was negotiated, a full
  32-place digit queue drops and counts the 33rd event, and reconfiguration cannot leak an
  incomplete old-generation digit.
- Focused RTP, media, call-event, interrupt-on-digit, hold/resume and scenario process tests pass;
  strict all-target/all-feature clippy passes for the four changed packages. Fixed-sleep, docs-link
  and provenance checks pass.
- Board/compliance regeneration, the complete gate, CHANGELOG and final status remain deferred to
  the requested push boundary.
