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
2. `Invitation::answer` marks the invitation answered immediately *before the 200 is handed to the
   transport* — not before the work that builds it. A CANCEL racing an answer therefore never puts
   a 487 behind a 200, and an answer that fails before sending leaves the invitation cancellable.
   See below; review round 2 found the first version of this wrong.

**The claim placement, which review round 2 corrected.** The first version took the claim at the
top of `Invitation::answer`, before `answer_tagged` had done anything. That was a real defect and
not just a theoretical one: `answer_tagged` parses the offer as its *first* action, so an INVITE
carrying a body `sipx-sdp` cannot read took the claim, failed, and returned `Err(Error::Sdp)` with
**no response sent at all**. The invitation was then permanently un-cancellable — a CANCEL drew its
200, `Pending::cancel` returned `false` because the phase was no longer `Ringing`, no 487 was sent,
and the INVITE transaction never received a final response. It needed no hostile peer, just a
malformed offer and an application that called `answer`.

The fix keeps the ordering guarantee rather than trading it away. The claim is now a hook
(`call::Claim`, a `&(dyn Fn() -> Result<()> + Send + Sync)`) passed from `Invitation::answer` down
through `answer_tagged` into `answer_negotiated`, and invoked at one line: directly above
`endpoint.respond`, after `Dialog::from_request`.

*The error paths proved to precede the response*, and therefore safe to leave unclaimed, are every
fallible expression above that line — `sipx_sdp::parse` (in `answer_tagged`), `negotiated`,
`MediaPort::bind`, the `NoCommonCodec` return, `negotiate_session`, every `ResponseBuilder` step,
and `Dialog::from_request`'s `NoDialog`. *At or after* the response there is exactly one fallible
expression, `endpoint.respond(...)?` itself; everything following it — the retransmit spawn, the
`EventSink`, the `Call` construction — is infallible. That is why a blanket rollback on `Err` was
not acceptable and was not used: `respond` failing is not proof that nothing reached the caller, as
a stream transport can write part of a response before erroring, so that one case stays claimed
deliberately and costs only the Timer B outcome already described.

Mutation-checked in both directions, recorded in spec §9.6: moving the claim earlier fails the new
test with the INVITE unanswered; removing it fails three of the existing ones, so the guarantee it
exists for is still genuinely held. The free `answer` and `answer_ringing` pass no claim — they do
not own an invitation to take.

*One observable change* falls out of the later claim, and it is an improvement rather than a
regression: a CANCEL arriving *while* `answer` is setting media up is now honoured in full — 487
sent, `answer` returns `Error::InvitationCancelled` — where before it drew a bare `200` and the
answer went on to succeed, leaving the caller to notice the 2xx and send a BYE (§9.1's advice for
exactly that race). Both are defensible; the new one matches what the caller asked for.

**Two vectors from the first run were strengthened, not rewritten.** Both negatives — the too-late
CANCEL and the replay — passed against a dispatcher with its "already answered" guard removed,
because the `serve` loop survives a stray 487 and the caller's client transaction has already
finished. They now watch the invitation's event stream, which is the only instrument that sees the
difference. Both mutations are recorded in the spec's §9.6. One test also had a genuine defect: a
`let invite = invite(...)` binding shadowed the helper function it later called.

**One thing for the CHANGELOG at integration.** `Error::InvitationCancelled` is a new variant on
`sipx_call::Error`, which — unlike `EndCause` — is *not* `#[non_exhaustive]`, so a downstream
`match` without a wildcard arm stops compiling. Pre-1.0 (0.8.0) that is a permitted minor-bump
break and it is what past stories did to this enum (`0a42ba6`, `c2ce7e3`), so no gate catches it
and nothing here needs changing; it just wants a line rather than being discovered downstream.

**Left undone, deliberately.**

- `docs/rfc/registry.toml` is fenced for this story, so the RFC 3261 entry's `note` still describes
  the dispatcher's answers without mentioning §9.2. It needs a sentence at integration.
- `ring()` mints its own `To` tag, so a `180` and a `487` for the same invitation carry different
  ones. Harmless — a non-2xx final destroys the early dialog whatever the tag, and transaction
  matching does not use it — but §12.1.1 would rather they agreed. `Invitation::answer` does agree
  with the CANCEL's 200; `ring` cannot without a signature change this story does not own.
  Confirmed as out of scope at review round 2; worth its own story if anyone wants it.
- A `serve` loop driven directly off an endpoint receiver, with no `Dispatcher`, still has no UAS
  CANCEL: §6's table answers a bare CANCEL 405. Every path that surfaces an `Invitation` goes
  through the dispatcher, so nothing reachable is affected.

**Verified on the final commit** (`c2bf15a`): `./scripts/gate.py` — *17 steps, all green*. The
failing-first evidence is the commit order: `57ed3c5` adds `tests/cancel.rs` alone, and there the
suite does not compile — 10 errors, all of them the API this story owes (`Invitation::is_cancelled`
×5, `Invitation::answer` ×3, `Error::InvitationCancelled` ×1, plus the shadowed-`invite` defect
×1). `bc2de4e` is what makes it build and pass.

One gate run failed the API-reference step before this and was **not** a code fault: rustdoc hit a
truncated write (`invalid template: … should have a newline on the last line`) next to a "corrupt
incremental compilation artifact" warning, with the disk at 95%. Clearing this worktree's own
`target/debug/incremental` and `target/doc` cleared it. If that step fails again, check `df -h`
before reading the error.

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
