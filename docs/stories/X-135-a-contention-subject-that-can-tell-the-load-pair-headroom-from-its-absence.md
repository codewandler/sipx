---
id: X-135
title: A contention subject that can tell the load-pair headroom from its absence
pillar: Quality
status: backlog
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

- [ ] A subject exists that was observed red under `contention-proof.py`'s own load with an equal
      ceiling, and green under the same load with the headroom. Both halves measured, in the same
      session, and the run counts recorded.
- [ ] Adding it does not lengthen an ordinary `cargo test -p sipx-cli --all-features` run — a long
      run belongs to the proof, not to the suite.
- [ ] The minutes it adds to a full proof run are stated, since `contention-proof.py` is already
      not a gate step for that reason.
- [ ] `./scripts/gate.py` green.

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
