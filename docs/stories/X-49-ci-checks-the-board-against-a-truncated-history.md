---
id: X-49
title: Stop CI checking the board against a history it did not fetch
pillar: Build
status: done
priority: 1
epic: conformance
areas: [scripts, ci]
note: found by CI going red on main and on every pull request at 1.0.0-alpha — two jobs failed, both accusing docs/maturity.md's event-date journal of recording a fact the snapshot did not have
---

# Stop CI checking the board against a history it did not fetch

## Goal
Give `maturity.py` the history it reads, and make a checkout that cannot answer say so — so a
truncated history stops being reported as a corrupt report.

## Acceptance
- [x] **The two red jobs are green.** `gate consistency` (`test-maturity.py`) and `rfc compliance`
      (`maturity.py --check`) both failed with `maturity: event-date journal records 173 filed facts,
      but the snapshot has 172 committed + 0 pending`, on `main` at `576f0dd` and on the
      `release/1.0.0-alpha.1` pull request. Both checkouts now fetch the full history.
- [x] **The cause is the checkout depth, not the journal.** `actions/checkout` defaults to
      `fetch-depth: 1`. `history_story_fact_days` reads filing days from
      `git log --diff-filter=A -- docs/stories`, and in a grafted single-commit checkout every story
      file present reads as *added by that commit*: the filed count becomes the number of story files
      that exist, all dated to the checkout. Reproduce with
      `git clone --depth 1 file://$PWD tmp && cd tmp && ./scripts/maturity.py --check`.
- [x] **The two counts agreed until a story was renumbered.** `eee4394` refiled `P-6` as `P-7`, which
      is two filings and one surviving file. That is the entire delta between 173 and 172, and it is
      why this was latent for the whole life of the check rather than red from the first commit.
- [x] **A checkout that cannot date a filing refuses instead of answering.** `shallow_history()`
      raises with the fix (`git fetch --unshallow`, or `fetch-depth: 0`) rather than reporting a
      depth-dependent count. Degrading to "rate unavailable" was rejected: `--check` would then fail
      as report drift, which is the same misdirection one step further away.
- [x] Failing-first test:
      `test-maturity.py::test_a_shallow_checkout_is_refused_rather_than_miscounted` builds the
      renumber shape in the fixture repository — three filings, two files — clones it at depth 1, and
      requires the diagnostic to name the checkout and *not* the journal. Red before the guard, where
      it read `event-date journal basis does not match the facts and attributed dates`. It also
      asserts the same checkout is green once unshallowed, so the guard is about the depth rather
      than about being a clone.

## Progress
- Done. `ci.yml` gives both jobs `fetch-depth: 0`; `maturity.py` grew `shallow_history()` and the
  refusal in `history_story_fact_days`; the suite is 50 tests and green, and `gate.py --check` still
  accounts for every job.

## Notes
- **The diagnostic pointed at the one file that was right.** The journal in `docs/maturity.md`
  recorded 173 filings because 173 filings happened. A check that compares a generated artefact
  against a source it may not have needs to validate the source first — the same shape as `X-35`
  finding `X-26`'s guard passing because it read three strings and the README's crate table was not
  one of them.
- `provenance` already ran with `fetch-depth: 0` for `check-provenance.sh --history`, so the pattern
  and its `with:` block were already in `ci.yml`; the two jobs that also read history did not have it.
- `gate.py --check` compares *commands*, not checkout options, so it could not have caught this. A
  job whose local step passes and whose CI step reads different data is a gap in the property `X-22`
  established; worth a story if a third instance appears.
