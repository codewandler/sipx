---
id: X-57
title: Make `-vv` reach DEBUG, because it is documented, accepted and inert
pillar: Build
status: done
priority: 3
epic: conformance
areas: [sipx-cli]
note: found by X-53 — verbosity counts arguments starting with `-v`, so `-vv` counts as one and yields INFO; nothing on a call's path logs at INFO, so the documented flag produces no output at all
---

# Make `-vv` reach DEBUG, because it is documented, accepted and inert

## Goal
Make the verbosity flag do what the CLI's own help says it does, so an operator debugging a call
gets the output the flag promises instead of silence.

## Acceptance
- [x] **`-vv` reaches DEBUG.** `crates/sipx-cli/src/main.rs:74` counts verbosity as
      `args.iter().filter(|a| a.starts_with("-v")).count()`, so `-vv` is **one** matching argument
      and yields INFO; `-vvv` likewise. `init_logging` plainly intends `-v` = INFO and `-vv` = DEBUG,
      and `USAGE` documents both. Only the undocumented `-v -v` reaches DEBUG today.
- [x] **The failing-first test is the flag as documented.** Run a real call with `-vv` and assert
      DEBUG records appear on stderr. It must be red before the fix. `X-53` built the machinery this
      needs — `place_a_call` and `log_records` — and its test uses `-v -v` precisely because `-vv`
      could not satisfy the control; that test should move to `-vv` once this lands, and its
      deviation note removed.
- [x] **Decide what `-vvv` and beyond mean**, and whether repeated short flags should be counted by
      character rather than by argument. `-v -v -v` and `-vvv` must not disagree.
- [x] **The inertness is worth its own assertion.** Even parsed correctly, `-v` alone produces
      nothing on a successful call: the only two `tracing::info!` sites in the workspace
      (`sipx-ua/src/agent.rs`, `sipx-media/src/bridge.rs`) are off a call's path. Either say in
      `USAGE` what each level is actually good for, or add the INFO records a call's path should
      have — but do not leave a documented level that produces silence.

## Progress
- Filed 2026-07-30 by `X-53`, which found it while building a positive control for the
  verbose-logging test and could not use `-vv` to produce one.
- Closed 2026-07-31. Verbosity is counted by `v` **letter** over arguments that are a `-` followed
  by nothing but `v`s, so `-vv` and `-v -v` are one request and `-vvv` agrees with `-v -v -v`. The
  ladder saturates at DEBUG: the workspace contains no `trace!` call, so mapping a third `v` to
  TRACE would document a level whose output is identical to `-vv`'s — the defect restated rather
  than fixed. The old prefix match also counted `-V` and `--verbose` as verbosity; only a cluster
  of `v`s does now.
- The inert half is answered with records rather than with a narrower promise: `calling`,
  `answered` and `hung up` on the dialling side and `waiting for a call` on the answering side,
  all at INFO on stderr. `USAGE` now says what each level is for.
- The implementor also found that `log_records` could not see the binary's own records at all — a
  library record is targeted `sipx_call::call` while the binary's are `sipx::dial`, so the test
  written to look for them could not observe its own subject. Both spellings count now.
- The implementor was killed mid-run by an org monthly spend limit, so its work was rescued to
  `impl/X-57` and the **failing-first proof was established at integration**: with `src/` reverted
  to the base and the new tests in place, `verbose_logging_stays_off_stdout` fails with no DEBUG
  records (`-vv` capped at INFO) and `one_v_reports_the_call_on_both_ends_of_it` fails with no
  records at all (the inertness). Both empty lists, so the redness is the clause.
- It never reached the gate, so `cargo fmt` was never run on its diff: the `fmt` step was the one
  red step after the merge, over a call site that outgrew a line. Fixed forward in `f32c6fa`
  rather than reverting the merge. Reviewed by one context rather than two.

## Notes
- **`X-53` deliberately did not fix this.** It was scoped to a test and this is a behaviour change
  to `src/`; widening the diff would have made a test story into a CLI story. The right call, and
  this is the follow-up it earned.
- Reads with the vision's "testable from a shell": a verbosity flag that produces no output on the
  path an operator is debugging is the same shape as a capture that can only be switched on by
  editing code.
