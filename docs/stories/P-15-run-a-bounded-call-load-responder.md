---
id: P-15
title: Run a bounded call-load responder
pillar: Phone
status: in-progress
priority: 13
design: docs/designs/comparative-load.md
epic: comparative-load
areas: [sipx-cli, load, m13, parity-wave-1]
predicate:
announcement:
note: late M13 after X-98 and X-75 · machine-ready signalling UAS with finite admission and cleanup
---

# Run a bounded call-load responder

## Goal

Add the missing machine-driven answering side for finite load and interoperability work without
overloading the interactive `sipx answer` contract.

## Acceptance

- [x] A public command emits a machine-readable readiness address before accepting traffic and
      requires a finite call count or duration, maximum active calls and cleanup deadline.
- [x] Seeded policy controls provisional response, answer/reject distribution and bounded dialog
      duration; signalling-only is the default and generated media is a separate explicit mode.
- [x] Concurrent INVITEs are admitted within the configured bound, and ACK, CANCEL and BYE outcomes
      are validated rather than counted as arbitrary packets.
- [x] Stable JSON reports INVITEs, provisional/final statuses, established/completed/cancelled/failed
      calls, active high-water, setup/teardown distributions and endpoint leftovers after cleanup.
- [x] Interrupt and internal error stop admission, terminate owned dialogs, await cleanup and leave no
      child, task, transaction or dialog behind.
- [x] UDP is the required baseline. TCP, TLS, WS and WSS are separate future scenarios so connection
      reuse and handshake cost cannot contaminate the SIP transaction baseline.
- [ ] A bounded failing-first integration test drives readiness through drain and the gate is green.

## Progress

- The public `load-responder` now has the specified finite lifecycle, seeded policy, SDP-free
  confirmed-dialog primitive, separately explicit generated-media mode, versioned readiness and
  summary records, and post-drain zero-state accounting. Failing-first evidence began with the
  absent `SignallingEvent`/answering APIs. Focused all-feature tests cover the exact wire flow and a
  live concurrency-one refusal while an admitted dialog remains active. Strict Clippy, CLI-contract,
  RFC-evidence, fixed-wait and provenance checks pass. The final acceptance item remains open until
  the integration branch runs the full project gate once for the whole M13 wave.
