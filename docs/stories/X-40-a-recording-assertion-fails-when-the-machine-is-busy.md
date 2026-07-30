---
id: X-40
title: Stop asserting on recorded audio without waiting for it
pillar: Build
status: ready
priority: 3
design: docs/designs/media.md
epic: conformance
areas: [sipx-cli]
note: alpha predicate 3, third instance after X-28 and X-29 — `dial_plays_a_file_and_records_the_far_end` waits for the call and then asserts on a real-time side effect, so under load it reads a valid WAV with zero samples; observed once, not reproducible in isolation
---

# Stop asserting on recorded audio without waiting for it

## Goal
Make `crates/sipx-cli/tests/cli.rs:296` fail only when media did not flow, rather than when the machine
was too busy to carry it in time.

## Acceptance
- [ ] **The recording is waited *for*, with a deadline, not slept past.** The test currently waits for
      the *call* to complete — bounded, 40s/25s timeouts — and then asserts on a real-time side effect:
      that RTP audio accumulated during a 6-second call (`!heard.samples.is_empty()`, then
      `peak > 6000`). Call success does not imply media flowed. Under CPU starvation the media path can
      deliver nothing and the file on disk is a valid WAV with zero samples, which is exactly what was
      observed: `panicked at crates/sipx-cli/tests/cli.rs:296:5: the callee recorded nothing`. Poll for
      a non-empty frame under a deadline, as `X-29` did for the DNS cache — load can then only lengthen
      the wait, and "never arrived" becomes a failure that says so.
- [ ] **The answerer's exit status is asserted, not discarded.** `cli.rs:291` does
      `let _ = answerer.wait().await;`, so "the callee recorded nothing" cannot distinguish silent
      media from an answerer that crashed. This is a diagnosis defect in its own right: it makes the
      failure it does report ambiguous.
- [ ] **The whole file is swept for the same shape, not just line 296.** `X-28` cleared the media path
      and `X-29` cleared `sipx-call` and `sipx-transport`; this is the third instance, so the pattern is
      established rather than incidental. Any other assertion in `crates/sipx-cli/tests/` that reads a
      real-time side effect after waiting on something else is the same defect.
- [ ] **The assertion still fails when media genuinely does not flow.** A deadline-polling test that
      cannot detect a silent media path is worse than a flaky one. Break the media path deliberately and
      show the test failing — `X-36` found a test that was green and could not detect the reversal of
      the invariant it was named for, and that is the failure mode to avoid here.
- [ ] Failing-first test: this defect resists a conventional failing-first test, because the failure is
      load-dependent and was not reproducible in isolation (3/3 passes alone, 15/15 twice for the full
      binary, 1/1 at the merge base). Say how the fix is pinned instead — a test that fails when the
      recording never becomes non-empty is the honest substitute, and it must be shown red before the
      fix.

## Notes
- **Observed once and not reproduced**, which is recorded here deliberately rather than treated as a
  reason to wait. It failed during a gate run while three other worktrees were compiling and the disk
  was ~98% full. The reporting implementor could not reproduce it in isolation and said so plainly
  instead of re-running to green — that is why there is a story rather than a silent retry.
- **The structural argument does not depend on reproducing it.** The test asserts on a real-time
  side effect after waiting for a different event. That is unsound under load whether or not it has
  been caught, and it is the same shape `X-28` and `X-29` closed.
- **Why this is priority 3.** Alpha predicate 3 is "a red gate means a defect. No test in the workspace
  fails because the machine was busy", and it is documented as **load-bearing for the other six**,
  because every predicate is asserted by the gate. A gate that cries wolf invalidates all of them, and
  the pressure it creates — learning to re-run a red step — is the habit the predicate exists to
  prevent. `X-39` is the same predicate failing from the other direction, where a step is red for a
  reason that is not a defect at all.
- Reads with `X-28` (media path), `X-29` (the rest, and the deadline-polling shape to copy) and `X-36`
  (a green test that asserted nothing).
