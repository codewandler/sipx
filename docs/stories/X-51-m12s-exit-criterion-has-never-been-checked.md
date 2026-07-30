---
id: X-51
title: Check M12's exit criterion against evidence, now that all four of its stories are closed
pillar: Build
status: ready
priority: 2
epic: conformance
areas: [docs]
note: found integrating X-50 — X-16, X-17, X-18 and X-19 are all done, and nobody has ever asked whether M12's four Done-when clauses hold; M10 looked reached by status and was not
---

# Check M12's exit criterion against evidence, now that all four of its stories are closed

## Goal
Answer whether **M12 — Provable** is reached, from its evidence rather than from its story statuses,
and record the answer where the next reader will find it.

## Acceptance
- [ ] **Each of the four `Done when` clauses is checked against a named test, script or CI job**, not
      against a `status:` field. The clauses are: the whole RFC 5118 corpus classified and green; the
      interop script running against two independent implementations; every discard in the signalling
      path counted and exportable next to a capture of the traffic that caused it; and a fuzzer
      driving the transaction layer with sequences of timers and messages rather than bytes.
- [ ] **The trap `X-50` fell into is avoided explicitly.** All four stories being `done` is what
      prompts this and is not evidence for it — `T-20` and `T-21` are `done` and their tests do not
      demonstrate M10's clauses as written. State for each clause what would have to be true, then go
      and look.
- [ ] **The third clause is the one to read hardest.** `X-18` counted transport discards and
      deliberately refused the media half, which is `M-32` and is open. The clause says *signalling*
      path, so `M-32` may well be out of scope for it — but say so from the clause's words rather
      than assuming it either way, and check that "exportable next to a capture" is a thing that can
      actually be done today and not two features that exist separately.
- [ ] **Whatever the answer, `docs/roadmap.md` records it** — M12 moved to Delivered with its
      evidence named, or M12 left in Next with the specific gap written down. A milestone whose
      stories are all closed and whose status nobody can state is the condition this story exists to
      end.
- [ ] If M12 is reached, the ordering note under it — "last, and for a reason that is not
      deprioritisation" — is now wrong and is corrected in the same commit.

## Progress
- Filed 2026-07-30 while integrating `X-50`, which found the same question already answered wrongly
  for M10 by reading statuses instead of tests.

## Notes
- The likely-easy evidence: `interop (kamailio)` and `interop (asterisk)` both run as CI jobs, which
  is the second clause almost by inspection. `fuzz smoke` covers the fourth. The first and third are
  the ones that need reading.
- **Reads with `X-50`**, which established the method: name the clause, name the test, and say
  whether the test demonstrates the clause *as written*.
