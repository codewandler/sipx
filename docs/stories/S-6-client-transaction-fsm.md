---
id: S-6
title: Implement the client transaction state machines
pillar: Signalling
status: done
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
- [x] `docs/specs/sip-transaction.md` records both FSMs as literal state × input →
      (state, actions) tables, with timers A–D and E–K and their reliable/unreliable
      transport variants.
- [x] The implementation is generated from — and tested against — those tables: a test
      walks every row and asserts the resulting state and emitted outputs.
- [x] Timers are emitted as `SetTimer`/`ClearTimer` outputs and fired as inputs; the crate
      reads no clock.
- [x] Retransmission intervals follow T1 backoff capped at T2, and the values are
      configurable per RFC 3261 §17.1.1.2 without a global.
- [x] The INVITE transaction generates ACK for a non-2xx final response itself, and does not
      for 2xx (§17.1.1.3) — a distinction that is a perennial source of bugs.
- [x] Failing-first test: `invite_client_tx_acks_non_2xx_only`.

## Progress
- Done. `crates/sipx-sip/src/transaction/client.rs`, written from the state tables in
  `docs/specs/sip-transaction.md` §4.1 and §4.2.
- RFC 6026 adopted: a 2xx moves the machine to `Accepted` with Timer M rather than terminating
  it. Without that, the second 200 from a forking proxy matches no transaction. The consequence
  worth knowing is that the TU can be handed the same INVITE's 2xx more than once — that is a
  fork, not a bug, and there is a test for it.
- Timer A doubles without a ceiling; Timer E doubles but stops at T2. The difference is easy to
  miss and both are pinned by tests that walk four retransmissions.

## Notes
- Transitions are only correct if the tables are: review the spec tables against the RFC
  figures before implementing.
