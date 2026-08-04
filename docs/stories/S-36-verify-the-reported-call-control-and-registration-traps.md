---
id: S-36
title: Verify the reported call-control and registration traps
pillar: Signalling
status: backlog
priority: 14
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

- [ ] **Transfer success semantics.** A blind transfer reports success only on the `NOTIFY` carrying
      `sipfrag` `200`, never on the `202` accepting the REFER (RFC 3515 §2.4.6). A transfer whose
      target rejects must surface as failure, and the subscription and dialog must be torn down
      rather than leaked.
- [ ] **Hold with an empty offer.** A re-INVITE carrying **no** SDP body is accepted and answered
      per RFC 3261 §14.2 / RFC 3264 §6.1 rather than refused.
- [ ] **Hold direction mirroring.** An offer of `a=sendonly` is answered `a=recvonly`, never
      `sendrecv`, and audio resumes correctly on the subsequent unhold.
- [ ] **Registration refresh uses the granted expiry.** The refresh timer derives from the
      `Expires` in the 200, not the value requested, including when the registrar shortens it.
- [ ] **Registration robustness.** A `100 Trying` from a registrar does not fail the registration; a
      single failed refresh retries rather than tearing the registration down; a `stale=true`
      re-challenge is answered with a fresh nonce rather than treated as a credential failure
      (RFC 3261 §22.4).
- [ ] **In-dialog authentication.** A challenge on an in-dialog `BYE` or `MESSAGE` is answered.
- [ ] **Asymmetric dynamic payload types.** An answer whose dynamic payload number differs from the
      offer's is honoured in both directions — packets are sent with the peer's number and read with
      ours (RFC 3264 §6.1).
- [ ] Every test names the RFC section it enforces in a comment on the test.
- [ ] Progress records, per item, whether it passed first time or revealed a defect. An item that
      passed first time is a result, not a non-event.
- [ ] `./scripts/gate.py` green.

## Progress
- (not started)

## Notes
- These are field-reported failures from a survey of public issues against comparable stacks; see
  [`docs/designs/demand.md`](../designs/demand.md). They are cheap to check and expensive to
  discover in production.
- Deliberately one story rather than seven: it is a single pass through `sipx-call`, `sipx-ua` and
  offer/answer, and splitting it would multiply setup for no isolation benefit.
- If an item turns out to need substantial work, split *that item* out and keep the rest moving.
