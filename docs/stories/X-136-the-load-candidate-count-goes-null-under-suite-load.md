---
id: X-136
title: The load candidate count goes null under suite load
pillar: Quality
status: ready
priority: 3
design:
epic: test-surfaces
areas: [sipx-cli]
predicate:
announcement:
note: a_load_run_that_reaches_no_address_reports_what_it_attempted reported candidates_attempted null in a full workspace run and 3 alone
---

# The load candidate count goes null under suite load

## Goal

Make `a_load_run_that_reaches_no_address_reports_what_it_attempted` assert what it means under the
contention a full workspace run produces, or establish that the counts themselves are unreliable
there — which would be a defect in `load` and not in the test.

## Why

Observed in the `1.0.0-rc.14` wave gate, on a box that had just finished building six stories:

```
assertion `left == right` failed: signalling: every address behind the name was attempted, and the
summary has to say so: {"candidates_attempted":null,"candidates_resolved":null,...}
  left: Null
 right: Number(3)
```

The same test passes in isolation in 0.05 s, and passed in a full `sipx-cli` suite on `T-46`'s
branch (106 passed). So it is not a logic regression from any one merge.

**Which of two things it is has not been established, and that is the work.** Either the test is
asserting a count that a loaded machine legitimately does not produce — a bound closing admission
before any call walks a failing pass, in which case `T-44`'s change to bounded runs is the thing to
read — or `record_pass` is losing a pass it should have kept, in which case an operator's
`candidates_attempted` is unreliable on a busy host and that is a real defect in `sipx load`.

The second reading is the one that matters, because the field exists precisely so a failing run can
say how far it got. A count that silently becomes `null` under load tells an operator nothing and
looks like "no pass was walked".

## Acceptance

- [ ] The failure is reproduced deliberately rather than waited for — `scripts/contention-proof.py`'s
      technique is the instrument, and `X-126`/`X-130` are the precedent for magnifying a fixture
      until the mechanism fires rather than widening a threshold on faith.
- [ ] Which of the two readings holds is settled with evidence, and written down. If the counts are
      genuinely lost, that is a defect in `load` and this story fixes it there, not in the test.
- [ ] Whatever is decided, the assertion afterwards is non-vacuous: a run that truly walked no pass
      must still be distinguishable from one whose count was dropped.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-09: filed from the rc.14 gate rather than fixed in it. The candidate is `T-44`'s bounded
  run — it now closes admission and waits for admitted calls instead of cancelling them — and
  `record_pass`'s deliberate silence on a poisoned lock, which is correct for a summary field and
  indistinguishable from "nothing recorded one".
