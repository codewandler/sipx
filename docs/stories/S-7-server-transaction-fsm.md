---
id: S-7
title: Implement the server transaction state machines
pillar: Signalling
status: done
priority:
design: docs/designs/sip-core.md
epic: sip-core
areas: [sipx-sip]
note:
---

# Implement the server transaction state machines

## Goal
Implement the INVITE and non-INVITE server transactions (RFC 3261 §17.2), including the
retransmission absorption that keeps duplicate requests from reaching the application.

## Acceptance
- [x] Both FSMs specified as state tables in `docs/specs/sip-transaction.md` and tested row
      by row, as in `S-6`.
- [x] A retransmitted request in `Proceeding` or `Completed` re-sends the last response and
      is **not** delivered to the application a second time.
- [x] The 100 Trying rule is honoured: sent automatically if the application has not
      responded within 200 ms (§17.2.1).
- [x] Timers G, H and I for INVITE, and J for non-INVITE, behave correctly on both reliable
      and unreliable transports.
- [x] ACK for a non-2xx response is absorbed by the transaction; ACK for 2xx is passed up as
      a new transaction-less request (§17.2.1, §13.3.1.4).
- [x] Failing-first test: `server_tx_absorbs_request_retransmission_without_second_delivery`.

## Progress
- Done. `crates/sipx-sip/src/transaction/server.rs`, from §4.3 and §4.4 of the spec.
- The tests found a real bug: the 100-Trying timer fired and sent a 100 even after the TU had
  answered with a 180. The transaction emits a `ClearTimer` when the TU responds, but a state
  machine that depends on its driver having honoured a cancellation is one race away from
  sending 100 Trying after 180 Ringing. It now checks whether the TU has answered.
- The tests also surfaced an API gap: the builders could only append headers, so a test that
  tagged a `To` produced a message with two of them. Added `set_header`, which replaces —
  appending is right for `Via` and `Route`, wrong for `To` and `CSeq`.

## Notes
- The 2xx-ACK asymmetry is shared with `S-6`; test both sides against the same scenario.
