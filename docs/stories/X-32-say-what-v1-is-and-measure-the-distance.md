---
id: X-32
title: Say what v1 is, and generate the distance to it
pillar: Build
status: done
design: docs/roadmap.md
epic: conformance
areas: [docs]
note: nothing in the repo defines v1 — the only `v1` is `sipx.app.v1`, a protocol version — so "how far are we" cannot be answered, only estimated
---

# Say what v1 is, and generate the distance to it

## Goal
Make "how far is sipx from 1.0" a question the repo answers mechanically, the way
`docs/compliance.md` answers "what does sipx implement".

## Acceptance
- [x] **v1 is defined as predicates, in `docs/vision.md` or `docs/roadmap.md`, before anything is
      measured.** Today the roadmap runs M0–M12 with an explicit "After M12" deferral list and
      never names 1.0; the only `v1` in the tree is `sipx.app.v1`, a protocol version. Each
      predicate must be checkable by a human reading the registry and the board — for example "no
      `media`-layer RFC is `partial`", or "every epic in the roadmap's Delivered section has no open
      story". A prose paragraph is not a definition.
- [x] A generated maturity report, from the two sources that are already machine-read and already
      checked: `docs/rfc/registry.toml` and story frontmatter. **Nothing hand-maintained.** This
      project has twice paid for a hand-maintained list drifting — the gate's command list (`X-22`)
      and the pool-key prose (`X-24`) — and both were fixed by generating them.
- [x] It reports **per layer and per pillar**, not one headline number. The aggregate hides the
      thing that matters: `media` is 15 RFCs with 5 implemented and 9 partial, while `core` is 9
      with 5 implemented and 2 partial. One percentage would call those the same.
- [x] **Partial counts as partial.** Whatever weighting is chosen is stated in the output itself, so
      a reader knows whether 61% means "61% of RFCs done" or "42 of 69 weighting partial at a half".
- [x] **It reports the discovery rate: stories filed versus closed, per day.** This is the story's
      most useful output and the least obvious. Story burn-down is not a maturity signal while
      discovery outpaces closure — for two of the last three days it did (60 filed/47 closed, then
      68/52), and the first day closure won was 2026-07-29 (32/41). The date that crossover becomes
      durable is the real maturity marker, because it is when the codebase stops surprising its
      authors.
- [x] **It states what it cannot see.** `status = "implemented"` over-reports: `X-30` demoted three
      rows the day it landed because the capability had no caller at the call layer, and its
      reachability check covers only `layer = "media"` — the general rule was measured and rejected
      22 of 29 role-claiming rows with just 3 justly. An index built on `implemented` inherits that
      blind spot everywhere except media, and must say so rather than imply a precision it lacks.
- [x] Runs in the gate, like `rfc-report.py --check`, so the report cannot lag the sources it is
      generated from.
- [x] Failing-first test: the generator's own suite, in the style of `scripts/test-rfc-report.py` —
      a fixture registry and a fixture story set with known counts, asserting the arithmetic and the
      weighting rather than eyeballing the output.

## Progress
- **Done.** `scripts/maturity.py` generates `docs/maturity.md` from `docs/rfc/registry.toml`, story
  frontmatter and git; `scripts/test-maturity.py` is its suite (14 tests); both are gate steps, taking
  the gate from 18 to **20**, and both are mirrored in `ci.yml` so `--check` stays satisfied.
- **v1 is now defined as five predicates** alongside the alpha's seven, in `docs/roadmap.md`. They are
  separate because every one needs something this repository cannot supply alone — chiefly *"the public
  API has been used from outside this repository"*, which no gate can assert and which the roadmap has
  always given as the reason to wait. **No feature count, no RFC total, no percentage** appears in
  either list, because the vision makes maximum feature count a non-goal.
- **The report's single most useful output is the one nobody asked for first**: filed versus closed per
  day, read from git. It shows **-37 on 2026-07-28, +4 on 2026-07-29, +1 on 2026-07-30** — so the
  crossover happened and has held two days. The report says in as many words that the marker is not a
  single winning day but the date the crossover becomes *durable*, because a shrinking board only means
  something once discovery has stopped outpacing it.
- **A predicate's state is derived from the board and nowhere else.** Each names the stories that close
  it, and is met only when all of them are `done`. `test_a_story_that_does_not_exist_is_unknown_not_met`
  pins the failure mode that would make the whole thing worthless: a blocker list pointing at nothing
  would otherwise read as *met*, so deleting a story would look like finishing it.
- **Two predicates are reported as `attested`, not `computed`, and say why.** "No known-wrong shipped
  path" cannot be computed — a defect nobody has found leaves no trace in either source — so what is
  reported is the absence of *open* stories describing one. `S-27` proves the difference: a `sips:` URI
  dialled in cleartext, found by reading code, not by any report.
- **No aggregate percentage exists and a test forbids one** (`test_no_aggregate_percentage_is_reported`).
  `media` is 15 RFCs with 11 partial; `security` is 11 with none partial. One number would call them
  alike.
- `test_the_reachability_layers_match_the_checker` ties the report's caveat to `rfc-report.py`'s actual
  scope, so widening the check there fails here until the caveat follows.
- Implemented by the coordinator rather than an implementor: delegation was unavailable on an org spend
  limit. Recorded because it breaks this project's normal separation.

## Notes
- **Asked for directly on 2026-07-29**: a maturity index — how much backlog and how much RFC
  coverage remains before this can be called v1. The honest first answer was that the question has
  no denominator yet, which is what makes the first Acceptance item the load-bearing one. Measuring
  against an undefined target produces a number that feels like progress and tracks nothing.
- **Snapshot at filing**, so the first generated report can be checked against a known state: 70
  RFCs — 31 implemented, 22 partial, 10 none, 6 syntax, 1 n/a; 127 stories — 99 done, 13 ready, 12
  backlog, 2 blocked, 1 in-progress; M0–M8 complete, M9–M12 open; open work by pillar Signalling 10,
  Media 6, Build 5, Application 5, Transport 1, Phone 1.
- **Do not let this become a dashboard.** The vision's non-goals discipline applies: one generated
  document that a release decision can be made from, not a metrics surface that needs its own
  maintenance. If it grows a second page it has failed.
- Reads with `X-30`, which is why the blind-spot item exists: that story made the registry's role
  claims trustworthy for media and explicitly not elsewhere, and recorded a cross-crate caller check
  as "what would widen this". Until that lands, `implemented` outside media means "the code exists",
  not "a call can reach it".
