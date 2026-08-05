---
id: T-31
title: Cancel an outgoing INVITE transaction from a forwarding element
pillar: Signalling
status: ready
priority: 1
design: docs/designs/sip-transport.md
epic: sip-transport
areas: [sipx-transport, sipx-sip]
predicate:
note: requested by sipx-clstr PX-12 — the UA has a private builder, while a proxy cannot cancel one branch through Handle. Re-filed — the first filing (as T-28, commit 09d5518) was never merged and its ID was recycled
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
      the race terminates the cancellation without sending a late CANCEL.
- [ ] The caller receives a typed transaction/delivery result associated with that INVITE branch.
      Transport failure, a final INVITE response that won the race, and the CANCEL transaction's
      terminal outcome are distinguishable without parsing logs.
- [ ] Existing UA cancellation uses the same primitive. The private `sipx-call::send_cancel` builder
      is removed or reduced to call policy over the public transport operation; there is one RFC
      3261 §9.1 construction path in the workspace.
- [ ] Failing-first test: `a_forwarding_element_can_cancel_one_outgoing_invite_branch` runs a real
      loopback peer, requests cancellation before the peer's `180`, then asserts that no CANCEL
      precedes the `180`, the eventual CANCEL reaches the INVITE's original target with the same
      branch and §9.1 headers, and the returned result names that branch. The minimal API step is an
      operation on the `Responses`/outgoing-transaction value returned by `Handle::send`; it does
      not compile against `v1.0.0-beta.4`, whose public handle exposes no operation that acts on an
      outgoing transaction.
- [ ] The transport spec records the operation, state/race table and byte-level CANCEL vector before
      implementation.

## Progress

- Filed from a downstream kernel-boundary review. No implementation has started.
- **Re-filed 2026-08-05.** The first filing (as `T-28`, commit `09d5518`, branch
  `filing/clstr-CX-7-public`) was pushed and never merged; `main`'s backlog work later allocated
  `T-28` to an unrelated path-MTU story, so the ask silently left the backlog. Content re-verified
  against `v1.0.0-beta.4` before re-filing: `impl Handle`
  (`crates/sipx-transport/src/endpoint.rs:489`) still exposes no cancel operation — its command
  surface ends at `send`, `send_directly`, `send_to_uri`, `respond` — and the only complete §9.1
  builder is still the private `sipx-call/src/call.rs:6075` `send_cancel`.

## Notes

- At `v1.0.0-beta.4`, `sipx-transport/src/endpoint.rs` stores the outgoing transaction's `Target`
  and exposes its event stream as `Responses` (`peek`, `next`, `final_response`,
  `take_transport_error`), but exposes no command that acts on that transaction.
  `sipx_sip::TransactionKey::for_cancelled_invite` already supplies the generic association rule
  for the receiving half.
- Requested by the downstream
  [sipx-clstr PX-12](https://github.com/codewandler/sipx-clstr/blob/main/docs/stories/PX-12-perform-the-cancel-and-timer-effects-the-driver-discards.md)
  through its [upstream ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md).
  The downstream proxy owns branch policy and Timer C; CANCEL construction and client-transaction
  control stay here.
