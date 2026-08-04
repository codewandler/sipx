---
id: P-15
title: Run a bounded call-load responder
pillar: Phone
status: backlog
priority: 11
design: docs/designs/comparative-load.md
epic: comparative-load
areas: [sipx-cli, load, m14]
predicate:
announcement:
note: after X-98 and X-75 · machine-ready signalling UAS with finite admission and cleanup
---

# Run a bounded call-load responder

## Goal

Add the missing machine-driven answering side for finite load and interoperability work without
overloading the interactive `sipx answer` contract.

## Acceptance

- [ ] A public command emits a machine-readable readiness address before accepting traffic and
      requires a finite call count or duration, maximum active calls and cleanup deadline.
- [ ] Seeded policy controls provisional response, answer/reject distribution and bounded dialog
      duration; signalling-only is the default and generated media is a separate explicit mode.
- [ ] Concurrent INVITEs are admitted within the configured bound, and ACK, CANCEL and BYE outcomes
      are validated rather than counted as arbitrary packets.
- [ ] Stable JSON reports INVITEs, provisional/final statuses, established/completed/cancelled/failed
      calls, active high-water, setup/teardown distributions and endpoint leftovers after cleanup.
- [ ] Interrupt and internal error stop admission, terminate owned dialogs, await cleanup and leave no
      child, task, transaction or dialog behind.
- [ ] UDP is the required baseline. TCP, TLS, WS and WSS are separate future scenarios so connection
      reuse and handshake cost cannot contaminate the SIP transaction baseline.
- [ ] A bounded failing-first integration test drives readiness through drain and the gate is green.

## Progress

- Backlog. Depends on X-98's result contract and X-75's supported test surface.
