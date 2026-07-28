---
id: S-19
title: Implement the UPDATE method
pillar: Signalling
status: backlog
priority:
design:
epic: conformance
areas: [sipx-sip, sipx-call]
note: M9 · RFC 3311 · the last session-integrity gap; 100rel unblocked it
---

# Implement the UPDATE method

## Goal
Let a session be renegotiated before it has been answered, and let a session-timer refresh use the
method RFC 4028 recommends rather than the only one sipx has.

## Acceptance
- [ ] UPDATE is sent and received in an early dialog as well as a confirmed one — RFC 3311 §5.1:
      "It MAY be sent for both early and confirmed dialogs." There must be a dialog: the request is
      in-dialog, so a UAC has nothing to address before a provisional established one.
- [ ] `Allow` on an INVITE and on its responses lists UPDATE (§4), because the peer's `Allow` is how
      the other side is permitted to decide it may use UPDATE at all.
- [ ] The three refusals of §5.2 are implemented as three *different* responses, not one:
      - a second UPDATE arriving before the first has a final response → **500** with a
        `Retry-After` "randomly chosen value between 0 and 10 seconds";
      - an offer arriving while this side has an offer outstanding → **491**;
      - an offer arriving while this side owes an answer → **500**.
      Collapsing these loses the distinction between "you are too early" and "we collided", and only
      the second is safe for the peer to retry immediately.
- [ ] An unacceptable session description is refused with **488**, and the dialog survives it (§5.2).
- [ ] A session-timer refresh uses UPDATE when the peer's `Allow` contained it and a re-INVITE
      otherwise (RFC 4028 §7.4: "If a UAC knows that its peer supports the UPDATE method, it is
      RECOMMENDED that UPDATE be used instead of a re-INVITE"). `S-11`'s refresh tests must still
      pass unchanged on the re-INVITE path.
- [ ] A confirmed-dialog renegotiation still prefers a re-INVITE (§5.1), so `M-8` behaviour does not
      change by accident.
- [ ] The RFC registry entry for RFC 3311 moves off `syntax only`, and RFC 3262's and RFC 4028's
      notes lose the sentences that say UPDATE is unavailable.
- [ ] Failing-first test: `an_update_renegotiates_a_session_before_it_is_answered`.

## Progress
- Not started. RFC 3311 is `syntax only` in `compliance.md`: `Method::Update` exists and nothing
  handles it.

## Notes
- The 491-versus-500 split is the part that is easy to get wrong, and it is the part a peer's retry
  logic depends on. 491 means glare — back off and retry. 500 with `Retry-After` means the request
  was well formed and badly timed.
- Scope: UPDATE as a renegotiation and refresh method. Using it to *unmute* an early media stream is
  `C-2`'s problem, and using it across two coupled dialogs is `C-1`'s.
