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
- 2026-07-30: **checked — M12 is not reached, and only the third clause is short.** `docs/roadmap.md`
  gains a "Where M12 stands" block naming the evidence for each clause, in `X-50`'s shape. The
  statuses were not consulted for any of it; every clause was read against the test or CI job that is
  supposed to demonstrate it, and each of those was opened and run.
- **Clause 1, the corpus — holds.** `crates/sipx-testkit/src/rfc5118.rs` classifies twelve messages
  across §4.1–§4.10 with none `Unreferenced`, `DEVIATIONS` is empty (`S-31` closed `X-16`'s one entry),
  and `cargo test -p sipx-sip --test rfc5118_corpus -p sipx-sdp --test rfc5118_sdp` runs 15 + 4 tests,
  all green. `recorded_deviations_still_hold` keeps the empty list from becoming a lie.
- **Clause 2, two peers — holds.** `run.sh --list` reports two profiles; `.github/workflows/ci.yml`
  builds the `interop` matrix from that list and runs one job per peer, `fail-fast: false`. Both play
  `server` and run the identical nine-test list `run.sh` owns (`ROLE_TESTS[server]`), and neither
  profile declares a `PEER_DIVERGES_ON` any longer — `T-23` closed the one `X-17` found.
- **Clause 4, the fuzzer — holds.** CI job `fuzz`, step "Fuzz the transaction driver", 60 s a push over
  seventeen committed seeds, corpus proven unmodified by `check-corpus-untouched.sh`. Messages are
  built rather than parsed. `KNOWN_DEFECTS` is empty after `S-26`, and
  `the_campaign_suppresses_nothing_and_run_agrees_with_run_strict` proves it; 12 tests green.
- **Clause 3, counted and exportable next to a capture — short, in two specific ways**, both now in the
  roadmap and carried into `X-54`. The *shape* the clause asks for does exist and is not vapour:
  `a_datagram_that_does_not_parse_is_still_captured` asserts the offending bytes are in the pcapng and
  the `parse_failures` counter rose. But "every" reaches only `sipx-transport` — the guard scans that
  crate's `src` and nothing else, while `sipx-call` drops a call event uncounted at `event.rs:299` and
  discards CANCEL/ACK/BYE send results at six sites in `call.rs` (best-effort by design, which is a
  reason and not a count — and nothing enumerates the crate to catch the next one) — and "next to" is false outside the
  process: `Handle::counters` and `Calls::counts` are read by no code in the workspace but the crates'
  own tests, while `--capture <FILE>` is on three CLI commands with no counterpart for the numbers.
- **The third clause's `M-32` question, answered from the clause's words**: it says *signalling* path,
  so the media counters `X-18` split out are outside it and `M-32` staying open does not hold M12 open.
- **Filed `X-54`** with the census and the failing-first test the fix needs. No test and no code was
  changed by this story; the deliverable is the finding.
- **Adjacent, not fixed and not filed**: `scripts/import-rfc5118-corpus.sh --check` — the thing that
  proves the twelve fixtures are still the RFC's own bytes — is invoked by no gate step and no CI job,
  unlike its RFC 4475 twin, which the `fuzz` job runs. Run by hand here, it passes ("corpus matches
  RFC 5118 (12 messages)"), so clause 1 holds today; nothing would notice if a fixture were edited
  tomorrow. That is a ratchet gap, not a clause gap.

## Notes
- The likely-easy evidence: `interop (kamailio)` and `interop (asterisk)` both run as CI jobs, which
  is the second clause almost by inspection. `fuzz smoke` covers the fourth. The first and third are
  the ones that need reading.
- **Reads with `X-50`**, which established the method: name the clause, name the test, and say
  whether the test demonstrates the clause *as written*.
