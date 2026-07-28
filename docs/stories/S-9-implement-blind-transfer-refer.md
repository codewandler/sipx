---
id: S-9
title: Implement blind transfer (REFER)
pillar: Signalling
status: done
priority: 5
design: docs/designs/sip-core.md
epic: depth
areas: [sipx-call]
note:
---

# Implement blind transfer (REFER)

## Goal
Transfer a call to a third party with REFER (RFC 3515), the simple case where the transferor
hands over and leaves.

## Acceptance
- [x] REFER is sent and received within a dialog, with `Refer-To` naming the target.
- [x] The transferee places the new call and reports progress back with NOTIFY, per RFC 3515 §2.4
      — a transferor that never learns the outcome cannot tell success from silence.
- [x] Implicit subscription is honoured: the NOTIFY sequence terminates, and no subscription is
      left running after the transfer finishes.
- [x] A REFER that cannot be honoured is rejected with a status the transferor can act on.
- [x] Failing-first test: `a_referred_call_reaches_the_target_and_notifies_the_transferor`.

## Progress
- Done. `crates/sipx-call/src/transfer.rs` for the types and the `message/sipfrag` body;
  `Call::refer`, `accept_referral`, `refuse_referral` and the REFER/NOTIFY arms of
  `Call::handle`. Tests in `crates/sipx-call/tests/transfer.rs` use three parties, which is the
  smallest number that makes a transfer mean anything — with two, a transferee that reports
  success without placing a call passes.
- The decision worth recording: **`Call::handle` does not answer a REFER.** Every other
  in-dialog request has one correct response; a REFER asks *may I call someone on your behalf*,
  and only the application knows. The two answers are `accept_referral` and `refuse_referral`.
  The exception is a `Refer-To` that names nothing usable, which is answered 400 outright —
  there is nowhere to transfer to, so there is nothing to decide.
- A 202 is modelled as `TransferState::Trying`, never as success. That is the whole point of
  RFC 3515 §2.4.4, and a transferor that read the 202 as success would tell a user their call
  was handed over when it may have been refused or rung out.
