---
id: X-125
title: The default load pair test rejects a call under suite contention
pillar: Quality
status: backlog
priority: 2
design:
epic:
areas: [sipx-cli]
predicate:
announcement:
note: default_load_pair_completes_the_requested_signalling_workload fails intermittently in the full cli suite — the responder 503s one call at max_active, connecting 19 of 20
---

# The default load pair test rejects a call under suite contention

## Goal

Make `default_load_pair_completes_the_requested_signalling_workload` decide on a happens-before
relation rather than on whether the machine had a spare core, so a green gate means the workload
completed and a red one means it did not.

## Acceptance

- [ ] The test passes under the contention the full `sipx-cli` suite actually produces, repeatedly,
      not only when run in isolation.
- [ ] Whatever bound or barrier replaces the current one names the question it answers, per the
      `check-fixed-sleep.py` rule.
- [ ] If the responder's admission control is the real finding rather than the test's timing, that
      is stated with evidence and the story is re-pointed at the responder.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `P-31`'s wave. Observed once in a full
  `cargo test -p sipx-cli --all-features` run and **not** reproducible in isolation — the same test
  passed 3/3 when run alone immediately afterwards, and passed in an earlier full run the same
  hour. So it is contention-sensitive rather than broken.

  The failure, verbatim from the load summary:

  ```json
  "outcomes":{"attempted":20,"connected":19,"failed":0,"peak_concurrency":8,"rejected":1,"timed_out":0}
  "response_codes":{"200":19,"503":1}
  ```

  and from the responder beside it:

  ```json
  "counts":{"active_high_water":8,"admitted":19,"cancelled":0,"completed":19,"established":19,
            "failed":0,"invalid_messages":0,"invitations":20,"rejected":1}
  "limits":{"calls":20,"cleanup_ms":5000,"dialog_duration_ms":5000,"duration_ms":null,"max_active":8}
  ```

  The generator's `concurrency` is 8 and the responder's `max_active` is 8, and
  `active_high_water` reached exactly 8. So the 20th INVITE arrived while the responder still had
  eight dialogs it had not finished tearing down, and admission control refused it with 503 —
  correctly, by its own contract. Nothing failed or timed out; one call was *rejected*.

  That makes the assertion `connected == 20` a bet that teardown of call *n* completes before
  INVITE *n+8* arrives, which is a scheduling race and not a property of the code under test.

## Notes

- `crates/sipx-cli/tests/cli.rs`, the assertion that failed is on `connected`.
- Two directions worth weighing before picking one: give the responder headroom over the
  generator's concurrency so admission control is not on the happy path at all, or assert on
  `attempted` and `failed`/`timed_out` and treat a 503 under a deliberately equal limit as a
  legitimate outcome. The first keeps the test's current meaning; the second changes what is being
  claimed, so it needs saying out loud.
- **Not a duplicate of `X-118`, and the distinction is the point.** `X-118` collects tests that fail
  under load because a *wall-clock bound* expired, and its remaining open row is the
  tolerance-versus-controllable-clock question for those. This one never waits on a clock: the
  responder answered, immediately and correctly, with a 503 it was configured to send. Scaling a
  bound would not touch it, and `support::machine`'s `bound()` and `scripts/contention-proof.py` —
  which exist for exactly `X-118`'s mechanism — do not help here. If the two are merged, the fix for
  this one still has to be admission headroom or a changed assertion, not a tolerance.
