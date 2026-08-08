---
id: P-29
title: Fund every phase from one process budget
pillar: Phone
status: ready
priority: 33
design:
epic: diagnostic-automation
areas: [sipx-cli]
predicate:
announcement:
note: P-26 bounded resolution BY the deadline rather than subtracting it FROM the deadline, so the worst case is two phases of the stated value
---

# Fund every phase from one process budget

## Goal

Make a command's stated deadline the budget for the whole process, not a value each phase gets a
fresh copy of. Today `dial --timeout 5` can spend five seconds resolving and then five more on the
invitation.

## Acceptance

- [ ] `dial`, `load` and `scenario` fund each phase from the remaining budget, the way `register`
      already does since `P-25`, so the stated deadline bounds the process rather than each phase.
- [ ] A failing-first test proves the worst case against a slow name plus a non-answering peer lands
      near the stated deadline, not near a multiple of it.
- [ ] `docs/specs/diagnostic-phone.md` §3.2's normative "starts when the initial INVITE is handed to
      the endpoint" is resolved deliberately — either restated, or kept with the accounting made
      explicit — and the decision is recorded rather than implied.
- [ ] The published `invitation_limit_ms` and `invitation_elapsed_ms` fields keep a stated meaning
      across the change, with a `CHANGELOG.md` entry if it moves. Tests assert those at exact values
      today.
- [ ] `dial`'s reference no longer describes the process bound as the sum of two named phases; with
      resolution bounded, the honest worst case is three.
- [ ] `load` consults `--duration` as well as `--timeout` when bounding resolution: a run with
      `--duration 2 --timeout 20` can currently spend eight seconds resolving.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `P-26`'s first deviation, which stopped here deliberately and said so.
  Bounding resolution by the deadline satisfied `P-26`'s acceptance; subtracting it would have
  changed the meaning of two published fields and rewritten a normative spec sentence, neither of
  which that story sanctioned.

## Notes

- `P-25`'s `register` is the reference: each candidate is funded from the remainder.
- This is a contract change, not a bug fix — treat the published field semantics as the hard part.
