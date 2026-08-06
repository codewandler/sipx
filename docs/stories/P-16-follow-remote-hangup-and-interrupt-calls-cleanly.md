---
id: P-16
title: "Follow remote hangup and interrupt calls cleanly"
pillar: "Phone"
status: ready
priority: 1
epic: phone-lifecycle
areas: [sipx-cli, sipx-call]
design: docs/designs/phone-lifecycle.md
note: "external reviews findings 1–2/1,2,4 · confirmed calls do not drain ACK/BYE and interrupts emit no terminal record"
---

# Follow remote hangup and interrupt calls cleanly

## Goal

Make each live `dial` and `answer` process follow the confirmed dialog it owns. A remote BYE, local
duration, process interrupt or terminal transport failure must select one teardown cause, drive the
required SIP exchange, stop media, join owned work and emit one terminal result.

## Acceptance

- [ ] `docs/specs/diagnostic-phone.md` defines the confirmed-command state table before code: active
      inputs, winner when terminal inputs race, BYE request/response ordering, interrupt result and
      exit semantics, and the join barrier before terminal output.
- [ ] A failing-first two-process test runs `answer --duration 10` against `dial --duration 2`,
      proves the dialer's BYE receives 200, and proves the answerer emits its terminal result and
      exits from remote hangup rather than waiting for its ten-second local duration.
- [ ] The same confirmed-call input pump dequeues ACK promptly. An independent-peer proof observes
      no INVITE 2xx retransmission after the ACK has been received, while retransmission still
      occurs before ACK as RFC 3261 requires.
- [ ] A bounded interrupt test delivers the platform interrupt to a confirmed dialer and separately
      to a confirmed answerer. Each sends BYE, waits for or finitely bounds its response, stops media,
      emits exactly one machine record and exits with the spec's deliberate status.
- [ ] Simultaneous remote BYE and local stop is deterministic: at most one BYE is originated, the
      received BYE is answered when present, and terminal output cannot be duplicated.
- [ ] Pending invitations retain CANCEL behavior rather than manufacturing a BYE before a dialog
      exists. Calls rejected or ended before confirmation keep their existing public classification.
- [ ] After every terminal path, transport routes, dialog tasks, media workers and device workers
      owned by the command have reached zero before the result is emitted.
- [ ] Text and JSON results, help/reference documentation, focused process tests and the complete
      repository gate are green.

## Review evidence

The external review's finding 1 captured a clean BYE after two seconds while the answerer stayed
alive for its full ten-second duration; terminating the dialer likewise left the peer alive and
produced no terminal machine record.
