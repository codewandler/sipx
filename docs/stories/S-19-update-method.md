---
id: S-19
title: Implement the UPDATE method
pillar: Signalling
status: in-progress
priority: 1
design: docs/specs/sip-update.md
epic: conformance
areas: [sipx-sip, sipx-call]
note: M9 · RFC 3311 · the last session-integrity gap; 100rel unblocked it
---

# Implement the UPDATE method

## Goal
Let a session be renegotiated before it has been answered, and let a session-timer refresh use the
method RFC 4028 recommends rather than the only one sipx has.

## Acceptance
- [x] UPDATE is sent and received in an early dialog as well as a confirmed one — RFC 3311 §5.1:
      "It MAY be sent for both early and confirmed dialogs." There must be a dialog: the request is
      in-dialog, so a UAC has nothing to address before a provisional established one.
- [x] `Allow` on an INVITE and on its responses lists UPDATE (§4), because the peer's `Allow` is how
      the other side is permitted to decide it may use UPDATE at all.
- [x] The three refusals of §5.2 are implemented as three *different* responses, not one:
      - a second UPDATE arriving before the first has a final response → **500** with a
        `Retry-After` "randomly chosen value between 0 and 10 seconds";
      - an offer arriving while this side has an offer outstanding → **491**;
      - an offer arriving while this side owes an answer → **500**.
      Collapsing these loses the distinction between "you are too early" and "we collided", and only
      the second is safe for the peer to retry immediately.
- [x] An unacceptable session description is refused with **488**, and the dialog survives it (§5.2).
- [x] A session-timer refresh uses UPDATE when the peer's `Allow` contained it and a re-INVITE
      otherwise (RFC 4028 §7.4: "If a UAC knows that its peer supports the UPDATE method, it is
      RECOMMENDED that UPDATE be used instead of a re-INVITE"). `S-11`'s refresh tests must still
      pass unchanged on the re-INVITE path.
- [x] A confirmed-dialog renegotiation still prefers a re-INVITE (§5.1), so `M-8` behaviour does not
      change by accident.
- [x] The RFC registry entry for RFC 3311 moves off `syntax only`, and RFC 3262's and RFC 4028's
      notes lose the sentences that say UPDATE is unavailable.
- [x] Failing-first test: `an_update_renegotiates_a_session_before_it_is_answered`.

## Progress
- Done. `sipx-sip::update` holds the pure half — the `Allow` contract of §4, the three-boolean
  offer/answer state and §5.2's three refusals — and `sipx-call` drives it from
  `crates/sipx-call/src/update.rs`, `Call::on_update`/`Call::update`, and `Ringing`.
  `docs/specs/sip-update.md` is the spec the tests are derived from.
- **The early half needed 100rel for a second reason.** Not only to have a dialog to address:
  §5.1 will not let an UPDATE carry an offer while an offer/answer exchange is open, and before
  the 200 the only place an answer may go is a reliable provisional (RFC 3262 §5). So
  `ring_early` answers the INVITE's offer in the provisional, and `answer_early` hands the port
  it bound to the `Call` — a second port would make the 200 contradict the 183, and the 200
  therefore carries no description at all.
- **Two of the three refusals are 500**, which is what RFC 3311 §5.2 says: an UPDATE arriving
  before a previous one is answered, and an offer arriving while an answer is owed, both draw a
  500 with a random `Retry-After`. They are kept as separate decisions anyway, because the
  reason is worth logging and a caller may want to tell them apart. The distinction that
  matters on the wire is 491 against 500: glare resolves by RFC 3261 §14.1's randomised
  back-off, and 500 with `Retry-After` says the request was fine and the moment was not.
- **Only the third refusal is reachable end to end.** sipx dispatches in-dialog requests
  sequentially through `&mut self`, so this side can never be mid-way through answering one
  UPDATE when the next arrives, and can never have an offer outstanding while `handle` runs.
  Rules 1 and 2 are therefore covered by the vectors in `sipx_sip::update`, and rule 3 has a
  wire test as well. That is a property of sipx's dispatch, not of the peer's: both rules stay
  implemented because the state they read is real and a concurrent dispatcher (`C-4`) would
  reach them.
- **The refresh carries no body.** Re-offering an unchanged description would put a liveness
  check under §5.2's offer/answer rules, where it could be refused for a reason that has
  nothing to do with liveness. `S-11`'s re-INVITE path is unchanged and still runs whenever the
  peer's `Allow` does not list UPDATE.
- The `Allow` value is one constant, `sipx_sip::update::ALLOW`, used by the INVITE, its
  provisional and 2xx responses, re-INVITEs, UPDATEs and the user agent's OPTIONS answer. §4
  makes that header the peer's only permission, so a second copy that drifts is a peer that
  silently never renegotiates.

## Notes
- The 491-versus-500 split is the part that is easy to get wrong, and it is the part a peer's retry
  logic depends on. 491 means glare — back off and retry. 500 with `Retry-After` means the request
  was well formed and badly timed.
- Scope: UPDATE as a renegotiation and refresh method. Using it to *unmute* an early media stream is
  `C-2`'s problem, and using it across two coupled dialogs is `C-1`'s.
