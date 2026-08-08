---
id: S-36
title: Verify the reported call-control and registration traps
pillar: Signalling
status: done
priority:
design: docs/designs/demand.md
epic: demand
areas: [sipx-call, sipx-ua, sipx-sdp]
predicate:
announcement:
note: six field-reported failure modes · each becomes a passing test or a defect · cheap, high information
---

# Verify the reported call-control and registration traps

## Goal

Settle, by test, whether sipx exhibits six failure modes independently reported against comparable
stacks — turning each into either a pinned behaviour or a filed defect.

## Acceptance

Each item below is a failing-first test. Where sipx already behaves correctly, the test still lands
and pins the behaviour; where it does not, the fix ships with it or a defect story is filed and
linked from Progress.

- [x] **Transfer success semantics.** A blind transfer reports success only on the `NOTIFY` carrying
      `sipfrag` `200`, never on the `202` accepting the REFER (RFC 3515 §2.4.6). A transfer whose
      target rejects must surface as failure, and the subscription and dialog must be torn down
      rather than leaked.
- [x] **Hold with an empty offer.** A re-INVITE carrying **no** SDP body is accepted and answered
      per RFC 3261 §14.2 / RFC 3264 §6.1 rather than refused.
- [x] **Hold direction mirroring.** An offer of `a=sendonly` is answered `a=recvonly`, never
      `sendrecv`, and audio resumes correctly on the subsequent unhold.
- [x] **Registration refresh uses the granted expiry.** The refresh timer derives from the
      `Expires` in the 200, not the value requested, including when the registrar shortens it.
- [x] **Registration robustness.** A `100 Trying` from a registrar does not fail the registration; a
      single failed refresh retries rather than tearing the registration down; a `stale=true`
      re-challenge is answered with a fresh nonce rather than treated as a credential failure
      (RFC 3261 §22.4).
- [x] **In-dialog authentication.** A challenge on an in-dialog `BYE` or `MESSAGE` is answered.
- [x] **Asymmetric dynamic payload types.** An answer whose dynamic payload number differs from the
      offer's is honoured in both directions — packets are sent with the peer's number and read with
      ours (RFC 3264 §6.1).
- [x] Every test names the RFC section it enforces in a comment on the test.
- [x] Progress records, per item, whether it passed first time or revealed a defect. An item that
      passed first time is a result, not a non-event.
- [x] `./scripts/gate.py` green.

## Progress
- 2026-08-05: selected for the post-beta.7 foundations and field-hardening wave. Existing coverage
  already proves transfer completion from final NOTIFY, granted registration expiry, fresh-nonce
  handling, direction mirroring and INFO/MESSAGE authentication. The sweep is adding the exact
  missing end-to-end observations before deciding which reported cases are defects.
- 2026-08-05: transfer success/failure, granted-expiry scheduling, sendonly/recvonly mirroring,
  stale-nonce handling and in-dialog digest retry all passed on their existing implementations;
  the hold test gained a post-resume audio assertion. A registrar `100 Trying` also passed first
  time. Focused results: registration 14/14, transfer 4/4, and the digest, hold and SDP tests green.
- 2026-08-05: three checks revealed defects and landed failing-first fixes. A bodyless re-INVITE
  returned 488 instead of carrying an offer in its 200; refresh registration stopped after one
  transient failure; and media used one dynamic payload number for both directions. Delayed offer
  state now settles the ACK answer, refresh uses the granted lease margin for one bounded retry,
  and the media/call/snapshot contracts retain distinct transmit and receive payloads. Snapshot
  version two remains backward-compatible with symmetric version-one bytes.

## Notes
- These are field-reported failures from a survey of public issues against comparable stacks; see
  [`docs/designs/demand.md`](../designs/demand.md). They are cheap to check and expensive to
  discover in production.
- Deliberately one story rather than seven: it is a single pass through `sipx-call`, `sipx-ua` and
  offer/answer, and splitting it would multiply setup for no isolation benefit.
- If an item turns out to need substantial work, split *that item* out and keep the rest moving.
