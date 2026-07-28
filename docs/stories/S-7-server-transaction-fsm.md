---
id: S-7
title: Implement the server transaction state machines
pillar: Signalling
status: backlog
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
- [ ] Both FSMs specified as state tables in `docs/specs/sip-transaction.md` and tested row
      by row, as in `S-6`.
- [ ] A retransmitted request in `Proceeding` or `Completed` re-sends the last response and
      is **not** delivered to the application a second time.
- [ ] The 100 Trying rule is honoured: sent automatically if the application has not
      responded within 200 ms (§17.2.1).
- [ ] Timers G, H and I for INVITE, and J for non-INVITE, behave correctly on both reliable
      and unreliable transports.
- [ ] ACK for a non-2xx response is absorbed by the transaction; ACK for 2xx is passed up as
      a new transaction-less request (§17.2.1, §13.3.1.4).
- [ ] Failing-first test: `server_tx_absorbs_request_retransmission_without_second_delivery`.

## Progress
- Not started.

## Notes
- The 2xx-ACK asymmetry is shared with `S-6`; test both sides against the same scenario.
