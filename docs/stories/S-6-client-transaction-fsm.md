---
id: S-6
title: Implement the client transaction state machines
pillar: Signalling
status: backlog
priority:
design: docs/designs/sip-core.md
epic: sip-core
areas: [sipx-sip]
note:
---

# Implement the client transaction state machines

## Goal
Implement the INVITE and non-INVITE client transactions (RFC 3261 §17.1) as sans-IO state
machines whose every transition is reachable in a unit test with no socket and no clock.

## Acceptance
- [ ] `docs/specs/sip-transaction.md` records both FSMs as literal state × input →
      (state, actions) tables, with timers A–D and E–K and their reliable/unreliable
      transport variants.
- [ ] The implementation is generated from — and tested against — those tables: a test
      walks every row and asserts the resulting state and emitted outputs.
- [ ] Timers are emitted as `SetTimer`/`ClearTimer` outputs and fired as inputs; the crate
      reads no clock.
- [ ] Retransmission intervals follow T1 backoff capped at T2, and the values are
      configurable per RFC 3261 §17.1.1.2 without a global.
- [ ] The INVITE transaction generates ACK for a non-2xx final response itself, and does not
      for 2xx (§17.1.1.3) — a distinction that is a perennial source of bugs.
- [ ] Failing-first test: `invite_client_tx_acks_non_2xx_only`.

## Progress
- Not started.

## Notes
- Transitions are only correct if the tables are: review the spec tables against the RFC
  figures before implementing.
