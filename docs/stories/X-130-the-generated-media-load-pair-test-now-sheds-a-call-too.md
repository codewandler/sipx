---
id: X-130
title: The generated-media load pair test now sheds a call too
pillar: Quality
status: in-progress
priority: 2
design:
epic:
areas: [sipx-cli]
predicate:
announcement:
note: generated_media_load_pair_retains_the_rtp_workload runs --concurrency 2 against --max-active 2 and was observed rejecting one of four calls in a full cli suite run
---

# The generated-media load pair test now sheds a call too

## Goal

Give `generated_media_load_pair_retains_the_rtp_workload` the same admission headroom `X-126` gave
the signalling pair, so `connected == calls` is a claim about the workload rather than about
whether the responder's accept loop was scheduled in time.

## Acceptance

- [x] The fixture's responder ceiling clears the generator's concurrency, and the change carries
      the reason rather than a tuned number. `max_active = concurrency * 2` at
      `crates/sipx-cli/tests/cli.rs:5444`, with `X-126`'s derivation above it and the measurement
      showing the third slot is really used.
- [x] `active_high_water` is asserted as a range whose ends each carry a claim, as the signalling
      pair's is, rather than dropped. `crates/sipx-cli/tests/cli.rs:5526`.
- [x] The test passes repeatedly under the contention the full `sipx-cli` suite produces. 10 of 10
      at two CPU burners per core, and a full `cargo test -p sipx-cli --all-features` green.
- [ ] `./scripts/gate.py` green. Not run here — one gate per wave.

## Notes

`X-126` named this test as the remaining zero-headroom fixture and left it alone deliberately: it
had the same shape — `--concurrency 2` against `--max-active 2` — but only two slot handovers
against the signalling pair's twelve, and it held 20 of 20 under deliberate CPU load. It has now
been observed failing, so it is no longer latent.

Seen in `cargo test -p sipx-cli --all-features` on 2026-08-09 while implementing `X-129`, on a tree
whose only differences from `c405a60` were documentation, one added documentation test, and this
story file. The load summary, verbatim:

```json
"limits":{"call_duration_ms":1000,"calls":4,"cleanup_ms":40000,"concurrency":2,
          "duration_ms":null,"rate":20.0,"setup_timeout_ms":5000}
"outcomes":{"attempted":4,"connected":3,"failed":0,"peak_concurrency":2,"rejected":1,"timed_out":0}
"response_codes":{"200":3,"503":1}
```

Nothing failed and nothing timed out; one call was *rejected* with 503 at a ceiling equal to the
generator's concurrency. That is the mechanism `X-126` measured and `X-129` documented — the
responder frees a slot only after answering that dialog's BYE, while the generator frees its own on
receiving that 200 — and it is the responder behaving correctly, so the fix belongs in the fixture.

`X-126`'s reasoning for `2 ×` transfers directly: every one of the generator's concurrent slots can
free and place its replacement INVITE while the dialog it replaced is still between the responder
answering its BYE and the responder releasing its slot, and in the worst case all of them do so at
once. Its measured headroom is on a concurrency of 8, not 2, so nothing here should copy the number
9, 12 or 16 — carry the argument, not the observation.

Worth checking whether this test also belongs in `scripts/contention-proof.py`'s `SUBJECTS`, as
`X-126` added the signalling pair. That is what stops the headroom being quietly taken back.

## Progress

- 2026-08-09: **fixed, and the mechanism reproduced — but not through this fixture, which is the
  finding worth carrying forward.**

  **The fixture as written could not be made to fail.** At the merge base `2a91041`, with the
  ceiling still equal to the generator's concurrency:

  - 12 paired runs driven directly at two CPU burners per core: 0 shed.
  - 12 more at four burners per core: 0 shed.
  - 10 runs of `cargo test -p sipx-cli --all-features --test cli -- --exact
    generated_media_load_pair_retains_the_rtp_workload` at two burners per core: 10 passed.
  - one full `cargo test -p sipx-cli --all-features`: 106 passed, 0 failed.

  That is 46 runs of `X-126`'s technique with nothing to show, and it is consistent with `X-126`'s
  own note that this fixture held 20 of 20. So the honest answer to "reproduce before you fix" is
  that the *filed observation* stands and the *deliberate load* does not reach it.

  **The mechanism does reproduce, at this exact ceiling, once the fixture stops rolling the dice
  twice.** Four calls at concurrency 2 is two slot handovers. Lengthening the identical pair to 40
  calls — same binary, same mode, same rate, same duration, 38 handovers instead of 2 — under the
  same two-burners-per-core load, at the base:

  | ceiling | runs | calls shed | `active_high_water` |
  |---|---|---|---|
  | `--max-active 2` (equal) | 6 | 3, every one a 503 | 2 in every run |
  | `--max-active 4` (headroom) | 6 | 0 | 3 in three of six |

  Every rejection carried the filed signature exactly: a 503, `peak_concurrency` 2, nothing failed
  and nothing timed out. Three sheds over 6 × 38 handovers is about **1.3% per handover**, so the
  4-call fixture is red about one run in 38 — which is why it was seen once in the field and never
  under load here, and why 46 clean runs are not evidence that it is safe. It was passing by
  winning a race, not by not running one: `active_high_water` sat *exactly at the ceiling* in all
  46.

  The headroom half is the positive evidence, not just the absence of a failure:
  `active_high_water` reached **3** in three of six headroom runs. Three is a slot an equal ceiling
  does not have, and the call that took it is the call an equal ceiling refuses.

  **What changed.** `crates/sipx-cli/tests/cli.rs` only.

  - `max_active = concurrency * 2` (`cli.rs:5444`), with `concurrency` named as a variable and both
    `--concurrency` and `--max-active` rendered from it, so the two cannot drift apart again. The
    factor is `X-126`'s argument and not its number: every one of the generator's concurrent slots
    can free and place its replacement INVITE while the dialog it replaced is still between the
    responder answering its BYE and the responder releasing its slot, and in the worst case all of
    them do so at once. `X-126`'s 16 is not copied; on a concurrency of 2 the same argument gives 4.
  - `active_high_water` asserted as `concurrency..=max_active` (`cli.rs:5526`), which this test did
    not assert on at all before. At least `concurrency` proves the responder really carried the
    workload; no more than `max_active` proves admission control still held. The measurement above
    is what makes the range non-vacuous — the observed value moves inside it.
  - The one unscaled wall-clock bound in this test, `Duration::from_secs(15)` around the `load` run,
    is now `bound(Duration::from_secs(15))` and says what it bounds. `X-126` wrapped the signalling
    pair's twin and this one was missed; it is a second way this same fixture goes red under the
    contention row above.

  **Not added to `contention-proof.py`'s `SUBJECTS`, and that is a measurement rather than a
  shrug.** Its docstring requires a subject to have been seen red under this load, "the only
  property that makes a subject worth the minutes". Under this load this fixture is red about one
  run in 38, so it would pass identically with and without the fix while adding minutes to every
  proof run — a green that means nothing, which is the exact failure that docstring exists to
  prevent. The variant that *does* discriminate is the 40-call one measured above, and it is filed
  as **`X-135`** with both halves of the measurement, since adding a long `#[ignore]`d fixture is
  new test surface rather than this story's Acceptance.

  So what holds the headroom here is the comment carrying the argument and the `active_high_water`
  range, not a proof run. Stated plainly because the story asked the question.

  **After.** 10 of 10 runs of the fixture at two burners per core; full
  `cargo test -p sipx-cli --all-features` green (106 passed, 0 failed, 1 ignored). Green in this
  worktree: `cargo clippy -p sipx-cli --all-targets --all-features --no-deps -- -D warnings`,
  `cargo fmt --all`, `check-fixed-sleep.py --check`, `contention-proof.py --check`,
  `test-contention-proof.py` (11 tests), `check-provenance.sh`. `gate.py` deliberately not run —
  one gate per wave — so that row is left unticked.

  **Owed CHANGELOG sentence** (fenced file, not edited here):

  > The generated-media load-pair test gives its responder twice the generator's concurrency, so
  > `connected == calls` is a claim about the workload rather than about whether the responder's
  > accept loop was scheduled before the replacement INVITE arrived.

  **`docs/stories/README.md` needs regenerating** for this story's status and for the new `X-135`;
  the board is the coordinator's file and was not touched here.
