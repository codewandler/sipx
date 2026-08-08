---
id: X-129
title: Say what --max-active costs a caller who sizes it to their concurrency
pillar: Quality
status: backlog
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

- [ ] `--max-active`'s documented description states that the responder releases a slot only after
      it has answered the dialog's BYE, so a peer that places its next call on receiving that 200
      can arrive before the slot is free.
- [ ] The guidance names the headroom that avoids it and what the headroom is *for*, rather than a
      number to copy.
- [ ] Whatever artifact is generated from the CLI help stays in sync.
- [ ] `./scripts/gate.py` green.

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
