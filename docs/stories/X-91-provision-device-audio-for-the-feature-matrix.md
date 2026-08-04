---
id: X-91
title: Provision device audio for the feature matrix
pillar: Build
status: done
priority: 1
design: docs/specs/release-rehearsal.md
epic: conformance
areas: [release, ci, sipx-cli]
predicate: 3
announcement:
note: beta-1 blocker · exact-sha CI enabled device audio without its Linux build prerequisite
---

# Provision device audio for the feature matrix

## Goal

Make the feature-matrix job test the optional feature graph it names instead of failing because its
Linux runner lacks the native library required by the device-audio feature.

## Acceptance

- [x] Exact-sha GitHub run `30904773713`, job `91977186019`, records the failing-first case: every
      feature combination before `sipx-cli device-audio` passes, then both device-audio variants
      fail because `alsa.pc` is unavailable.
- [x] The feature-matrix job installs only the Linux device-audio build prerequisites before it
      runs the unchanged feature checker.
- [x] Gate-consistency still accounts for every CI command, the complete local gate passes, and a
      new exact-sha GitHub run completes with the feature matrix and Pages deployment green.

## Progress

- The complete 32-step local gate passed at `0ae603d`, including every feature combination, because
  this development host already has the ALSA development package. The CI failure is therefore a
  runner-provisioning defect, not a feature-graph or Rust compilation defect.
- The complete gate passed again at `f30ffd2`. Exact-sha GitHub run `30905359518` completed green;
  feature-matrix job `91979064074` and Pages deployment job `91979625126` both passed.

## Notes

- The feature checker itself remains the single local/CI command. Installing `libasound2-dev` and
  `pkg-config` is runner provisioning, which the gate's drift checker classifies separately from
  verification commands.
