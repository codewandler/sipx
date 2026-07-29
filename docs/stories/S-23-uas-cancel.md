---
id: S-23
title: Answer a CANCEL — the UAS half sipx never implemented
pillar: Signalling
status: in-progress
priority: 1
design: docs/designs/sip-ua.md
epic: conformance
areas: [sipx-call, sipx-ua, sipx-sip]
note: found by C-4 — no 487 for the INVITE and no 200 for the CANCEL anywhere in the workspace
---

# Answer a CANCEL — the UAS half sipx never implemented

## Goal
Let a caller give up before sipx answers. A CANCEL arriving for an invitation sipx is still ringing
on must end that invitation the way RFC 3261 §9.2 requires, rather than being routed somewhere and
ignored.

## Acceptance
- [ ] A CANCEL matching a pending INVITE transaction is answered **200 OK** (RFC 3261 §9.2), and
      the INVITE it cancels is answered **487 Request Terminated** — two responses on two
      transactions, which is the part that is easy to get half-right.
- [ ] A CANCEL that matches no pending INVITE transaction is answered **481** (§9.2), not dropped.
- [ ] The matching is the RFC's, not an approximation: §9.2 matches on the top `Via` branch and the
      request method of the transaction being cancelled, so a CANCEL for a transaction that has
      already sent a final response has no effect and says so.
- [ ] The application learns the invitation ended and why. A host holding a ringing call needs to
      stop ringing; an invitation that is cancelled must not be answerable afterwards.
- [ ] A CANCEL arriving *after* sipx has sent a 2xx is not a way to tear down the dialog — §9.2 is
      explicit that CANCEL has no effect on a transaction that has already answered, and BYE is the
      request for that. Tested as a negative.
- [ ] Failing-first test: `a_caller_that_gives_up_before_the_answer_ends_the_invitation`.

## Progress
- Not started.

## Notes
- Found by `C-4` while building the dispatcher: it routes a CANCEL for a routed invitation into
  that call's inbox and surfaces an unrouted one, so nothing is lost — but there is nothing on the
  other end that honours it. A grep of the workspace finds no 487 and no UAS-side CANCEL handling
  at all.
- **Priority 1 because it is a conformance hole in the most ordinary flow there is.** A caller
  hanging up while the phone is still ringing is not an edge case; today sipx keeps ringing and the
  caller's stack is left to time out.
- `C-4`'s dispatcher is the natural place for the routing half and is already done; this story owes
  the transaction matching and the two responses. Read `docs/specs/call-dispatch.md` first — it
  records where a CANCEL currently goes and says explicitly that routing it is not support for it.
- Related in kind: `T-19` and `C-4` both exist because a request that reaches sipx and produces no
  response is the failure this project keeps deciding it will not ship.
