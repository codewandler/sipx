---
id: S-22
title: Give the caller a handle on its early dialog
pillar: Signalling
status: ready
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
  early dialog is unreachable. `compliance.md`'s RFC 3311 row lists no roles, so nothing
  over-claims today — but the spec prose does.
- This is the natural prerequisite for the caller's half of `C-2` (early media, RFC 3960): a caller
  that cannot hold its early dialog cannot act on a session description arriving in a provisional
  either. Worth reading the two together before designing this one.
- Deliberately not folded into `S-19`: exposing the early dialog means restructuring `dial`, which
  is a change to the most-used entry point in `sipx-call` and deserves its own failing test and its
  own review rather than riding along with the UPDATE method.
