---
id: X-121
title: Refuse a stale uplifted binary
pillar: Build
status: ready
priority: 10
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

- [ ] A process test that spawns `target/debug/sipx` verifies the binary's compiled feature set
      before asserting on behaviour, and names the mismatch when it differs.
- [ ] A failing-first test reproduces the trap: build without `--all-features`, then run an
      all-features process test, and prove the run reports a stale binary rather than an audio
      failure.
- [ ] The mechanism is cheap enough to run per test process — `sipx version --json` already reports
      the build, so prefer reading it over rebuilding.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `P-27`'s adjacent findings. Seven `sipx-cli` tests — `browser_audio_profile_*`,
  `dph_5/7/12`, `diagnostic_phone_opus_*` — failed **reproducibly** twice, and the cause was a 91 MB
  `target/debug/sipx` left by an earlier non-`--all-features` command. The later `--all-features` run
  did not re-uplift it, so the tests spawned a binary whose `sipx devices` answered "audio devices
  require a build with the device-audio feature". Removing the binary made the same command green
  (97 MB, 87 of 87 passing).

## Notes

- **This looks exactly like `X-118`'s load flakiness while being deterministic**, which is what makes
  it dangerous: the failure presents as "heard no audio at all", the same shape a real media
  regression has. Two gate failures earlier in this session were diagnosed as contention on a shared
  box; that diagnosis was supported by a clean re-run and by a sibling story hitting the same class,
  but this trap is a second explanation for the same symptom and neither was ruled out against the
  other.
- Worth doing before `X-118`, since a deterministic cause found first would change what `X-118` is
  looking for.
