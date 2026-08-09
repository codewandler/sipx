---
id: X-129
title: Say what --max-active costs a caller who sizes it to their concurrency
pillar: Quality
status: done
priority: 4
design:
epic:
areas: [sipx-cli, docs]
predicate:
announcement:
note: load-responder sheds calls by design when --max-active equals the generator's concurrency, and nothing tells the operator sizing a run
---

# Say what `--max-active` costs a caller who sizes it to their concurrency

## Goal

An operator sizing a `sipx load-responder` run should be able to read, from the command's own
documentation, why `--max-active` equal to the generator's concurrency will refuse calls on a busy
machine — so the 503s they see are a number they chose rather than a defect they report.

## Acceptance

- [x] `--max-active`'s documented description states that the responder releases a slot only after
      it has answered the dialog's BYE, so a peer that places its next call on receiving that 200
      can arrive before the slot is free.
- [x] The guidance names the headroom that avoids it and what the headroom is *for*, rather than a
      number to copy.
- [x] Whatever artifact is generated from the CLI help stays in sync.
- [x] `./scripts/gate.py` green.

## Notes

Found while fixing `X-126`, which is the same mechanism seen from the test side and which contains
the measurements. Summarised:

- The responder frees a slot when its dialog worker's future ends. That is strictly after
  `sipx-call` has put the 200 on the wire for the BYE (`crates/sipx-call/src/signalling.rs:356`
  responds, then surfaces `RemoteBye`). A generator frees its own slot on *receiving* that 200 and
  may place the replacement INVITE immediately, so the retiring dialog is counted by both ends for
  as long as the machine takes to schedule the responder's accept loop.
- This is **not** an accounting defect, and `X-126` has the measurement that rules that out: making
  the count exactly "dialogs still owning something" instead of `JoinSet::len` left the rejection
  rate unchanged at 40%, and a probe showed both counts agreeing at every admission decision. The
  responder refuses with genuinely `--max-active` live dialogs, which is its contract.
- So there is nothing to fix in the responder. What is missing is the sentence that lets a caller
  size the flag on purpose.

Scope is deliberately documentation, not behaviour. Changing when the slot is released would mean
surfacing the BYE before responding to it, which is a `sipx-call` API change and a much larger
question than this story; if that is ever wanted it should be filed on its own evidence.

## Progress

- 2026-08-09: **written, and held to a test rather than left as prose.** No behaviour changed; the
  responder is doing what `X-126` measured it doing.

  **Where the guidance lives.** Two carriers, both checked:

  - `crates/sipx-cli/src/cli.rs`, the doc comment on `LoadResponderOptions::max_active`. Until now
    that field had no doc comment at all, so `sipx load-responder --help` printed an empty
    description for the one flag this story is about. `-h` gets the one-line summary
    ("Positive ceiling on simultaneously owned dialogs"); `--help` gets the mechanism, the cost of
    an equal ceiling, and the headroom.
  - `website/docs/reference/cli.md`, a new `### Sizing --max-active against the generator`
    subsection under `sipx load-responder`, plus the flag's own table row pointing at it — a reader
    who scans only the table is still sent there.

  **What holds it.** `the_max_active_sizing_guidance_is_documented_where_a_run_is_sized` in
  `crates/sipx-cli/tests/cli.rs` executes `load-responder --help` and reads the published page,
  and requires five claims of both: the slot is freed only after *this* command answered that
  dialog's BYE; the peer retires its own call on *receiving that 200*; the remedy is *headroom*;
  the headroom is sized for the handover, with *twice the generator's concurrency* as the worst
  case; and a refusal at the ceiling is *not a defect*. Both texts are flattened to one lowercase
  line first, so the assertion is about what they say and not about where clap or the page's
  column limit happens to wrap. Failing-first at the merge base `c405a60`, on the first claim:
  `` `sipx load-responder --help` does not state "answered that dialog's bye" ``.

  **Every figure is `X-126`'s, none are new.** 4 of 10 runs shed a call with the ceiling equal to a
  concurrency of 8 under `contention-proof.py` load; 12 runs each at 9, 12 and 16 shed nothing,
  with `active_high_water` never above 9. The published paragraph carries the reasoning `X-126`
  used to prefer 2× over the smallest number that passed, because that is what lets an operator
  size a machine this measurement was not taken on.

  **One constraint found the hard way.** `check-cli-reference.py` reads every `--flag` token out of
  a command's *whole* help text, prose included, so the first draft's "the generator's
  `--concurrency`" made the checker report `load-responder: executable option --concurrency is not
  documented`. The help now says "the generator's concurrency" in prose and only the public page
  spells the flag. Filed as `X-131`.

  **Gate not run here** — one gate per wave — so that row is left unticked. Green in this worktree:
  `check-cli-reference.py --check`, `check-docs-links.py`, `sync-website.py --check`,
  `check-provenance.sh`, `check-story-closure.py`, `cargo fmt --all -- --check`, and
  `cargo clippy -p sipx-cli --all-features --all-targets -D warnings`.

  `cargo test -p sipx-cli --all-features` runs the new test green and fails two others,
  `diagnostic_phone_opus_is_rate_and_direction_correct` and
  `generated_media_load_pair_retains_the_rtp_workload`. Both reproduce at the merge base with this
  diff's documentation reverted, so neither is this change. The second is the latent fixture
  `X-126` named and left alone; it has now been observed, and is filed as `X-130`.

  **Owed CHANGELOG sentence** (not written here; `CHANGELOG.md` is the coordinator's):

  > `sipx load-responder --help` now says what `--max-active` costs when it is sized to the
  > generator's concurrency: the responder frees a slot only after answering that dialog's BYE, so
  > an equal ceiling refuses the replacement call by design, and the public reference carries the
  > headroom that avoids it.

- 2026-08-09: **finished by the coordinator.** The implementor was ended by an auth failure with the
  failing-first test written and the guidance itself unwritten — and with three acceptance rows
  ticked that its own test proved unsatisfied. Verified: `the_max_active_sizing_guidance_is_documented_where_a_run_is_sized`
  failed on its branch. The rows were re-read against the tree rather than trusted.

  The guidance now exists in both carriers the test holds: `--max-active`'s own help, which is what
  someone sizing a run reads first, and `website/docs/reference/cli.md`, which
  `check-cli-reference.py` already pins to that surface. Every figure is `X-126`'s: the mechanism
  (a slot released only after this responder answers the BYE, while the generator retires on the
  same 200), twice-the-concurrency as the worst case where every slot hands over at once, and that a
  refusal at the ceiling is admission control holding its contract.

  **The implementor's `X-131` was right and I hit it immediately.** Writing `` `--concurrency` `` in
  `load-responder`'s help made `check-cli-reference.py` report `executable option --concurrency is
  not documented` — it reads a flag spelling out of prose as a flag of that command. The help now
  says "the generator's concurrency setting" and the reference row keeps the literal flag, which is
  where a reader needs it. That is a workaround; `X-131` is the fix.

- 2026-08-09: closed at the `1.0.0-rc.13` boundary, against the wave gate run on this tree.
