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
- [x] A CANCEL matching a pending INVITE transaction is answered **200 OK** (RFC 3261 §9.2), and
      the INVITE it cancels is answered **487 Request Terminated** — two responses on two
      transactions, which is the part that is easy to get half-right.
- [x] A CANCEL that matches no pending INVITE transaction is answered **481** (§9.2), not dropped.
- [x] The matching is the RFC's, not an approximation: §9.2 matches on the top `Via` branch and the
      request method of the transaction being cancelled, so a CANCEL for a transaction that has
      already sent a final response has no effect and says so.
- [x] The application learns the invitation ended and why. A host holding a ringing call needs to
      stop ringing; an invitation that is cancelled must not be answerable afterwards.
- [x] A CANCEL arriving *after* sipx has sent a 2xx is not a way to tear down the dialog — §9.2 is
      explicit that CANCEL has no effect on a transaction that has already answered, and BYE is the
      request for that. Tested as a negative.
- [x] Failing-first test: `a_caller_that_gives_up_before_the_answer_ends_the_invitation`.

## Progress
Done, pending review. The rule is written down in `docs/specs/call-dispatch.md` §9, which is where
a reader should start; this is the record of what was decided and what was left.

**Where it lives.** `Dispatcher::cancel` (`crates/sipx-call/src/dispatch.rs`) places every CANCEL —
it is a new row 4 of §3's table, above the route lookup, because a CANCEL belongs to a transaction
rather than to a dialog and routing it by key is what put it in an unread inbox before. The
matching key is `sipx_sip::TransactionKey::for_cancelled_invite`, which already existed and already
had §9.2's rule in its doc comment; no new matching logic was written in `sipx-sip`.

**The application's half** is on `Invitation`: `is_cancelled` to poll, `events` to be woken, and
`answer`, which refuses once the caller has gone and records the final response that makes a later
CANCEL the no-op §9.2 requires. The event is `CallEvent::Ended(EndCause::RemoteCancel)` on `C-3`'s
own stream — a new `EndCause` variant, additive on a `#[non_exhaustive]` enum, mapping to the
contract's `remote` cause on the wire.

**Two decisions worth arguing with.**

1. §9.1's identifier check is applied *in addition* to §9.2's transaction match: a CANCEL whose
   `Call-ID` and `From` tag disagree with the INVITE its branch names is refused 481. §9.1 requires
   a CANCEL to copy them, so no well-formed CANCEL is affected. The reason is that a `Via` sent-by
   is whatever the sender writes, so §17.2.3's match alone lets anyone who can observe a branch
   stop somebody else's phone ringing.
2. `Invitation::answer` marks the invitation answered *before* the 200 is built. A CANCEL racing an
   answer therefore never puts a 487 behind a 200. The cost is the other order: an answer that
   fails after claiming leaves an invitation that a CANCEL will no longer 487, and the caller's
   Timer B resolves it.

**Two vectors from the first run were strengthened, not rewritten.** Both negatives — the too-late
CANCEL and the replay — passed against a dispatcher with its "already answered" guard removed,
because the `serve` loop survives a stray 487 and the caller's client transaction has already
finished. They now watch the invitation's event stream, which is the only instrument that sees the
difference. Both mutations are recorded in the spec's §9.6. One test also had a genuine defect: a
`let invite = invite(...)` binding shadowed the helper function it later called.

**Left undone, deliberately.**

- `docs/rfc/registry.toml` is fenced for this story, so the RFC 3261 entry's `note` still describes
  the dispatcher's answers without mentioning §9.2. It needs a sentence at integration.
- `ring()` mints its own `To` tag, so a `180` and a `487` for the same invitation carry different
  ones. Harmless — a non-2xx final destroys the early dialog whatever the tag, and transaction
  matching does not use it — but §12.1.1 would rather they agreed. `Invitation::answer` does agree
  with the CANCEL's 200; `ring` cannot without a signature change this story does not own.
- A `serve` loop driven directly off an endpoint receiver, with no `Dispatcher`, still has no UAS
  CANCEL: §6's table answers a bare CANCEL 405. Every path that surfaces an `Invitation` goes
  through the dispatcher, so nothing reachable is affected.

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
