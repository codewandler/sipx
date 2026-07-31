---
id: X-56
title: Run the RFC 5118 corpus check from the gate, as its RFC 4475 twin already is
pillar: Build
status: in-progress
priority: 3
epic: conformance
areas: [scripts, ci]
note: found by X-51's evidence check — `import-rfc5118-corpus.sh --check` proves the twelve fixtures are still the RFC's own bytes, and no gate step and no CI job invokes it; the 4475 twin is run by the fuzz job
---

# Run the RFC 5118 corpus check from the gate, as its RFC 4475 twin already is

## Goal
Make the RFC 5118 fixtures as tamper-evident as the RFC 4475 ones already are, so M12's first clause
keeps holding rather than merely holding today.

## Acceptance
- [ ] **The gap, demonstrated first.** `scripts/import-rfc5118-corpus.sh --check` verifies the twelve
      Appendix A messages against the RFC's own archive. Grep the repository: nothing invokes it —
      not `scripts/gate.py`, not any job in `.github/workflows/ci.yml`. Its RFC 4475 counterpart *is*
      invoked, by the `fuzz` job. Show that a fixture edited by hand leaves the whole gate green.
- [ ] **It runs from a gate step and a CI job**, and `./scripts/gate.py --check` accounts for it —
      every command a CI job runs is either mirrored by a gate step or named in `NOT_RUN_LOCALLY`
      with a reason, which is the property `X-22` established.
- [ ] **The failing-first test is the tampered fixture.** Edit one byte of one 5118 message, show the
      new step red, restore it, show it green. A check that cannot detect a hand-edited corpus is the
      thing this story is about.
- [ ] Consider whether the 4475 and 5118 checks belong in the same step or job, and say which and
      why. Two corpora with one rule between them is one place to remember; two steps is two.

## Progress
- Filed 2026-07-30 by `X-51`, which ran the check by hand while verifying M12's first clause, found
  it passing, and noticed nothing would ever notice if it stopped.

## Notes
- **This is `X-31`'s lesson in a second corpus.** `X-31` made the 4475 corpus tamper-evident because
  a fixture that can be quietly edited is a conformance claim that can be quietly weakened. The 5118
  import script was written with the same `--check` mode and then wired to nothing.
- M12's first clause — "the whole 5118 corpus is classified and green" — genuinely holds today;
  `X-51` verified it and `crates/sipx-testkit/src/rfc5118.rs:300`'s `DEVIATIONS` list is empty. This
  story is about the claim staying true, not about it being false.
