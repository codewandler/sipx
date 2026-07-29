---
id: X-31
title: Close the drift holes in the transaction-sequence harness
pillar: Build
status: done
design: docs/designs/sip-core.md
epic: sip-core
areas: [sipx-testkit, docs]
note: found by X-19's independent review — one invariant arm is unfalsifiable by pigeonhole, the timer table is hand-maintained with a const assert that cannot catch drift, and the CI corpus check cannot see added files
---

# Close the drift holes in the transaction-sequence harness

## Goal
Make `X-19`'s fuzzing harness fail when it should. Three of its guards cannot currently catch the
thing they were written to catch, and a fuzzer that silently stops covering something is worse than
one that was never written, because the green campaign is read as evidence.

## Acceptance
- [x] **`Invariant::StoreGrowth`'s headline arm can fire, or it goes.**
      `crates/sipx-testkit/src/transaction_sequence.rs:988` checks `live > MAX_LIVE_TRANSACTIONS`,
      where `MAX_LIVE_TRANSACTIONS = 2 * SLOTS * FOLDED_METHODS = 40` (line 132) — and the
      vocabulary can name at most 4 slots × 5 folded methods per space = 40 distinct keys, held in
      two `HashMap`s. Pigeonhole makes it unfalsifiable. The invariant's other two arms
      (`live > self.tracked.len()` at :998, and the post-quiescence check at :1046-1064) are
      genuinely falsifiable and carry it, so this is decoration in a file that is otherwise careful
      about exactly this.
- [x] **The timer table cannot silently stop covering a timer.** `TIMERS`
      (`transaction_sequence.rs:75-89`) is hand-maintained and the const assert at :115 only checks
      `TIMER_COUNT == TIMERS.len()` — not that the table covers the `Timer` enum. A fourteenth
      variant would never be fuzzed and nothing would fail, which is the drift the comment at :109
      says it is guarding against. Make the assert cover the enum, or generate the table from it.
- [x] **The corpus check sees added files, not only modified ones.** CI's "Verify the
      transaction-sequence corpus is untouched" (`.github/workflows/ci.yml:268-269`) is
      `git diff --exit-code`, which is blind to new untracked files; the parser equivalent
      (`scripts/import-rfc4475-corpus.sh:51`) uses `diff -r --brief` and is not. `X-19`'s
      Acceptance said "exactly as the parser targets are". Currently mitigated only by
      `the_committed_corpus_is_exactly_the_seed_programs`.
- [x] **`docs/rfc/registry.toml`'s RFC 2543 row stops lagging the spec.** Line 264 still describes
      2543-style matching with no counterpart to the "Known deviation" note `X-19` added to
      `docs/specs/sip-transaction.md` §6.2. Literally true as written ("for a request"), but the
      table is linked as a measurement and now trails the spec. **Coordinate with `S-26`** — if
      that story has landed, the deviation is gone and this item is instead about making sure no
      stale note survives it.
- [x] Failing-first test for each of the first three: a mutation that the guard should catch and
      currently does not — a fourteenth `Timer` variant, a corpus file added rather than edited.

## Progress
- **Done.** The three guards that could not catch the thing they were written for now can, and the
  fourth item turned out to have already been resolved by `S-26`.
- **Item 1 — the unfalsifiable arm is deleted, not rescued.** `MAX_LIVE_TRANSACTIONS = 40` and the
  vocabulary names at most 40 keys, so `live > MAX_LIVE_TRANSACTIONS` could never fire. The invariant's
  other two arms (`live > self.tracked.len()` and the post-quiescence check) are genuinely falsifiable
  and carry it. The constant is kept only as documentation, with a comment saying why it is no longer
  asserted — deleting it would have made the doc below it wrong.
- **Item 2 — the table and the enum now agree in both directions.** A `const TIMER_COUNT == TIMERS.len()`
  assert proved only that the two were the same *size*; a fourteenth `Timer` variant would have been
  silently never-fuzzed. `timer_row` is an exhaustive match, so adding a variant is a compile error
  *there*, and a runtime test — `the_table_and_the_enum_agree_row_for_row` — round-trips every row so
  the table and the enum cannot drift on order either. Exhaustiveness alone cannot see a row named in
  the match but filled with the wrong value.
- **Item 3 — the corpus check sees additions, not only edits.** CI was `git diff --exit-code`, which is
  blind to untracked files, so a seed added by hand would have passed. `scripts/check-corpus-untouched.sh`
  checks both modifications and untracked files, as the RFC 4475 check always has. Failing-first
  verified: adding `t99-probe` fails with the file named and the reason.
- **Item 4 — no stale note survives, because `S-26` already landed.** It rewrote the RFC 2543 row to
  scope the fallback to the server half and deleted the spec's "Known deviation" note in the same
  commit. The item's own hedge ("coordinate with S-26") was the right call; what remained was verifying
  that nothing lagged, and nothing did.
- **Failing-first is by mutation for items 2 and 3** (the fourteenth variant is now a compile error,
  and the added corpus file is caught), and by *deletion* for item 1: a guard that cannot fire is
  proved wrong by showing the falsifiable arms remain.
- Implemented by the coordinator rather than an implementor: delegation unavailable on an org spend
  limit. This was the last story blocking alpha predicate 2.

## Notes
- **Every item here came from `X-19`'s independent review, not from its implementor.** The harness
  is good work and the review's verdict was PASS with nothing blocking; these are the seams a second
  reader found in a 1 622-line file that the first reader wrote.
- **The theme is the project's own recurring one.** `X-22` made the gate check itself against CI
  because a hand-maintained list drifted; `X-24` generated the pool-key doc from the type because
  prose drifted from the field. `TIMERS` and `MAX_LIVE_TRANSACTIONS` are the same shape one layer
  down — a hand-maintained constant guarded by an assert that cannot see the thing that changes.
- The suppression breadth the review also found (`KNOWN_DEFECTS` keys off `slot >=
  FIRST_LEGACY_SLOT` at :650, so it masks *all* `UnroutableResponse` for legacy slots rather than
  the one known cause) is **not** in this story's scope: `S-26` deletes the suppression outright
  when it fixes the defect, and `the_known_defect_suppression_is_still_needed_and_still_works`
  fails the moment it does. If `S-26` narrows rather than removes it, that comparison should be
  field-wise on the two keys and belongs there.
