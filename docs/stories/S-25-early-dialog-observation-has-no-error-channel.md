---
id: S-25
title: Give the early-dialog loop a way to fail
pillar: Signalling
status: done
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
- [x] `Invitation::adopt_early_answer` (and the `observe` loop that calls it) can report a fatal
      outcome. Today it returns `()`, so three unrelated failures leave the same trace: a body that
      does not parse, a description that cannot be negotiated, and — since `M-29` — an `a=crypto`
      that fails RFC 4568 §5.1.3's check.
      → `adopt_early_answer` and `observe` both return `Result<()>`; the three failures are
      `Error::Sdp` (parse), `Error::NoCommonCodec` (negotiation) and `Error::Sdp` (§5.1.3), each
      distinct in what it says. `crates/sipx-call/src/call.rs` — `observe`, `adopt_early_answer`.
- [x] The RFC 4568 refusal in particular reaches the application with the tag that came back. It is
      currently `tracing::debug!` and nothing more. The call still fails safely when the 2xx
      arrives, because `settle_from` runs the same check and *does* propagate — so this is about
      the reason being lost, not about media being keyed on an unchecked answer.
      → the `tracing::debug!` is gone; `settle_answer`'s error propagates with `?`. Asserted by
      `a_refused_early_answer_ends_the_invitation_with_a_cancel`, which requires `Error::Sdp`, that
      it name the tag, and that it *not* repeat the key material.
- [x] **The failure mode chosen is stated, and it is not ACK-then-BYE.** Ending an invitation from
      a *provisional* means CANCEL (RFC 3261 §9.1), because there is no final response to ACK. That
      is a different shape from what `dial` does after a 2xx, and picking it is the substance of
      this story rather than an implementation detail.
      → **CANCEL.** Stated in `Dialing::abandon`'s doc comment, which is the one place the choice
      and its alternative are written down, and asserted on the wire: the test requires the *first*
      request after the PRACK to be a `CANCEL`, which also rules out an ACK or a BYE.
- [x] The early-dialog loop `S-22` landed keeps working for the case where nothing is wrong: a
      provisional carrying no description at all is not an error and must stay silent.
      → `a_reliable_provisional_with_no_description_leaves_the_invitation_running`, which also
      answers the 2xx afterwards — an `Ok` from `dial_early` alone would not prove the invitation
      was still there.
- [x] Failing-first test: a caller whose reliable provisional carries an answer echoing a tag it
      never offered, and which never receives a 2xx, currently waits and then times out with no
      indication of why. Name the test that makes it report.
      → `a_refused_early_answer_ends_the_invitation_with_a_cancel` in
      `crates/sipx-call/tests/update.rs`.

## Progress
- **Done.** The error channel is the **return type of the early-dialog loop**, not a new stream:
  `adopt_early_answer` and `observe` return `Result<()>`, and `answered`/`reach_early_dialog` turn
  a refusal into `Dialing::abandon`, which CANCELs and returns the error. `dial_early` therefore
  fails outright when the provisional that would have established the dialog carries an answer it
  cannot use, rather than handing back a handle to an invitation nobody can complete.
- **The failure mode is CANCEL (RFC 3261 §9.1)**, and it is chosen by *where* we are rather than by
  what went wrong: there is no final response to acknowledge, so §15's ACK-then-BYE — what `dial`
  and `Dialing::confirm` do after a 2xx — has nothing to attach itself to. `give_up`/`withdraw`
  were already exactly this request, including the case a CANCEL cannot close: a `200` that crossed
  it is acknowledged and hung up, because by then §15 *does* apply. So the shape was in the crate;
  what was missing was a path to it.
- **The provisional is PRACKed before it is refused.** RFC 3262 §4 makes the acknowledgement a MUST
  for every reliable provisional a UAC receives and the far end retransmits until one arrives, so
  `observe` holds the refusal, acknowledges, and only then propagates. Failing a beat sooner would
  leave the peer resending a response into a CANCEL that had already gone. The test asserts the
  PRACK and then the CANCEL, in that order.
- **No new event stream, deliberately** — the story's Notes asked this to be resolved first. Three
  reasons. `S-22` left `Dialing` without one and recorded it as `C-2`'s to design, and `C-2` is
  still `ready`. The defect is that the caller *waits*, and an event on a channel nobody is obliged
  to read would not end the wait — the future would still hang to the deadline; what ends it is the
  loop returning. And the calling side already has an error channel the answering side lacks:
  `Invitation` needed a stream because a ringing application has nothing to await (`S-23`), whereas
  a caller is inside `dial_early`/`answered`, both of which return `Result`. Nothing here forecloses
  `C-2` also emitting this on a `Dialing` stream when it builds one.
- **No new error variant, deliberately.** The refusal comes back as `Error::Sdp` /
  `Error::NoCommonCodec` — the same vocabulary `settle_from` uses on the 2xx, because it is the
  same fault found earlier. A second spelling would make an application match twice for one thing.
- **This is diagnosis, not a vulnerability fix**, and the code says so where it could be misread:
  the session stayed `Offered` before this story, so nothing was ever keyed on a refused answer,
  and `settle_from` re-ran the same check on the 2xx. What was lost was the reason — entirely, for
  a caller that never receives a 2xx.
- Failing-first evidence, with only the tests added:
  `cargo test --all-features -p sipx-call --test update` →
  `a_refused_early_answer_ends_the_invitation_with_a_cancel ... FAILED`,
  `panicked at crates/sipx-call/tests/update.rs:1768:29: the withdrawal of the invitation never
  arrived` — the caller PRACKed the provisional, kept waiting, and sent nothing. The companion
  `a_reliable_provisional_with_no_description_leaves_the_invitation_running` passed at the merge
  base and still does; it is a guard, not a new capability.
- The refusal test runs over **WSS**, which the rest of `tests/update.rs` does not. RFC 4568 §7.1
  means sipx offers a key only where the signalling protects one, so over the file's UDP pair there
  is no `a=crypto` in the INVITE and §5.1.3's check never runs at all.
- Registry: RFC 4568's note gains the early-dialog half and `tests/update.rs` as evidence.
  `status` stays `partial`, for what it was already partial for (no MKI, no key lifetimes, no
  session parameters, no `RTP/SAVPF`).
- Gate: `./scripts/gate.py` → 18 steps, all green.
- Re-verified after the interruption, on the same branch. The failing-first run against the
  merge-base `call.rs` with only the tests added reproduces exactly what is recorded above —
  `update.rs:1768:29: the withdrawal of the invitation never arrived` — and the no-description
  guard passes there. All 16 `tests/update.rs` tests pass with the change, and the gate's 18
  steps are green on the real run; the line above was written before one.

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
