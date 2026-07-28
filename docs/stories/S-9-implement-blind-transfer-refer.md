---
id: S-9
title: Implement blind transfer (REFER)
pillar: Signalling
status: ready
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
- [ ] REFER is sent and received within a dialog, with `Refer-To` naming the target.
- [ ] The transferee places the new call and reports progress back with NOTIFY, per RFC 3515 §2.4
      — a transferor that never learns the outcome cannot tell success from silence.
- [ ] Implicit subscription is honoured: the NOTIFY sequence terminates, and no subscription is
      left running after the transfer finishes.
- [ ] A REFER that cannot be honoured is rejected with a status the transferor can act on.
- [ ] Failing-first test: `a_referred_call_reaches_the_target_and_notifies_the_transferor`.

## Progress
- Not started.
