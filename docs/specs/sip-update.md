# Spec: the UPDATE method

**Status:** normative · **Crates:** `sipx-sip`, `sipx-call` · **Stories:** S-19 · **Design:**
[sip-ua](../designs/sip-ua.md)

## 1. Normative references

- **RFC 3311** — the UPDATE method. §4 (determining support), §5.1 (sending), §5.2 (receiving).
- RFC 3261 §12.2 (in-dialog requests), §13.2 (offer/answer in an INVITE), §20.5 (`Allow`).
- RFC 3262 §5 — an SDP answer may only be carried in a **reliable** provisional response.
- RFC 3264 — the offer/answer model: one offer at a time, per direction.
- RFC 4028 §7.4 — a session refresh SHOULD use UPDATE when the peer is known to support it.

**Out of scope:** early media as an audible stream (`C-2`), UPDATE across two coupled dialogs
(`C-1`), and anything that touches a socket in `sipx-sip`.

## 2. What UPDATE is for

A re-INVITE renegotiates a session that is already up. It cannot renegotiate one that is not:
until the INVITE has a final response there is a transaction in progress, and a second INVITE
inside it is not a thing SIP has. UPDATE is that missing request — an in-dialog renegotiation
that does not disturb the INVITE transaction it runs alongside.

RFC 3311 §5.1: it "MAY be sent for both early and confirmed dialogs, and MAY be sent by either
caller or callee". For a *confirmed* dialog the same section says a re-INVITE is still
RECOMMENDED, because an UPDATE must be answered promptly and leaves no window in which a user
could be asked to approve the change. So sipx keeps the re-INVITE as the confirmed-dialog
renegotiation (`M-8`) and uses UPDATE for the two things a re-INVITE cannot do well:

1. renegotiating an **early** dialog, and
2. **refreshing** a session timer, which changes nothing and only needs to be seen (RFC 4028
   §7.4).

## 3. There has to be a dialog

UPDATE is an in-dialog request. Before a dialog exists there is nothing to address: no remote
target, no `To` tag, no `CSeq` space of our own. So the early-dialog case is only reachable once
a provisional response has established one (RFC 3261 §12.1), which is why `S-12` (100rel) had to
land first.

It goes further than addressing. An UPDATE that carries an **offer** is only legal when the
offer/answer state is idle (§5.1, and §5.2's mirror image below), and in an early dialog that
means the INVITE's own offer must already have been answered. RFC 3262 §5 allows exactly one
way to do that before the 200: **the answer travels in a reliable provisional response.** RFC
3311 §4 assumes precisely this arrangement when it says that a reliable provisional carrying SDP
"SHOULD contain an `Allow` header field that lists the UPDATE method".

Hence `sipx-call`'s two ringing entry points:

| Function | Provisional | Offer/answer after it | Early UPDATE with an offer |
|---|---|---|---|
| `ring` | no body | UAS still owes an answer | refused, **500** (§6) |
| `ring_early` | the SDP answer, sent reliably | complete | accepted |

`ring_early` requires 100rel from the peer and fails if the peer did not offer it: an answer in
an unreliable provisional is forbidden outright, and sending one anyway would leave the two
sides disagreeing about which description is in force with no way to notice.

## 4. Advertising it (§4)

A peer may only decide to send an UPDATE if it has been told the method exists here. §4 puts
that in `Allow`, on three messages:

- the INVITE (a UAC "SHOULD also include an `Allow` header field in the INVITE request, listing
  the method UPDATE"),
- a reliable provisional carrying SDP, and
- the 2xx.

One constant, `sipx_sip::update::ALLOW`, is the list every one of those writes, because a copy
that drifts is a peer that silently never renegotiates early and no test that would catch it.

The reverse reading is `sipx_sip::update::peer_allows`, over the `Allow` of whatever message
introduced the peer: the INVITE for a UAS, the 2xx (or a reliable provisional) for a UAC. It is
the only permission that matters — §7 below turns it into the choice between UPDATE and a
re-INVITE for a refresh.

## 5. The state a dialog keeps

`sipx_sip::update::Negotiation` is pure and holds three booleans:

| Field | Set when | Cleared when |
|---|---|---|
| `offered` | we send an offer (INVITE, UPDATE or PRACK) | its answer arrives |
| `owed` | an offer arrives that we have not answered | we send the answer |
| `in_progress` | an UPDATE is accepted for processing | its final response is sent |

`offered` and `owed` are RFC 3264's one-offer-at-a-time rule seen from each end. They are *not*
the same flag: a dialog can owe an answer and have no offer outstanding, and the two produce
different refusals below. `in_progress` is about transactions, not descriptions, and applies to
an UPDATE with no body at all.

## 6. Receiving an UPDATE (§5.2)

Three refusals, checked in this order. They are three different answers on purpose — the whole
value of the distinction is that a peer's retry logic depends on it.

| # | Condition | Response | What it tells the peer |
|---|---|---|---|
| 1 | `in_progress` — a previous UPDATE has no final response yet | **500** + `Retry-After: 0..10` | you are too early; the same request will work shortly |
| 2 | offer received while `offered` — our own offer is unanswered | **491** | we collided; back off per RFC 3261 §14.1 and retry |
| 3 | offer received while `owed` — we have not answered an offer already in hand | **500** + `Retry-After: 0..10` | your request was well formed and badly timed |

Rule 1 applies to every UPDATE. Rules 2 and 3 apply only to one that carries an offer: an
offerless UPDATE — the session refresh of §7 — changes no description and cannot collide with
one.

Collapsing these into a single failure loses the only thing a peer can act on. 491 means glare,
which both sides resolve by waiting a *randomised* interval and retrying; 500 with `Retry-After`
means the request was fine and the moment was not. A peer told 500 when it should have been told
491 backs off wrongly; a peer told 491 when it should have been told 500 retries into the same
wall.

`Retry-After` is "a randomly chosen value between 0 and 10 seconds" (§5.2). The number is
supplied by the caller, not generated in `sipx-sip`: the core reads no clock and no entropy
source. `sipx_sip::update::RETRY_AFTER_MAX_SECS` is the bound.

Once accepted:

- the description is renegotiated and the answer goes in the 2xx (§5.2: the UAS "MUST adjust the
  session parameters accordingly and generate an answer in the 2xx response");
- an unacceptable description is refused **488** and **the dialog survives** — the same rule
  `M-8` applies to a re-INVITE, and for the same reason: a renegotiation that fails must leave
  what was working alone;
- UPDATE is a target refresh request, so a `Contact` on it replaces the dialog's remote target
  (§5.1, RFC 3261 §12.2.2);
- RFC 4028: it refreshes the session timer, whatever it was sent for.

There is no ACK. UPDATE is a non-INVITE transaction, so its final response is retransmitted by
the transaction layer rather than by the TU — unlike the 2xx to a re-INVITE, which
`M-8` has to resend itself until the ACK arrives.

## 7. Refreshing a session with it (RFC 4028 §7.4)

> "If a UAC knows that its peer supports the UPDATE method, it is RECOMMENDED that UPDATE be
> used instead of a re-INVITE."

So the refresher picks by the peer's `Allow`:

| Peer's `Allow` lists UPDATE | Refresh |
|---|---|
| yes | UPDATE, **with no body** |
| no | re-INVITE, exactly as `S-11` sends it |

No body, because a refresh changes nothing: the session description in force is still in force,
and re-offering it would start an offer/answer exchange that can fail, on a timer whose only job
is to prove the far end is alive. It also keeps the refresh clear of §6's rules 2 and 3 — a
refresh must never be refused for a reason that has nothing to do with liveness.

The `Session-Expires` and `Min-SE` on the refresh are the ones `S-11` already writes, and the
2xx is read back the same way (`session::adopt`). A refresh that is refused leaves the deadline
where it was, so the next attempt is the retry; a refresh that draws 408 or 481 means the dialog
is gone and the call is torn down. Both are `S-11`'s rules, unchanged — only the method moved.

## 8. Test vectors

Derived from §§4–7 and implemented in `crates/sipx-sip/src/update.rs` (pure) and
`crates/sipx-call/tests/update.rs` (on the wire).

### 8.1 The three refusals

| State | UPDATE has an offer | Expected |
|---|---|---|
| idle | yes | accept |
| idle | no | accept |
| `in_progress` | no | 500, `Retry-After` |
| `in_progress` | yes | 500, `Retry-After` |
| `offered` | yes | 491 |
| `offered` | no | accept |
| `owed` | yes | 500, `Retry-After` |
| `owed` | no | accept |
| `offered` + `in_progress` | yes | 500 — rule 1 is checked first |

### 8.2 Sending

| State | May send an offer in an UPDATE |
|---|---|
| idle | yes |
| `offered` | no |
| `owed` | no |
| `in_progress` | yes — it is the *peer's* transaction, not an offer of ours |

### 8.3 `Allow`

| Header value | `peer_allows` |
|---|---|
| `INVITE, ACK, CANCEL, BYE, OPTIONS, UPDATE` | true |
| `INVITE, ACK, BYE` | false |
| `invite,update` | true — tokens are case-insensitive (RFC 3261 §7.3.1) |
| `UPDATEX` | false — a token, not a substring |
| *(absent)* | false — §4 makes silence mean "do not" |
