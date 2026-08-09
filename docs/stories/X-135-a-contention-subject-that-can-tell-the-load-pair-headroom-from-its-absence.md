---
id: X-135
title: A contention subject that can tell the load-pair headroom from its absence
pillar: Quality
status: done
priority: 3
design:
epic:
areas: [sipx-cli]
predicate:
announcement:
note: the generated-media load pair rolls its handover race twice per run, so contention-proof.py cannot make it red and adding it to SUBJECTS would cost minutes to prove nothing
---

# A contention subject that can tell the load-pair headroom from its absence

## Goal

`scripts/contention-proof.py` should have a subject that goes red when a load pair's responder
ceiling is equal to the generator's concurrency and green when it clears it — so the headroom
`X-126` and `X-130` gave those fixtures is held by a measurement rather than only by a comment.

## Acceptance

- [x] A subject exists that was observed red under `contention-proof.py`'s own load with an equal
      ceiling, and green under the same load with the headroom. Both halves measured, in the same
      session, and the run counts recorded.
      `contention_subject_a_long_generated_media_pair_admits_every_call`
      (`crates/sipx-cli/tests/cli.rs:5654`), 6 of 6 red and 0 of 6 green — table in `## Progress`.
- [x] Adding it does not lengthen an ordinary `cargo test -p sipx-cli --all-features` run — a long
      run belongs to the proof, not to the suite. `#[ignore]`d; the suite reports `107 passed;
      0 failed; 2 ignored` for `tests/cli.rs` where it reported one ignored before.
- [x] The minutes it adds to a full proof run are stated, since `contention-proof.py` is already
      not a gate step for that reason. About 27 seconds, beside the `SUBJECTS` entry that names it.
- [x] `./scripts/gate.py` green.

## Notes

`X-130` measured why the obvious move does not work. `generated_media_load_pair_retains_the_rtp_workload`
places 4 calls at concurrency 2, which is **two** slot handovers — two chances per run for the
replacement INVITE to arrive inside the window where a retiring dialog is counted by both ends. At
two CPU burners per core the per-handover shed rate is about 1.3%, so the fixture as written is red
roughly one run in 38. Measured at the merge base with an equal ceiling: 46 runs of the fixture
itself shed nothing, and a full `cargo test -p sipx-cli --all-features` shed nothing either.

So adding this fixture to `SUBJECTS` would add minutes to every proof run for a check that passes
identically with and without the fix. That is the "green for the same reason an empty test suite is"
failure the script's own docstring exists to prevent, and it is why `X-130` did not add it.

What does discriminate, measured in the same session with the same binary and load — the identical
pair lengthened to 40 calls, 38 handovers instead of 2, nothing else changed:

| ceiling | runs | calls shed | `active_high_water` |
|---|---|---|---|
| `--max-active 2` (equal) | 6 | 3, all 503 | 2 in every run |
| `--max-active 4` (headroom) | 6 | 0 | 3 in three of six |

Both halves are there: red without the headroom, green with it, and the `active_high_water` of 3 is
the retiring dialog that an equal ceiling refuses, seen directly rather than inferred.

The shape that fits is a longer variant of the pair, `#[ignore]`d so it stays out of the ordinary
suite the way `contention_control_an_unbounded_wait_is_still_reported` does, and named in `SUBJECTS`
with `ignored=True` — `Subject.command` already takes that argument for the control. A 40-call pair
at concurrency 2 is about 20 seconds idle and longer under load, which is the cost to state.

Worth deciding as part of it whether the signalling pair wants the same treatment. `X-126`'s fixture
has twelve handovers rather than two and *was* reproducible directly, so it is already a real
subject; this story is about the fixtures whose race is too rare to catch as written.

## Progress

- 2026-08-09: **done, and the shape the Notes proposed was measured and then rejected on the
  evidence.** A `SUBJECTS` entry rather than a separate opt-in target, because the discrimination
  turned out to be near-certain rather than a coin toss — but only after the length was chosen from
  a measurement instead of from the fixture it replaces.

  **The 40-call variant the Notes proposed discriminates half the time, which is not enough.**
  Reproduced first, exactly as `X-130` left it — the identical pair at concurrency 2, lengthened to
  40 calls, under `contention-proof.py`'s own load at two CPU burners per core, run through the
  script's own `Subject.command()`:

  | ceiling | runs | red | calls shed | `active_high_water` |
  |---|---|---|---|---|
  | `--max-active 4` (headroom) | 6 | 0 | 0 | — |
  | `--max-active 2` (equal) | 6 | **3** | 2, 4 and 1 — every one a 503 | 2 in every run |

  That confirms `X-130` to the run (it saw 3 sheds over 6 runs too) and it is a usable subject by the
  letter of the Acceptance. It is a poor one by the intent: a proof run would miss a removed headroom
  half the time, for 21 seconds of every proof run. Paying wall clock for a coin toss is the second
  cost this story was told to weigh, and it fails it.

  **The lever is handovers, and handovers are bought with concurrency rather than with time.** A shed
  is one overlap between a retiring dialog and its replacement, so the run's chance of going red is
  set by how many handovers it performs, while its wall clock is set by `calls / concurrency`
  one-second calls. Handovers per second is therefore `concurrency`, and raising that buys the
  discrimination at a fixed price. Same pair, same mode, same rate, same one-second calls, at the
  signalling pair's concurrency of 8 with 200 calls — 192 handovers in 26 seconds instead of 38 in 21:

  | ceiling | runs | red | calls shed | `active_high_water` |
  |---|---|---|---|---|
  | `responder_ceiling(8)` = 16 (headroom) | 6 | **0** | 0 | 9, in 4 of 4 sampled separately |
  | `--max-active 8` (equal) | 6 | **6** | 6, 8, 10, 13, 11, 8 — every one a 503 | 8 in every run |

  Both halves, same session, same binary, same load. Every red carried the filed signature exactly:
  a 503, `peak_concurrency` 8, `failed` 0 and `timed_out` 0 — the responder honouring its contract at
  a ceiling with no room for a retiring dialog. The leanest red run still shed six calls, so this is
  not a subject that goes red on a good day; and the green half's `active_high_water` of **9** is the
  positive evidence rather than the absence of a failure, because a ninth simultaneous dialog is
  precisely the call an equal ceiling refuses.

  **What holds the headroom is a shared derivation, not a longer copy of it.** The subject asserts
  its own `--max-active`, so a per-fixture copy of `concurrency * 2` would have left it defending
  only itself. `responder_ceiling` (`crates/sipx-cli/tests/cli.rs:5241`) is now the one place the
  factor lives, and the signalling pair, the generated-media pair and the subject all read it — so
  the edit that takes the headroom back is the edit the subject goes red for. That is what turns
  `X-126`'s and `X-130`'s comments into a measurement.

  **What changed.**

  - `crates/sipx-cli/tests/cli.rs`: `responder_ceiling` with the derivation (`:5241`); both existing
    pairs now call it instead of spelling `concurrency * 2` out; and
    `contention_subject_a_long_generated_media_pair_admits_every_call` (`:5659`), `#[ignore]`d, with
    both halves of the table above in its doc comment.
  - `scripts/contention-proof.py`: `ignored` is a field of `Subject` rather than an argument to
    `command()`. That is the load-bearing part of the change, not tidying — `main` ran subjects
    without it, so an `#[ignore]`d subject would have been asked for by name, skipped, reported as
    zero tests run and exited 0. Passed as a subject that held. `--check` now verifies the attribute
    against the source **in both directions** for every subject, since the same false green is
    reachable from either side, and it reads the attribute block immediately above the `fn` rather
    than a fixed character window — the old window was wide enough to see the control's `#[ignore]`
    from its neighbour twelve lines below.
  - `scripts/test-contention-proof.py`: the failing-first test. Three cases for the ignored subject —
    that the tree has one and that it is invoked with `--ignored`, and that `--check` reports each
    direction of the drift.

  **The cost, stated without flattering it.** The subject adds about 27 seconds to every run of
  `contention-proof.py`. On a warm build the whole proof measured **38 seconds**, so this one subject
  is most of the run — that is the honest framing, not "27 seconds on top of minutes". It buys the
  only subject in the list whose red is near-certain rather than a 40% chance. It costs an ordinary
  `cargo test -p sipx-cli --all-features` nothing, which is mechanical rather than argued:
  `tests/cli.rs` reports `107 passed; 0 failed; 2 ignored` where it reported one ignored before.

  **The signalling pair needs no separate treatment**, which the Notes left open. It is already a
  subject, `X-126` reproduced it directly at 4 of 10 runs, and it now shares `responder_ceiling` with
  the new subject — so the derivation it depends on is held by a 6-of-6 measurement rather than by
  its own 4-of-10 one. No follow-up story was filed.

  **After.** In this worktree: `cargo test -p sipx-cli --all-features` (120/6/107/4/2/6 passed, 0
  failed, 2 ignored in `tests/cli.rs`), `cargo clippy -p sipx-cli --all-targets --all-features
  --no-deps -- -D warnings`, `cargo fmt --all`, `check-fixed-sleep.py --check` (44 of 44 classified),
  `contention-proof.py --check` (5 subjects and 1 control resolved), `test-contention-proof.py`
  (14 tests), `check-provenance.sh` (clean), and one end-to-end `./scripts/contention-proof.py` which
  reported **proven** with the control red in the same run. `gate.py` deliberately not run — one gate
  per wave — so that row is left unticked.

  **Owed CHANGELOG sentence** (fenced file, not edited here):

  > `scripts/contention-proof.py` gained a subject that can tell a load pair's admission headroom
  > from its absence — a 200-call generated-media pair, kept out of the ordinary suite, measured 6 of
  > 6 red at a responder ceiling equal to the generator's concurrency and 0 of 6 with the headroom —
  > and the ceiling both pairs use is now one derivation that subject defends rather than a comment
  > in each of them.

  **`docs/stories/README.md` needs regenerating** for this story's status; the board is the
  coordinator's file and was not touched here.

- 2026-08-09: closed at the `1.0.0-rc.16` boundary, against the wave gate run on this tree.
