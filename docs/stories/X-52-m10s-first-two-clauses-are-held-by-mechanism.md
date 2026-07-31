---
id: X-52
title: Demonstrate M10's GRUU and push clauses as they are written
pillar: Build
status: in-progress
priority: 3
epic: conformance
areas: [sipx-call, sipx-ua, sipx-cli]
note: found by X-50 — T-20 and T-21 are done and their tests stop short of the clauses; the GRUU test is an OPTIONS to one agent and the push test ends when the INVITE arrives, so nothing answers it
---

# Demonstrate M10's GRUU and push clauses as they are written

## Goal
Close the gap `X-50` found between M10's first two clauses and the tests that are supposed to
demonstrate them, so the milestone can be recorded on evidence rather than on mechanism.

## Acceptance
- [ ] **Two registrations of one address of record, and a call that reaches exactly one of them.**
      M10's clause is "one of two registrations of the same address of record can be called
      individually". `T-20`'s `a_request_to_a_gruu_reaches_the_instance_that_registered_it` is an
      `OPTIONS` against one agent and a stub registrar — it demonstrates that a GRUU is recognised
      and that a wrong one is refused, which is the mechanism, not the clause. The new test registers
      two instances of one AOR and places a **call** at one instance's GRUU, asserting the other
      instance never sees it.
- [ ] **A pushed client that answers.** `T-21`'s
      `a_push_wakes_a_client_that_refreshes_its_binding_before_the_invite` asserts RFC 8599 §4.1.3's
      ordering — push, binding-refresh REGISTER, then the INVITE — and stops when the INVITE arrives.
      The clause is "a push wakes a client that held no connection **into an answered call**". Carry
      it through to an answered call with audio, or state why the answered half is not testable here
      and what would make it so.
- [ ] **Failing-first for both.** Each new test must be red before the work and green after, and the
      redness must come from the clause rather than from the harness — a test that fails because two
      registrations were never set up proves nothing about being individually callable.
- [ ] **`docs/roadmap.md`'s "Where M10 stands" block is updated in the same commit**, and if this
      closes the last gap, M10 moves to Delivered with the three tests named. `X-50` wrote that block
      and it becomes wrong the moment this lands.
- [ ] No claim is made for a clause whose test does not demonstrate it. That substitution is the
      whole reason this story exists.

## Progress
- Filed 2026-07-30 by `X-50`, which checked M10's evidence rather than its statuses and found the ICE
  clause demonstrated and the other two not.

## Notes
- **This is the whole remaining distance to M10** under the reading `X-50` settled: the third clause
  is demonstrated by `M-27`'s `a_call_uses_a_nominated_pair_when_both_host_candidates_are_silent`,
  and `M-24`'s relay is explicitly not in the milestone.
- Neither `T-20` nor `T-21` is reopened. Both delivered the mechanism they were written for; this is
  the composing demonstration on top, which is a different story and was never in their Acceptance.
- **Reads with `X-51`**, the same question asked of M12.
