---
id: X-126
title: The default load pair test rejects a call under suite contention
pillar: Quality
status: in-progress
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

- [x] The test passes under the contention the full `sipx-cli` suite actually produces, repeatedly,
      not only when run in isolation.
- [x] Whatever bound or barrier replaces the current one names the question it answers, per the
      `check-fixed-sleep.py` rule.
- [x] If the responder's admission control is the real finding rather than the test's timing, that
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

- 2026-08-08: renumbered from `X-125` on filing. Two agents allocated the same id in the same wave —
  `X-125` was already taken by the non-Linux compile blind spot — so the id moved and nothing else
  did. Cross-references to `X-118` in this file are unaffected; it remains a different mechanism
  from that story's, which is the point of filing it separately.

- 2026-08-08: **reproduced, diagnosed and fixed. The answer is (1), the fixture's headroom, and
  (3) was measured and refuted rather than argued away.**

  **Reproduced first.** `scripts/contention-proof.py`'s technique — two CPU burners per core, 40 on
  this 20-core box — makes it repeatable. At the merge base `1c7d1c4`:

  ```
  $ cargo test -p sipx-cli --all-features --test cli -- --exact \
        default_load_pair_completes_the_requested_signalling_workload
  ```

  **4 of 10 runs failed**, every one with the filed signature — `connected: 19`, `rejected: 1`,
  `peak_concurrency: 8`, responder `active_high_water: 8`, `"503": 1`. Driving the two commands
  directly under the same load, **6 of 12 runs** shed at least one call, up to 4 in one run.

  **(3) is refuted, by measurement and not by reading.** The hypothesis was that `JoinSet::len` —
  which admission control reads — counts a finished-but-unjoined worker, so a dialog whose BYE had
  already been answered would hold a slot until the accept loop got a turn. Implemented it: an
  `Arc<AtomicUsize>` claimed by the accept loop and released by an RAII guard at the end of the
  worker future, so the count is exactly "dialogs still owning something".

  It changed nothing. **8 of 20 runs failed, against 4 of 10 at the base** — the same 40%. A probe
  on the accept loop then said why, printing both counts at every admission decision:

  ```
  PROBE inv=18 active=7 joinset=7 at_capacity=false t=2052ms
  PROBE inv=19 active=8 joinset=8 at_capacity=true  t=2072ms
  PROBE join   active=7 joinset=7                   t=2075ms
  ```

  The two counts agree at every decision point in the trace. At the refusal there were genuinely
  eight live dialogs, and the ninth slot came free 3 ms later. The responder was right. The
  accounting experiment was reverted; it is not in the diff.

  **(1) is the finding.** The responder frees a slot only after its worker future ends, which is
  strictly after `sipx-call` has put the 200 on the wire for that dialog's BYE
  (`signalling.rs:356` responds, then surfaces `RemoteBye`). The generator frees its own slot on
  *receiving* that 200 and places the replacement INVITE at once. So a retiring call is counted by
  both ends across a window that no responder-side accounting can close, and with
  `--max-active` equal to `--concurrency` that window sat on the happy path of all twelve slot
  handovers in the run. Ordinary scheduling jitter crossed the line; on an idle box the responder
  wins the race and the test looks solid.

  (2) was weighed and rejected. Asserting only on `attempted`/`failed`/`timed_out` would let a
  responder that 503s every call pass, which is not "the workload completed".

  **What changed, and the sentence the headroom comes with.** `--max-active` is now `2 ×
  concurrency`, and the factor is derived, not tuned: *every one of the generator's concurrent
  slots can free and place its replacement INVITE while the dialog it replaced is still between the
  responder answering its BYE and the responder releasing its slot, and in the worst case all of
  them do so at once.* Measured headroom for comparison — 12 runs each under the same load, all
  0 rejections at `--max-active` 9, 12 and 16, with observed `active_high_water` never above 9. So
  9 would also have passed; it is the value the observation supports rather than the one the
  argument does, which is why it is 16.

  `active_high_water` moved from `== 8` to `8..=16`. Both ends carry a claim — at least the
  generator's concurrency proves the workload was really carried, at most the ceiling proves
  admission control still held — and the figure in between is the scheduling detail the test
  stopped asserting on. The unscaled `Duration::from_secs(15)` around the `load` run (the one
  wall-clock bound in this test that `X-118`'s `bound()` had missed) is now wrapped and says what
  it bounds.

  `default_load_pair_completes_the_requested_signalling_workload` is now a `contention-proof.py`
  subject. Its `SUBJECTS` docstring said "each bounds a wall clock"; that is now accurate about
  which subjects are `X-118`'s mechanism and which is not, since this one never waited on a clock.
  Running it there is what stops the headroom being quietly taken back.

  **Pass rate after: 30 of 30** under the identical load that gave 4 of 10 at the base. Full
  `cargo test -p sipx-cli --all-features` green; clippy, fmt, `check-fixed-sleep.py --check`,
  `contention-proof.py --check`, `test-contention-proof.py` and `check-provenance.sh` all green.
  `gate.py` deliberately not run here — the coordinator runs one gate per wave — so that row is
  left unticked.

  **Owed CHANGELOG sentence** (not written here; `CHANGELOG.md` is the coordinator's):

  > `sipx load-responder`'s admission ceiling in the default load-pair test now sits above the
  > generator's concurrency, so the test measures whether the workload completed rather than
  > whether the machine had a spare core.

  **Left open / adjacent.** `generated_media_load_pair_retains_the_rtp_workload` has the same
  zero-headroom shape — `--concurrency 2` against `--max-active 2` — but only two slot handovers
  against this test's twelve, and it held 20 of 20 under the same load. Latent, not observed, and
  left alone rather than swept into this diff. Filed as `X-129`: nothing warns a user sizing a real
  run that `--max-active` equal to a generator's concurrency will shed calls by design.
