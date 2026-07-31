---
id: X-59
title: Refuse an INVITE addressed to another instance's GRUU, or write down why answering it is right
pillar: Application
status: ready
priority: 4
epic: conformance
areas: [sipx-call, sipx-ua]
note: found by the independent review of X-52 — an INVITE whose Request-URI is instance one's public GRUU, delivered to instance two's flow, is answered by instance two and carries audio; `sent_to_our_gruu` is a predicate nothing but tests call, and `crates/sipx-call/src/` contains no GRUU reference at all
---

# Refuse an INVITE addressed to another instance's GRUU, or write down why answering it is right

## Goal
Decide what a call should do with a request addressed to a GRUU it does not own, so that "one of two
registrations can be called individually" is a property of the stack wherever the stack can hold it,
and an honest statement of scope wherever it cannot.

## Acceptance
- [ ] **Reproduce it.** An INVITE whose Request-URI is instance one's public GRUU, delivered to
      instance two's flow, is **answered** by instance two, and audio flows both ways. The review
      probed exactly this and the probe is the failing-first test: nothing in
      `crates/sipx-call/src/` reads a `gr` parameter — `grep -rn gruu crates/sipx-call/src/` returns
      nothing — and `UserAgent::sent_to_our_gruu` (`crates/sipx-ua/src/agent.rs:352`) is a predicate
      called from tests only.
- [ ] **Decide, and say why in the spec rather than only here.** Two defensible answers and the
      story is choosing one:
      - **Refuse.** A UA that recognises its own GRUU can refuse one that is not — a `404` is the
        shape RFC 5627 §5.4 leaves available — which makes individual callability hold end to end
        even when something upstream misroutes.
      - **Answer, deliberately.** In RFC 5627 the *registrar* mints a GRUU and a *proxy* resolves it
        to one binding, and sipx is the UA half of that RFC and implements neither. On that reading a
        request arriving at this flow was routed here by something entitled to route it, and second-
        guessing the Request-URI would break legitimate forwarding.
      Whichever is chosen, it stops being an accident: today the behaviour is not a decision, it is
      the absence of one.
- [ ] **The demonstration follows the decision.** If refusal is chosen, `X-52`'s
      `each_of_two_registrations_of_an_address_of_record_is_called_individually` gains the assertion
      it currently only approximates, and the clause is held by the stack rather than by the test's
      `route()`. If answering is chosen, the test's routing double is *correct* and the thing to fix
      is the claim about it — see the item below.
- [ ] **`docs/roadmap.md`'s M10 record matches whichever it is.** The Delivered entry and the "Where
      M10 stands" block were corrected on 2026-07-31 to disclose that the registrar and the
      resolution are doubles and that what sipx holds is per-instance GRUU learning, presentation and
      recognition. If this story makes refusal real, that disclosure narrows; if it makes answering
      deliberate, the disclosure is the permanent answer and should cite this story.

## Progress
- Filed 2026-07-31 by the independent review of `X-52`, which did not take the strengthened test's
  word for it and instead delivered the misaddressed INVITE itself.

## Notes
- **`X-52` is not reopened and M10 is not withdrawn.** The review confirmed the other two clauses
  hold on evidence — it falsified the audio half by forcing the send path to silence, and falsified
  the push clause at its centre by removing §4.1.3's refresh from `UserAgent::woken`, which fails the
  test on the PURR because the PURR is a witness that the refresh happened and was answered. What was
  wrong was a sentence in the ledger, and that has been corrected.
- **This is the same substitution one level out.** `X-50` found M10's clauses held by mechanism
  rather than demonstration; `X-52` fixed that; and then the ledger describing `X-52` claimed a
  refusal the stack does not have. The lesson worth keeping is that the ledger outlives the test's
  own prose, and a reader checks the milestone against the ledger.
- The review also noted two weaker spots that are not this story: the push test's `register` → `invite`
  ordering is enforced by the harness's own oneshot, so only `push` → `register` is evidence about
  sipx; and M10's third clause asserts received *length* of a constant tone
  (`crates/sipx-call/tests/ice_call.rs:410-415`) rather than the G.711 equality `reachable.rs` uses.
