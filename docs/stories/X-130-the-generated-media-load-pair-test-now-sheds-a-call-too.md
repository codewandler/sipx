---
id: X-130
title: The generated-media load pair test now sheds a call too
pillar: Quality
status: backlog
priority: 2
design:
epic:
areas: [sipx-cli]
predicate:
announcement:
note: generated_media_load_pair_retains_the_rtp_workload runs --concurrency 2 against --max-active 2 and was observed rejecting one of four calls in a full cli suite run
---

# The generated-media load pair test now sheds a call too

## Goal

Give `generated_media_load_pair_retains_the_rtp_workload` the same admission headroom `X-126` gave
the signalling pair, so `connected == calls` is a claim about the workload rather than about
whether the responder's accept loop was scheduled in time.

## Acceptance

- [ ] The fixture's responder ceiling clears the generator's concurrency, and the change carries
      the reason rather than a tuned number.
- [ ] `active_high_water` is asserted as a range whose ends each carry a claim, as the signalling
      pair's is, rather than dropped.
- [ ] The test passes repeatedly under the contention the full `sipx-cli` suite produces.
- [ ] `./scripts/gate.py` green.

## Notes

`X-126` named this test as the remaining zero-headroom fixture and left it alone deliberately: it
had the same shape — `--concurrency 2` against `--max-active 2` — but only two slot handovers
against the signalling pair's twelve, and it held 20 of 20 under deliberate CPU load. It has now
been observed failing, so it is no longer latent.

Seen in `cargo test -p sipx-cli --all-features` on 2026-08-09 while implementing `X-129`, on a tree
whose only differences from `c405a60` were documentation, one added documentation test, and this
story file. The load summary, verbatim:

```json
"limits":{"call_duration_ms":1000,"calls":4,"cleanup_ms":40000,"concurrency":2,
          "duration_ms":null,"rate":20.0,"setup_timeout_ms":5000}
"outcomes":{"attempted":4,"connected":3,"failed":0,"peak_concurrency":2,"rejected":1,"timed_out":0}
"response_codes":{"200":3,"503":1}
```

Nothing failed and nothing timed out; one call was *rejected* with 503 at a ceiling equal to the
generator's concurrency. That is the mechanism `X-126` measured and `X-129` documented — the
responder frees a slot only after answering that dialog's BYE, while the generator frees its own on
receiving that 200 — and it is the responder behaving correctly, so the fix belongs in the fixture.

`X-126`'s reasoning for `2 ×` transfers directly: every one of the generator's concurrent slots can
free and place its replacement INVITE while the dialog it replaced is still between the responder
answering its BYE and the responder releasing its slot, and in the worst case all of them do so at
once. Its measured headroom is on a concurrency of 8, not 2, so nothing here should copy the number
9, 12 or 16 — carry the argument, not the observation.

Worth checking whether this test also belongs in `scripts/contention-proof.py`'s `SUBJECTS`, as
`X-126` added the signalling pair. That is what stops the headroom being quietly taken back.
