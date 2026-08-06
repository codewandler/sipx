---
id: M-71
title: "Deliver negotiated DTMF receive events through scenario"
pillar: "Media"
status: in-progress
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

- [ ] The RTP/media spec pins receive state for start, continuation, duration growth, end-bit
      retransmission, duplicates, reordering, timeout and source/payload changes using RFC 4733
      byte vectors.
- [ ] Payload type is taken from negotiated SDP and tested with a non-101 dynamic value. Packets on
      an unnegotiated payload cannot become digits.
- [ ] Failing-first two-scenario process proof negotiates telephone events in both directions,
      sends `1234`, observes sender completion, and reproduces the current remote `call.dtmf` wait
      expiry.
- [ ] The corrected receiver emits one ordered typed event per digit with the negotiated digit and
      duration facts; continuation and repeated end packets cannot duplicate events.
- [ ] The existing bounded RTP ingress and call-event queues remain the only handoff path. A stalled
      scenario consumer cannot block RTP/RTCP work or grow an unbounded digit buffer.
- [ ] Call replacement, hold/resume and teardown reset receive state deliberately; events from a
      prior stream or call cannot leak into the next correlation scope.
- [ ] Sender behavior, interrupt-on-digit playback, hold/resume controls, malformed-packet tests and
      the complete repository gate are green.

## Review evidence

Finding 10 negotiated `telephone-event/8000`, completed `send_dtmf`, then timed out waiting for the
remote typed event in the same harness that successfully observed hold and resume.

## Progress

- Receiver state and the process boundary are being specified before the implementation. The
  failing process proof will retain the sender completion and remote wait-expiry output that
  distinguishes delivery failure from negotiation or send failure.
- Board and compliance regeneration, the complete gate and final status are deferred to the
  requested push boundary.
