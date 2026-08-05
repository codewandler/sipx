---
id: T-34
title: Cancel an outgoing INVITE transaction from a forwarding element
pillar: Signalling
status: ready
priority: 1
design: docs/designs/sip-transport.md
epic: sip-transport
areas: [sipx-transport, sipx-sip]
predicate:
announcement:
note: requested by sipx-clstr PX-12 — one public operation must cancel the exact outgoing INVITE branch
---

# Cancel an outgoing INVITE transaction from a forwarding element

## Goal

Give a forwarding element a public operation that cancels one existing outgoing INVITE transaction
without reconstructing CANCEL or transaction association outside the kernel.

## Acceptance

- [ ] The operation is anchored to the outgoing INVITE transaction created by `Handle::send`; a
      caller cannot accidentally cancel a different branch or synthesize a second transaction key.
- [ ] The CANCEL uses the INVITE's Request-URI, selected `Target`, top Via including its branch,
      Call-ID, To, From and CSeq number, changing only the method where RFC 3261 §9.1 requires it.
- [ ] Cancellation observes §9.1's provisional-response precondition: if requested before any
      provisional response, it waits for one before transmitting CANCEL; a final response that wins
      the race terminates cancellation without sending a late CANCEL.
- [ ] The caller receives a typed result associated with that INVITE branch. Transport failure, a
      final INVITE response that won the race and the CANCEL transaction's terminal outcome remain
      distinguishable without parsing logs.
- [ ] Existing UA cancellation uses the same primitive. The private `sipx-call::send_cancel`
      builder is removed or reduced to call policy over the public transport operation; there is
      one RFC 3261 §9.1 construction path in the workspace.
- [ ] Failing-first test: `a_forwarding_element_can_cancel_one_outgoing_invite_branch` requests
      cancellation before a loopback peer's `180`, proves no CANCEL precedes that `180`, and then
      verifies the original target, branch and §9.1 headers plus the returned branch identity.
- [ ] The transport spec records the operation, state/race table and byte-level CANCEL vector before
      implementation, and the full gate is green.

## Progress

- Re-filed on 2026-08-05 after the original T-28 filing was never merged and that ID was allocated
  to unrelated path-MTU work. Re-verified against `1.0.0-beta.5`: `Responses` still exposes only
  observation of the outgoing transaction, while the complete CANCEL builder remains private to
  `sipx-call`.

## Notes

- Requested by the downstream
  [sipx-clstr PX-12](https://github.com/codewandler/sipx-clstr/blob/main/docs/stories/PX-12-perform-the-cancel-and-timer-effects-the-driver-discards.md)
  through its [upstream ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md).
  The downstream proxy owns branch policy and Timer C; CANCEL construction and client-transaction
  control stay here.
