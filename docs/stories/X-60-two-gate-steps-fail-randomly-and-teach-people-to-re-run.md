---
id: X-60
title: Two gate steps fail randomly, which teaches people to re-run the gate
pillar: Build
status: done
priority: 3
epic: conformance
areas: [sipx-cli, scripts, ci]
predicate: 3
note: observed twice in one run — `a_recording_cut_short_by_the_cap_is_kept` failed under full-workspace load and passed alone and on re-run, and the docs-site dead-anchor probe failed with `Detected unsettled top-level await` and passed on re-run in an unchanged tree
---

# Two gate steps fail randomly, which teaches people to re-run the gate

## Goal
Make a red gate mean something, by removing the two steps that currently go red without a defect
behind them.

## Acceptance
- [x] **`a_recording_cut_short_by_the_cap_is_kept` is made load-independent, or made to say why it
      cannot be.** `crates/sipx-cli/tests/recording_bounds.rs:150`. Observed failing inside
      `cargo test --workspace --all-features` and passing both alone and on a full re-run of the same
      tree, twice over. The panic is `.expect("the result line")` on an `Option`, **not** the 40-second
      timeout directly above it in the same expression — so the answerer's stdout closed with no line
      at all rather than being slow. That is an early exit, and the likely cause is `--wait 20`
      expiring before the caller reaches it when the machine is running thirty other suites. Find the
      exit path and make the test wait for the thing rather than for a duration, or state at the line
      which of the four questions the duration answers (`check-fixed-sleep.py`'s own words).
- [x] **A failure names which exit path it took.** The test cannot currently distinguish "the
      answerer expired before the call" from "the answerer answered and printed nothing", and those
      have different causes. The assertion should say which it saw — an `Option` that is `None` is
      the least informative form a real failure could take.
- [x] **The docs-site dead-anchor probe stops failing at random.** `scripts/build-docs.sh` — the probe
      that asserts a link to a non-existent id fails the build. Observed failing with
      docusaurus's `Warning: Detected unsettled top-level await` while the real site build in the
      *same* run succeeded, and passing on a standalone re-run of an unchanged tree. Either the probe
      is made deterministic or the flake's cause is written down where the next reader meets it.
- [x] **A re-run must not be the documented remedy.** Whatever is done, it is not "run it again".
      `AGENTS.md` makes a green gate the precondition for calling a story done, so a step that is red
      one run in several converts that precondition into a coin toss.

## Progress
- Filed 2026-07-31, from two independent observations in a single coordinator run. The recording
  flake was hit while gating `X-54`'s merge — the gate reported `1 of 25 steps failed`, the story was
  nearly reverted for it, and the failure did not reproduce: the same tree passed the isolated test
  and then passed all thirty suites under `--workspace --all-features`. The docs-site flake was hit
  and reported independently by `X-54`'s implementor in its own worktree.
- Implemented 2026-08-03. The recording fixture now binds its caller before starting the answerer's
  wait-for-call clock. Its remaining five-minute wait is explicitly a bound on failure, orders of
  magnitude above the honest loopback setup, and EOF reports whether the wait expired, the process
  exited cleanly without a result, a signal killed it, or another command failure occurred, with
  exit status and stderr. A forced `--wait 0` reproduced the observed shape exactly: only the
  listening line on stdout, then EOF, exit 5 and the timeout report on stderr. The cap regression
  passed in isolation after the change.
- The dead-anchor guard now calls the installed link handler directly with the real site config and
  a synthetic page/anchor graph. The public site still receives one full production build, while the
  guard no longer starts the second compiler/worker lifecycle that sometimes ended with unsettled
  top-level await. The complete docs step, its reversal tests, twenty consecutive direct probes, and
  the fixed-sleep check pass.
- A combined-tree integration run found the same class one test binary inward:
  `dial_plays_a_file_and_records_the_far_end` ran beside thirty other asynchronous CLI scenarios,
  each spawning real processes, and its caller's media worker did not run before the six-second call
  duration expired. The answerer remained healthy for its full ten-second call and reported zero
  received packets, so this was not the old recorder-start idle window. The CLI integration binary
  now admits one process scenario at a time through an asynchronous semaphore. Caller and answerer
  inside a scenario still run concurrently; the next scenario begins when the preceding processes
  exit, with no sleep or widened duration standing in for capacity.

## Notes
- **This is `X-34`'s doctrine again, from the other side.** `X-34` made the gate refuse to report
  rather than report something it could not stand behind, because a result that cannot be believed is
  worse than no result. A step that is red without a defect is the same failure wearing the opposite
  sign: it trains everyone to re-run, and the re-run habit is what lets a *real* red be dismissed.
- **The cost was nearly a wrong revert.** The impl-coord rule is that a red gate after a merge means
  revert that merge, and exactly one merge had changed. Only re-running caught that the merge was
  innocent. Next time the coordinator may be less lucky, or less patient.
- Reads with `X-28`, `X-29` and `X-40` — all three were a duration standing in for a happens-before,
  which is what `check-fixed-sleep.py` now guards. This one is a duration in the *fixture setup*
  (`--wait 20`) rather than in an assertion, which is why the guard does not see it.
