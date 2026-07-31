---
id: X-52
title: Demonstrate M10's GRUU and push clauses as they are written
pillar: Build
status: done
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
- [x] **Two registrations of one address of record, and a call that reaches exactly one of them.**
      M10's clause is "one of two registrations of the same address of record can be called
      individually". `T-20`'s `a_request_to_a_gruu_reaches_the_instance_that_registered_it` is an
      `OPTIONS` against one agent and a stub registrar — it demonstrates that a GRUU is recognised
      and that a wrong one is refused, which is the mechanism, not the clause. The new test registers
      two instances of one AOR and places a **call** at one instance's GRUU, asserting the other
      instance never sees it.
- [x] **A pushed client that answers.** `T-21`'s
      `a_push_wakes_a_client_that_refreshes_its_binding_before_the_invite` asserts RFC 8599 §4.1.3's
      ordering — push, binding-refresh REGISTER, then the INVITE — and stops when the INVITE arrives.
      The clause is "a push wakes a client that held no connection **into an answered call**". Carry
      it through to an answered call with audio, or state why the answered half is not testable here
      and what would make it so.
- [x] **Failing-first for both.** Each new test must be red before the work and green after, and the
      redness must come from the clause rather than from the harness — a test that fails because two
      registrations were never set up proves nothing about being individually callable.
- [x] **`docs/roadmap.md`'s "Where M10 stands" block is updated in the same commit**, and if this
      closes the last gap, M10 moves to Delivered with the three tests named. `X-50` wrote that block
      and it becomes wrong the moment this lands.
- [x] No claim is made for a clause whose test does not demonstrate it. That substitution is the
      whole reason this story exists.

## Progress
- Filed 2026-07-30 by `X-50`, which checked M10's evidence rather than its statuses and found the ICE
  clause demonstrated and the other two not.
- Closed 2026-07-31, and **M10 is delivered**. Both clauses are demonstrated in
  `crates/sipx-cli/tests/reachable.rs`, beside the CLI's tests because this is the crate that already
  depends on registration, signalling, media and audio at once — which is what a clause about being
  *reached* is a claim about. Nothing calls the binary; the library is the subject.
- **Both tests passed the first time they ran.** Nothing in the stack was broken: M10 was short of
  evidence, not of behaviour. That makes the third Acceptance item unsatisfiable as written — there
  was no defect to be red about — so it is satisfied by **falsification** instead, which is the
  honest equivalent and is recorded as a deviation rather than as a pass:
  - Making `Gruus::from_response` select a binding by position instead of by `+sip.instance` — the
    failure that function's own doc comment names — has both instances adopt the same GRUU, and
    `each_of_two_registrations_of_an_address_of_record_is_called_individually` fails naming it:
    "two instances of one AOR were issued the same GRUU, which names neither".
  - Discarding the PURR RFC 8599 §8.2 assigns the binding fails
    `a_push_wakes_a_client_that_held_no_connection_into_an_answered_call` on the assertion that the
    PURR names the binding the held request is released down.
  - Both mutations were reverted and both tests confirmed green again, and both ran green inside the
    full 25-step gate. The audio-carrying half of the push clause is asserted but was not
    separately mutated: its controls are the clip's own loudness and G.711 equality both ways, so
    silence of the right length cannot pass it.
- The routing double reads the Request-URI and nothing else, through the library's own
  `sipx_sip::gruu::gr_value` rather than by URI equality. RFC 5627 §5.4 warns that a public GRUU is
  URI-equal to the address of record, so a double comparing URIs would fan every GRUU out to both
  instances and pass while demonstrating the opposite of the clause.
- `docs/roadmap.md` was updated in the **ledger commit** rather than in the implementor's commit, a
  deliberate deviation from this story's fourth item: the roadmap is a shared ledger fenced out of
  every implementor worktree, because otherwise two stories in one wave collide on it. The block
  was written from the evidence this story produced.
- **Strengthened after the story closed, 2026-07-31.** The implementor was resumed, kept working, and
  was killed a second time with the result uncommitted in a worktree the harness had already
  reclaimed once; it was rescued, brought current with `main`, formatted and gated. Its version is a
  better answer to the last Acceptance item than what was first merged: the integrated test asserted
  that the instance the GRUU does not name never sees the INVITE, but the only thing standing between
  that instance and the call was the test's own routing double — so the clause was demonstrated by
  the *harness* declining to deliver rather than by sipx declining to answer. Both instances are now
  passed to the call helper and the un-named one is asserted **not to recognise** the GRUU as its
  own. It is not asserted to refuse a call so addressed — the independent review delivered that
  INVITE to the un-named instance and it answered and carried audio, because `sipx-call` reads no
  `gr` parameter at all (`X-59`). The claim first written here, that it would refuse, was false and
  is corrected rather than left standing. The arrival check is INVITE-specific rather than "did anything arrive",
  because an instance that has taken a call of its own has its own ACK and in-dialog traffic waiting.
  The test is renamed `each_of_two_registrations_of_an_address_of_record_is_called_individually`, and
  the roadmap and this file were corrected to name it. Re-falsified by the same mutation, green
  again, and green inside the full 25-step gate.
- The implementor was killed mid-run by an org monthly spend limit. Its work was rescued to
  `impl/X-52`, and the falsification, the gate and the roadmap block were done at integration. It
  never reached the gate, so `cargo fmt` had not run on the file; two call sites were rewrapped.
  Reviewed by one context rather than two.

## Notes
- **This is the whole remaining distance to M10** under the reading `X-50` settled: the third clause
  is demonstrated by `M-27`'s `a_call_uses_a_nominated_pair_when_both_host_candidates_are_silent`,
  and `M-24`'s relay is explicitly not in the milestone.
- Neither `T-20` nor `T-21` is reopened. Both delivered the mechanism they were written for; this is
  the composing demonstration on top, which is a different story and was never in their Acceptance.
- **Reads with `X-51`**, the same question asked of M12.
