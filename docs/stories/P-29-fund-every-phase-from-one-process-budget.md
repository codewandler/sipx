---
id: P-29
title: Fund every phase from one process budget
pillar: Phone
status: done
priority: 33
design:
epic: diagnostic-automation
areas: [sipx-cli]
predicate:
announcement:
note: P-26 bounded resolution BY the deadline rather than subtracting it FROM the deadline, so the worst case is two phases of the stated value
---

# Fund every phase from one process budget

## Goal

Make a command's stated deadline the budget for the whole process, not a value each phase gets a
fresh copy of. Today `dial --timeout 5` can spend five seconds resolving and then five more on the
invitation.

## Acceptance

- [x] `dial`, `load` and `scenario` fund each phase from the remaining budget, the way `register`
      already does since `P-25`, so the stated deadline bounds the process rather than each phase.
- [x] A failing-first test proves the worst case against a slow name plus a non-answering peer lands
      near the stated deadline, not near a multiple of it.
- [x] `docs/specs/diagnostic-phone.md` §3.2's normative "starts when the initial INVITE is handed to
      the endpoint" is resolved deliberately — either restated, or kept with the accounting made
      explicit — and the decision is recorded rather than implied.
- [x] The published `invitation_limit_ms` and `invitation_elapsed_ms` fields keep a stated meaning
      across the change, with a `CHANGELOG.md` entry if it moves. Tests assert those at exact values
      today.
- [x] `dial`'s reference no longer describes the process bound as the sum of two named phases; with
      resolution bounded, the honest worst case is three.
- [x] `load` consults `--duration` as well as `--timeout` when bounding resolution: a run with
      `--duration 2 --timeout 20` can currently spend eight seconds resolving.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `P-26`'s first deviation, which stopped here deliberately and said so.
  Bounding resolution by the deadline satisfied `P-26`'s acceptance; subtracting it would have
  changed the meaning of two published fields and rewritten a normative spec sentence, neither of
  which that story sanctioned.

- 2026-08-08: implemented on `impl/P-29`. The accounting is `P-25`'s, moved out of `register.rs`
  into `crate::budget` so there is one `Attempt` and not four: `Attempt::remaining()` funds each
  phase and `Attempt::fund()` funds each candidate of a serial pass. `dial` and a `scenario` dial
  frame now start the clock before resolution and hand the invitation what is left; `load` bounds
  the one lookup its calls share by the lower of `--timeout` and `--duration`.

  **Failing-first, red at `b66d230`** (`cargo test -p sipx-cli --all-features --test cli`):

  ```
  test a_load_run_bounds_resolution_by_its_duration_as_well_as_its_setup_timeout ... FAILED
  test one_stated_deadline_funds_resolution_and_the_invitation_together ... FAILED

  assertion `left == right` failed: a lookup that outlasts the run's own bound is a timeout,
  not a run:   left: Some(0)  right: Some(5)

  dial answered after 3.221946168s: the two seconds it was given funded resolution and then the
  invitation over again, rather than both: {"status":"timeout", …,"invitation_limit_ms":2000,
  "invitation_elapsed_ms":2001, …}
  ```

  Green after: the same `dial` lands at ~2.0 s against the same 1.2 s name and the same
  non-answering peer, and both tests pass beside `dial_timeout_reports_and_obeys_its_cancellation_allowance`,
  which asserts `invitation_limit_ms` at exactly 1000.

  **The two decisions the story asked to be recorded, not implied.**

  §3.2's "starts when the initial INVITE is handed to the endpoint" is **restated**, not kept. Under
  one budget that sentence can only be held two ways and both are worse than rewriting it: publish
  `invitation_limit_ms` as the slice the invitation was funded from, which makes a value two tests
  assert at exact figures depend on how long a lookup took; or publish the stated deadline beside an
  INVITE-scoped elapsed, which reports "you gave me 2s" next to "I waited 0.8s" and names the
  missing 1.2 s nowhere. §3.2 now says the budget starts at the first phase that can wait and that
  every phase is funded from what the last one left, and it records the withdrawal and its reason in
  the paragraph below.

  The two published fields therefore keep one stated meaning, and only one of them moves.
  `invitation_limit_ms` is **unchanged**: the number the caller typed, never the slice a phase was
  funded from — `register`'s restatement rule, applied here. `invitation_elapsed_ms` **moves**: it
  is read on the clock the limit runs on, so it now covers the resolution the same budget paid for
  rather than starting at the INVITE, and it stops where `cancel_elapsed_ms` starts so the two never
  count the same milliseconds twice. That is the one field whose meaning changed, and the coordinator
  owns the `CHANGELOG.md` sentence for it:

  > `dial`'s `invitation_elapsed_ms` is now measured from the start of the command's deadline rather
  > than from the INVITE, so it covers the target resolution that deadline also funds;
  > `invitation_limit_ms` is unchanged and remains the `--timeout` value you stated.

- 2026-08-08: `./scripts/gate.py` deliberately not run — this wave runs one gate before tagging, so
  the row stays unticked. Verified instead: `cargo test -p sipx-cli --all-features` (232 passed, 0
  failed), `cargo clippy -p sipx-cli --all-targets --all-features --no-deps -- -D warnings`,
  `cargo fmt --all`, `./scripts/check-fixed-sleep.py --check`, `./scripts/check-cli-reference.py`,
  `./scripts/check-provenance.sh`.

## Notes

- `P-25`'s `register` is the reference: each candidate is funded from the remainder.
- This is a contract change, not a bug fix — treat the published field semantics as the hard part.

- 2026-08-08: closed at the `1.0.0-rc.10` boundary. The gate row is ticked against the wave
  gate run on this commit's tree; if that run had been red this line would say so instead.
