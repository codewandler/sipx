---
id: X-124
title: Stop the gate leaving a default-feature binary
pillar: Build
status: ready
priority: 2
design:
epic: test-surfaces
areas: [scripts, ci]
predicate:
announcement:
note: check-cli-reference builds sipx-cli with default features after the all-features test step, so every gate run ends by leaving a binary the next run's tests will spawn
---

# Stop the gate leaving a default-feature binary

## Goal

Stop the gate from ending each run with a `target/debug/sipx` built without the features its own
process tests require.

## Acceptance

- [ ] `scripts/check-cli-reference.py` no longer leaves a default-feature `target/debug/sipx`
      behind, either by building what the tests need or by building somewhere the tests do not
      spawn from.
- [ ] Whichever is chosen, the public CLI reference is still held against a stated build, and the
      story says which build and why — repointing it at `--all-features` changes what the reference
      documents, which is a decision rather than a detail.
- [ ] A failing-first test proves a gate run does not leave a binary whose features differ from the
      test suite's.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `X-121`'s adjacent findings and verified in the tree.
  `scripts/check-cli-reference.py:231` runs `cargo build -p sipx-cli --quiet` — a **default**-feature
  build — and `scripts/gate.py` orders `cli reference` (line 297) **after** the all-features `test`
  step (line 292). So every gate run ends by leaving a `sipx` binary without `device-audio`, `dtls`
  or `opus`, which the next run's process tests will spawn.

## Notes

- **This is the strongest lead for `X-118`.** Two gate failures in this project were diagnosed as
  contention on a shared box; the evidence for that was a clean re-run and a sibling story hitting
  the same class, and this deterministic cause was never ruled out against it. `X-121`'s guard now
  makes the two distinguishable — a wrong binary announces itself instead of failing as "heard no
  audio at all".
- `X-121` deliberately did not repoint the build, because it changes which build the published CLI
  reference is held against. That is this story's decision to make.
