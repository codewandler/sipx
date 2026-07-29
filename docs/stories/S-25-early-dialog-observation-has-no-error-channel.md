---
id: S-25
title: Give the early-dialog loop a way to fail
pillar: Signalling
status: ready
priority: 6
design: docs/designs/sip-ua.md
epic: conformance
areas: [sipx-call]
note: found by M-29 — adopt_early_answer returns (), so a parse failure, a negotiation failure and a refused a=crypto in a reliable provisional are all discarded identically
---

# Give the early-dialog loop a way to fail

## Goal
Let a caller learn that the session description in a reliable provisional was refused, instead of
discovering it as a log line — or not at all, if no 2xx ever arrives.

## Acceptance
- [ ] `Invitation::adopt_early_answer` (and the `observe` loop that calls it) can report a fatal
      outcome. Today it returns `()`, so three unrelated failures leave the same trace: a body that
      does not parse, a description that cannot be negotiated, and — since `M-29` — an `a=crypto`
      that fails RFC 4568 §5.1.3's check.
- [ ] The RFC 4568 refusal in particular reaches the application with the tag that came back. It is
      currently `tracing::debug!` and nothing more. The call still fails safely when the 2xx
      arrives, because `settle_from` runs the same check and *does* propagate — so this is about
      the reason being lost, not about media being keyed on an unchecked answer.
- [ ] **The failure mode chosen is stated, and it is not ACK-then-BYE.** Ending an invitation from
      a *provisional* means CANCEL (RFC 3261 §9.1), because there is no final response to ACK. That
      is a different shape from what `dial` does after a 2xx, and picking it is the substance of
      this story rather than an implementation detail.
- [ ] The early-dialog loop `S-22` landed keeps working for the case where nothing is wrong: a
      provisional carrying no description at all is not an error and must stay silent.
- [ ] Failing-first test: a caller whose reliable provisional carries an answer echoing a tag it
      never offered, and which never receives a 2xx, currently waits and then times out with no
      indication of why. Name the test that makes it report.

## Progress
- Not started.

## Notes
- Found by `M-29` while wiring RFC 4568 §5.1.3 into `sipx-call`. It could not fix this within its
  own fence: `observe` returns `()` from inside the loop that drives an early dialog, so giving it
  a fatal path is a change to the machinery `S-22` had just landed, with CANCEL semantics attached.
  Logging at `debug` was the smallest honest thing that fit, and it said so.
- **The security property is already safe; this is about diagnosis.** On refusal the session stays
  `Offered`, so nothing is keyed on the refused answer, and the 2xx re-settles through
  `settle_from` where the refusal ends the call. The gap is the caller who never gets a 2xx.
- Reads with `C-2` (early media): both want the early dialog to tell the application something it
  currently cannot, and `S-22` deliberately left `Dialing` without an event stream, recording that
  as `C-2`'s to design. If `C-2` gives the early dialog a stream, this story's answer may be a
  variant on it rather than a separate channel — worth checking before designing a second path.
