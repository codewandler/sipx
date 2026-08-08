---
id: X-124
title: Stop the gate leaving a default-feature binary
pillar: Build
status: done
priority:
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

- [x] `scripts/check-cli-reference.py` no longer leaves a default-feature `target/debug/sipx`
      behind, either by building what the tests need or by building somewhere the tests do not
      spawn from.
- [x] Whichever is chosen, the public CLI reference is still held against a stated build, and the
      story says which build and why — repointing it at `--all-features` changes what the reference
      documents, which is a decision rather than a detail.
- [x] A failing-first test proves a gate run does not leave a binary whose features differ from the
      test suite's.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `X-121`'s adjacent findings and verified in the tree.
  `scripts/check-cli-reference.py:231` runs `cargo build -p sipx-cli --quiet` — a **default**-feature
  build — and `scripts/gate.py` orders `cli reference` (line 297) **after** the all-features `test`
  step (line 292). So every gate run ends by leaving a `sipx` binary without `device-audio`, `dtls`
  or `opus`, which the next run's process tests will spawn.

- 2026-08-08: implemented. **Decision: keep the reference on a default-feature build, and move it
  off the shared target directory** into `target/cli-reference`. The alternative — repointing it at
  `--all-features` — would have changed what the published CLI reference documents: a reader who
  installs `sipx-cli` without extra features gets the default surface, and the reference is that
  reader's page. Moving the output keeps both truths: the reference is still held against the build
  it describes, and the shared binary is left exactly as the tests built it. Cost of the choice: one
  extra build directory, and the reference build no longer shares the test build's cache.
  Proved by observation rather than argument: `sipx version --json` reports
  `["device-audio","dtls","opus"]` both before and after the check runs, where the shared binary
  previously came back with `[]`.
- 2026-08-08: the guard tests were themselves nearly vacuous — first written after `unittest.main()`,
  where they could never run, and against the wrong module alias. Both fixed, and the failing-first
  proof is a real reversion: with the old `build_binary` in place the suite fails on the missing
  `--target-dir`, and passes once restored.

## Notes

- **This is the strongest lead for `X-118`.** Two gate failures in this project were diagnosed as
  contention on a shared box; the evidence for that was a clean re-run and a sibling story hitting
  the same class, and this deterministic cause was never ruled out against it. `X-121`'s guard now
  makes the two distinguishable — a wrong binary announces itself instead of failing as "heard no
  audio at all".
- `X-121` deliberately did not repoint the build, because it changes which build the published CLI
  reference is held against. That is this story's decision to make.
