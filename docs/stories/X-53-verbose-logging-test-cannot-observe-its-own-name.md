---
id: X-53
title: Make `verbose_logging_stays_off_stdout` able to observe logging at all
pillar: Build
status: ready
priority: 4
epic: conformance
areas: [sipx-cli]
note: found by X-45's sweep — the test runs a command refused as a usage error before any socket binds, so no log record is ever produced and the assertion would hold identically if logging went to stdout
---

# Make `verbose_logging_stays_off_stdout` able to observe logging at all

## Goal
Give the second instance of `X-45`'s defect the same fix: a test named for a property it cannot
currently observe should be able to fail when that property is violated.

## Acceptance
- [ ] **Demonstrate the blindness before fixing it.** `crates/sipx-cli/tests/cli.rs:647` runs
      `dial sip:bob@example.com --json -vv`, which is refused as a usage error before any socket is
      bound, so the CLI emits no log events at all. `assert!(stdout.is_empty())` therefore holds
      whether logging goes to stderr or to stdout. Sabotage `init_logging` to write to stdout, show
      the current test still green, and record that as the failing-first evidence — the same shape
      `X-45` used.
- [ ] **The rewritten test runs a command that actually logs**, at a verbosity that produces records,
      and asserts stdout carries none of them while stderr does. A test that asserts only the
      absence, with nothing proving records existed to be misplaced, has the defect it is replacing.
- [ ] **The exit code is asserted.** The current test does not check it, so nothing pins the code
      path it silently depends on — a change that made the invocation succeed instead of being
      refused would move the test to a different branch without failing it.
- [ ] Failing-first: the sabotaged build must make the rewritten test red, and the restored build
      must make it green. Both runs quoted.

## Progress
- Filed 2026-07-30 by `X-45`'s implementor, which swept all five files under `crates/sipx-cli/tests/`
  for this shape and found exactly two genuine instances. It fixed `no_capture_flag_means_no_file`
  and left this one rather than widen its diff, which was the right call.

## Notes
- **Reads with `X-45`** and `X-36` — the same defect class three times now: an assertion about the
  absence of a side effect, in a test that never runs the code that would produce it. `X-44` is the
  standing proposal to catch a related class mechanically; worth asking there whether this one can be
  caught by a guard rather than by a sweep.
- `X-45`'s sweep found everything else under `crates/sipx-cli/tests/` defensible, and its story
  records why for each. Start from that list rather than re-sweeping.
