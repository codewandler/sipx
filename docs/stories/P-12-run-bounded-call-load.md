---
id: P-12
title: Run bounded call load from the diagnostic phone
pillar: Phone
status: done
priority: 10
design: docs/designs/phone.md
epic: phone
areas: [sipx-cli, sipx-testkit]
note: reuse X-4's load model; every run has finite admission and cleanup bounds
---

# Run bounded call load from the diagnostic phone

## Goal

Expose the existing load model as a safe, reproducible command with a result a release proof can
assert.

## Acceptance

- [x] `sipx load` enforces every bound and cleanup rule in `diagnostic-phone.md` §5 before starting.
- [x] The runner reuses `sipx-testkit` rather than creating a second load scheduler.
- [x] Seed, effective limits, call outcomes, response codes, setup distribution and media quality are
      emitted in the stable JSON summary.
- [x] Interrupt, timeout and internal error stop admission, terminate the whole owned workload and
      wait for cleanup before returning.
- [x] `DPH-10` and `DPH-11` fail first; a bounded overload scenario also feeds `T-22` evidence.

## Progress

- `sipx load` now validates a target, positive finite rate and concurrency, and at least one
  positive call-count or duration bound before binding. The command shares transport identity and
  digest policy with `dial`, and a seed deterministically controls both paced arrival jitter and a
  bounded generated media frame per connected call.
- `sipx-testkit::load::run_bounded` owns admission, the concurrency ceiling, the stop signal and the
  cleanup join. Count, duration, Ctrl-C and internal-task failure all close admission; every call
  receives the stop signal; cleanup has the normative 40-second failure bound and no task is
  detached on return.
- The one-line `sipx.load.v1` summary carries effective limits, cause-separated outcomes, response
  codes, setup percentiles and aggregate media quality. Missing measurements are JSON `null`.
- DPH-10 is exercised through the real binary against a peer that observes exactly three INVITEs
  and rejects them. DPH-11 sends SIGINT only after a peer observes the first INVITE, then proves no
  second call was admitted and the single interrupted summary followed the timed-out call's cleanup.
  Both vectors also exercise the scheduler directly and assert return follows every owned call's
  cleanup acknowledgement.
