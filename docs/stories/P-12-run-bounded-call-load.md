---
id: P-12
title: Run bounded call load from the diagnostic phone
pillar: Phone
status: backlog
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

- [ ] `sipx load` enforces every bound and cleanup rule in `diagnostic-phone.md` §5 before starting.
- [ ] The runner reuses `sipx-testkit` rather than creating a second load scheduler.
- [ ] Seed, effective limits, call outcomes, response codes, setup distribution and media quality are
      emitted in the stable JSON summary.
- [ ] Interrupt, timeout and internal error stop admission, terminate the whole owned workload and
      wait for cleanup before returning.
- [ ] `DPH-10` and `DPH-11` fail first; a bounded overload scenario also feeds `T-22` evidence.

## Progress

- Not started.
