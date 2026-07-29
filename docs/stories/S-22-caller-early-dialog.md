---
id: S-22
title: Give the caller a handle on its early dialog
pillar: Signalling
status: in-progress
priority: 5
design: docs/designs/sip-ua.md
epic: conformance
areas: [sipx-call]
note: found by S-19 — sipx as caller can neither send nor receive an UPDATE while ringing
---

# Give the caller a handle on its early dialog

## Goal
Let the application reach the early dialog on the **caller's** side, so everything sipx can already
do before a call is answered — renegotiate with UPDATE, receive one, read early media — is
available to the side that placed the call and not only to the side that received it.

## Acceptance
- [ ] `dial` (or a sibling that does not replace it) yields the early dialog to the application
      instead of consuming it internally. Today `dial_with` awaits the final response inside itself
      (`crates/sipx-call/src/call.rs:1853-1869`), so there is no moment at which the caller holds
      anything.
- [ ] From that handle a caller can **send** an UPDATE and **receive** one, with the same RFC 3311
      §5.2 refusal rules the answering side already applies — `S-19` built the decision as a pure
      function in `sipx_sip::update`, so this story wires it, it does not re-decide it.
- [ ] The existing `dial` signature keeps working unchanged for callers that only want the answered
      `Call`. A story that makes the simple case harder has traded the wrong thing.
- [ ] RFC 3311's registry row gains the `uac` role for the early-dialog case, or the note says
      precisely which half is still missing. `docs/specs/sip-update.md` §3 currently reads as
      though either end can do this; after this story it should be true, or the section should say
      which end cannot.
- [ ] Failing-first test: `a_caller_renegotiates_before_the_callee_answers`.

## Progress
- Not started.

## Notes
- Found by `S-19` and confirmed by its independent review: both send-side assertions in
  `sipx_sends_an_update_in_an_early_dialog_and_in_a_confirmed_one` are UAS-role, because the UAC
  early dialog is unreachable. ~~`compliance.md`'s RFC 3311 row lists no roles, so nothing
  over-claims today — but the spec prose does.~~
- **Correction, verified twice: the registry over-claims too, and the sentence above was wrong.**
  The RFC 3311 row is `status = "implemented"` and its note opens *"Sent and received in an early
  dialog and a confirmed one."* An empty `roles` field does not neutralise a claim written in the
  passive voice — that sentence reads as role-neutral and so claims for both ends something only
  the answering end can do. Confirmed by reading the test rather than the name: the "caller" in
  `crates/sipx-call/tests/update.rs:782` is a raw peer sending a hand-built `raw_invite`, `dial`
  never appears in the file, and both send-side assertions go through `ring_early` and
  `answer_early`, which are UAS handles.
- **So the registry Acceptance item is larger than adding `uac`.** The note's first sentence has to
  name which half works *before* the role becomes a truthful edit rather than a second over-claim
  stacked on the first. If the caller half ever slips out of this story, the note must be corrected
  on its own and `status: implemented` re-examined — it is not defensible for a method whose send
  path exists in only one role.
- The test name promises a UAC path it never exercises; it should say UAS when this story lands
  beside it.
- This is the natural prerequisite for the caller's half of `C-2` (early media, RFC 3960): a caller
  that cannot hold its early dialog cannot act on a session description arriving in a provisional
  either. Worth reading the two together before designing this one.
- Deliberately not folded into `S-19`: exposing the early dialog means restructuring `dial`, which
  is a change to the most-used entry point in `sipx-call` and deserves its own failing test and its
  own review rather than riding along with the UPDATE method.
