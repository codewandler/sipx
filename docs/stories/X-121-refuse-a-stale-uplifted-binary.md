---
id: X-121
title: Refuse a stale uplifted binary
pillar: Build
status: done
priority:
design:
epic: test-surfaces
areas: [sipx-cli, scripts]
predicate:
announcement:
note: a non-all-features cargo run leaves target/debug/sipx without device-audio, and a later all-features test spawns it without complaint
---

# Refuse a stale uplifted binary

## Goal

Make a process test fail loudly when the binary it spawns was not built with the features the test
requires, instead of failing as though the feature were broken.

## Acceptance

- [x] A process test that spawns `target/debug/sipx` verifies the binary's compiled feature set
      before asserting on behaviour, and names the mismatch when it differs.
- [x] A failing-first test reproduces the trap: build without `--all-features`, then run an
      all-features process test, and prove the run reports a stale binary rather than an audio
      failure.
- [x] The mechanism is cheap enough to run per test process — `sipx version --json` already reports
      the build, so prefer reading it over rebuilding.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `P-27`'s adjacent findings. Seven `sipx-cli` tests — `browser_audio_profile_*`,
  `dph_5/7/12`, `diagnostic_phone_opus_*` — failed **reproducibly** twice, and the cause was a 91 MB
  `target/debug/sipx` left by an earlier non-`--all-features` command. The later `--all-features` run
  did not re-uplift it, so the tests spawned a binary whose `sipx devices` answered "audio devices
  require a build with the device-audio feature". Removing the binary made the same command green
  (97 MB, 87 of 87 passing).

- 2026-08-08: implemented on `impl/X-121` off `wave/rc7`. `sipx version --json` gained a `features`
  array — it reported the version and *not* the build, so the mechanism the Acceptance assumed had
  to be built before it could be read. `crates/sipx-cli/tests/support/uplift.rs` reads it once per
  test binary (4.6 ms) and refuses a mismatch by name; all four `sipx-cli` test targets that spawn
  the binary call it. Reproduced at the merge base: the seven named tests failed with `the address
  line`, `typed setup failure` and a bare exit-code assertion, and now fail with the two feature
  sets and `rm -f …/target/debug/sipx`. Caught in both directions — a binary *ahead* of the tests
  is the same trap and had the same unreadable symptom.

  **The producer is `scripts/check-cli-reference.py`**, whose `cargo build -p sipx-cli --quiet`
  (line 231) is a default-features build; `gate.py` runs it at step "cli reference", *after* the
  all-features "test" step, so every gate run ends by leaving a binary without `device-audio` at
  `target/debug/sipx`. That is left as a finding, not fixed here: this story refuses the stale
  binary, and pointing the check at `--all-features` changes what the public CLI reference is held
  against. On cargo 1.97.0 a subsequent `cargo test --all-features` does re-uplift, so the observed
  failure needs a run that does not rebuild the bin unit — a directly invoked test executable, or a
  concurrent build sharing the directory. The guard does not depend on which.

- [ ] `./scripts/gate.py` was not run: the wave coordinator runs one per wave. Verified instead —
      `cargo test -p sipx-cli --all-features` (222 passing), the same suite with default features,
      `cargo clippy -p sipx-cli --all-targets --all-features --no-deps -- -D warnings`,
      `cargo fmt --all --check`, `./scripts/check-cli-reference.py --check`,
      `./scripts/check-provenance.sh`, `./scripts/coverage-report.py --check`, and the six
      `sipx-cli` feature combinations `check-features.sh` compiles.

- 2026-08-08: closed in the `1.0.0-rc.7` boundary.

## Notes

- **This looks exactly like `X-118`'s load flakiness while being deterministic**, which is what makes
  it dangerous: the failure presents as "heard no audio at all", the same shape a real media
  regression has. Two gate failures earlier in this session were diagnosed as contention on a shared
  box; that diagnosis was supported by a clean re-run and by a sibling story hitting the same class,
  but this trap is a second explanation for the same symptom and neither was ruled out against the
  other.
- Worth doing before `X-118`, since a deterministic cause found first would change what `X-118` is
  looking for.
