---
id: P-16
title: "Follow remote hangup and interrupt calls cleanly"
pillar: "Phone"
status: in-progress
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

- [x] `docs/specs/diagnostic-phone.md` defines the confirmed-command state table before code: active
      inputs, winner when terminal inputs race, BYE request/response ordering, interrupt result and
      exit semantics, and the join barrier before terminal output.
- [x] A failing-first two-process test runs `answer --duration 10` against `dial --duration 2`,
      proves the dialer's BYE receives 200, and proves the answerer emits its terminal result and
      exits from remote hangup rather than waiting for its ten-second local duration.
- [x] The same confirmed-call input pump dequeues ACK promptly. An independent-peer proof observes
      no INVITE 2xx retransmission after the ACK has been received, while retransmission still
      occurs before ACK as RFC 3261 requires.
- [x] A bounded interrupt test delivers the platform interrupt to a confirmed dialer and separately
      to a confirmed answerer. Each sends BYE, waits for or finitely bounds its response, stops media,
      emits exactly one machine record and exits with the spec's deliberate status.
- [x] Simultaneous remote BYE and local stop is deterministic: at most one BYE is originated, the
      received BYE is answered when present, and terminal output cannot be duplicated.
- [x] Pending invitations retain CANCEL behavior rather than manufacturing a BYE before a dialog
      exists. Calls rejected or ended before confirmation keep their existing public classification.
- [x] After every terminal path, transport routes, dialog tasks, media workers and device workers
      owned by the command have reached zero before the result is emitted.
- [ ] Text and JSON results, help/reference documentation, focused process tests and the complete
      repository gate are green.

## Review evidence

The external review's finding 1 captured a clean BYE after two seconds while the answerer stayed
alive for its full ten-second duration; terminating the dialer likewise left the peer alive and
produced no terminal machine record.

## Progress

- The two-process duration proof failed first because the answerer did not emit a terminal result
  within its remote-hangup bound; it now exits in about two seconds with `ended_by=remote`, while
  the dialer observes the BYE response.
- Confirmed dial and answer commands now share one inbound-dialog pump. It prioritizes queued
  remote BYE over interrupt and local completion, continues answering crossed requests during
  local teardown, and joins cancellation-aware media/device work before output.
- Exact process proofs cover ACK retransmission stopping, both confirmed interrupt roles, pending
  INVITE cancellation, and the remote/local teardown race. The complete `sipx-call` and `sipx-cli`
  all-feature package suites, affected clippy targets, docs/reference checks, provenance, and the
  fixed-sleep classifier pass.
- The generated board and complete repository gate remain deferred until push by user request, so
  the final acceptance item and story status remain open.
