---
id: X-136
title: The load candidate count goes null under suite load
pillar: Quality
status: done
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

- [x] The failure is reproduced deliberately rather than waited for — `scripts/contention-proof.py`'s
      technique is the instrument, and `X-126`/`X-130` are the precedent for magnifying a fixture
      until the mechanism fires rather than widening a threshold on faith.
- [x] Which of the two readings holds is settled with evidence, and written down. If the counts are
      genuinely lost, that is a defect in `load` and this story fixes it there, not in the test.
- [x] Whatever is decided, the assertion afterwards is non-vacuous: a run that truly walked no pass
      must still be distinguishable from one whose count was dropped.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-09: filed from the rc.14 gate rather than fixed in it. The candidate is `T-44`'s bounded
  run — it now closes admission and waits for admitted calls instead of cancelling them — and
  `record_pass`'s deliberate silence on a poisoned lock, which is correct for a summary field and
  indistinguishable from "nothing recorded one".

- 2026-08-09: **the second reading holds — the counts were dropped, not legitimately absent** — and
  the defect is one layer below `record_pass`. Fixed in `sipx_transport::destination`.

  *Reproduced,* by magnifying the fixture rather than waiting for a loaded box (`X-130`'s
  precedent). A `load` run whose first candidate accepts the connection and answers nothing prints,
  today, the summary the gate reported character for character:

  ```
  {"candidates_attempted":null,"candidates_resolved":null,...,
   "outcomes":{"attempted":1,"connected":0,"failed":0,"peak_concurrency":1,"rejected":0,
               "timed_out":1},...}
  ```

  `outcomes.attempted: 1` and `peak_concurrency: 1` are what settle it: **a call was admitted and it
  walked a pass.** `T-44`'s bounded run is exonerated — the first reading requires a bound that
  closed admission before any call walked a candidate, and this run admitted one, attempted a
  candidate and reported `null` anyway.

  *The mechanism.* `load` classifies a response deadline as the far end answering for the name
  (deliberate, `T-42`), so a candidate that runs out of the pass's budget instead of refusing
  promptly ends the pass as `Unreached::Answered` — and that variant carried no `Attempts`, so the
  depth it had reached was destroyed before `record_pass` ever saw it. A refusal is prompt on an
  idle box and the pass ends `Unreachable` with its count intact, which is why the test passes alone
  in 0.05 s; a busy host is exactly where a candidate stops refusing promptly.

  *The fix.* `Unreached::Answered` now carries `attempts` like every other ending, and
  `Unreached::attempts()` returns `None` only for `Nothing`. The variant's own doc claimed it was an
  ending "no candidate was attempted for", which `walk`'s control flow contradicts: `Answered` is
  reached only from inside the loop, after `attempts.attempt()` has counted the candidate that
  produced the answer. `dial`, `peers`, `scenario` and `load` all read the pair through
  `outcome.attempts()`, so all four now report how far a pass got when an answer ended it.

  *Non-vacuity.* The two endings now report different numbers — `1 of 3` for a pass an answer cut
  short at the head of the list, `3 of 3` for a name whose every address refused — and
  `a_pass_with_nothing_to_attempt_reports_no_depth_at_all` pins the other half: absent still means
  no pass ran. Where a `load` summary can still print `null`, `outcomes.attempted` beside it
  separates "no call was admitted" from "a call was and reached nothing this counts".

  *What was not reproduced.* The original test's own fixture never went red under load here: 8 runs
  under 40 CPU burners, then 100 runs at 20-way process and port churn under 20 burners, all green.
  So the trigger that starved that particular candidate on the gate box is not itself demonstrated —
  only that the ending it produces destroys the count, which is the defect the story is about.

  *Owed CHANGELOG sentence* (not written here — the ledger is the coordinator's):
  `Fixed: a candidate pass ended by an answer now reports how far it got, so sipx load no longer
  prints candidates_attempted: null for a run that walked the list (X-136).`

- 2026-08-09: closed at the `1.0.0-rc.15` boundary, against the wave gate run on this tree.
