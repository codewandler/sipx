---
id: X-96
title: Arm the browser cleanup proof after process start
pillar: Build
status: in-progress
priority: 1
design: docs/specs/browser-audio-proof.md
epic: conformance
areas: [ci, browser-audio]
predicate: 3
announcement:
note: the full beta.4 gate exposed a total-timeout race before the lifecycle probe wrote its PID evidence
---

# Arm the browser cleanup proof after process start

## Goal

Make the browser-proof lifecycle test exercise cleanup after a process group actually starts,
instead of racing the harness's complete-operation timeout against process admission.

## Acceptance

- [ ] The missing-PID-file failure is reproduced before the correction and retained as evidence.
- [ ] The lifecycle test uses the role timeout that begins after group admission, while the larger
      complete-proof timeout remains only an outer failure bound.
- [ ] A bounded repeated run and the complete gate pass without weakening process-group cleanup.

## Progress

- Filed from the complete beta.4 gate. The failure reproduced on iteration 2 of a bounded repeated
  harness run: `test_timeout_kills_the_entire_process_group` tried to read `pids`, but the one-second
  complete-proof deadline had killed the inner owner before its probe necessarily wrote that file.

## Notes

- `tests/browser-audio/run.sh` starts the complete-proof deadline outside the internal process owner;
  the role deadline starts only after `start_group` has admitted and recorded the child group.
