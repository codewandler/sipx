# Two-dialog call coupling

**Status:** normative · **Story:** C-1 · **Crates:** `sipx-call`, `sipx-media` ·
**RFCs:** 3261, 3264, 3262, 3311, 7092

## 1. Scope

This is the primitive a back-to-back user agent is built from, not a listener, router, registrar or
dial plan. RFC 7092 supplies the taxonomy: the same signalling policy can be used without a media
bridge (§3.1.3) or with the channel-based terminating bridge (§3.2.3).

The primitive has three layers. `CouplingState` is sans-I/O and describes every legal offer
carrier. `EarlyCoupling` owns an inbound `Invitation`/`Ringing`, an outbound `Dialing`, and both
routed inboxes until cancellation, refusal, or confirmation. `Coupling` then owns the two confirmed
`Call`s. The application creates and routes the two user-agent legs; the coupling owns them once
joined.

## 2. Types and ownership

| Type | Values / ownership | Purpose |
|---|---|---|
| `Leg` | `One`, `Two` | Names one axis without implying caller/callee; either side may offer |
| `OfferAxis` | `InitialInvite`, `ReliableProvisional`, `Prack`, `Update`, `Reinvite` | The five legal carriers this policy distinguishes |
| `NegotiationState` | `Idle`, `Offering(axis)`, `Answering(axis)` | One value **per leg**, never shared between them |
| `CouplingState` | two negotiation states and confirmation state | Sans-I/O policy for offer/answer and lifecycle |
| `EarlyCoupling` | `Invitation`, `Ringing`, `Dialing`, two request receivers | Pending-dialog driver through failure or confirmation |
| `ConfirmedCoupling` | `Coupling` plus the two request receivers | Ownership-preserving early-to-confirmed handoff |
| `Coupling` | two `Call`s, two request receivers, optional `Bridge` | Confirmed-dialog driver and sole owner of both calls |

`Coupling` does not expose either call by value. Borrowed inspection does not transfer ownership.
Media remains owned by its call and moves through the bridge's bounded channels; neither call waits
on the other's media worker.

## 3. Offer/answer table

`begin_offer(source, axis)` is the one entry point for an offer, whether its carrier is the initial
INVITE, a reliable provisional, PRACK, UPDATE or re-INVITE.

| Source state | Peer state | Result | New source / peer state |
|---|---|---|---|
| idle | idle | `Relay` | answering(axis) / offering(axis) |
| offering(axis) | any | refuse **491** | unchanged |
| answering(axis) | any | refuse 500 | unchanged |
| idle | non-idle | refuse **491** | unchanged |

`complete(source)` records that the peer's answer returned on the same axis and returns both legs
to idle. `fail(source)` returns both legs to idle without an answer.

The states are separate even while their transition is coupled. One shared `busy` flag cannot say
which leg owes an answer, which leg has an outstanding offer, or where the answer must return.

## 4. Glare

RFC 3261 §14.2 assigns 491 to a re-INVITE collision. The coupling applies the same collision
decision before forwarding any offer to a leg on which it already has an exchange outstanding.
The arriving leg receives 491 while the outgoing exchange remains in progress. The confirmed
driver keeps polling that leg's routed inbox while awaiting the outgoing final response, rather
than allowing the crossed request to sit in the queue until the collision has disappeared.
Non-offer requests read during that mutable exchange are deferred in a 16-entry FIFO. At capacity,
the driver stops reading and leaves backpressure at the already-bounded routed inbox.

The final 491 ends the incoming server transaction, so replaying that `Incoming` later would be a
protocol error: RFC 3261 §14.1 assigns the randomised retry to the request's UAC. A fresh retry
after the outstanding exchange settles enters `begin_offer` as a new transaction and is relayed.
No timer is read and no stale network request is retained by this state machine.

### 4.1 Executable early axes and remaining gap

The application creates the two initial INVITE legs and their `Ringing`/`Dialing` state before the
ownership handoff; `EarlyCoupling` does not claim to relay those already-sent requests or
provisionals. From the handoff it executes the offer shape the present public call API can create:
both reliable 183 answers are PRACKed, and an offer-carrying UPDATE on either early dialog is relayed
before its source receives an answer. It keeps polling the far inbox during that UPDATE, so its
glare decision is live too. Initial-INVITE relay by the owning object therefore remains part of the
all-axis acceptance gap as well.

Two legal RFC 3262 shapes remain policy-only: an offer that *originates* in a reliable provisional,
and its answer in PRACK (or an offer originating in PRACK). `ring_early` only answers the initial
INVITE offer, `Dialing` only interprets provisional SDP as that answer, and `Ringing::on_prack`
settles RAck without SDP negotiation. Executing those axes requires extending those three APIs to
surface an offer-bearing provisional and negotiate a PRACK body; naming `OfferAxis` does not claim
that wire support. Consequently C-1's all-axis acceptance item remains open.

## 5. Lifecycle table

| Event | Peer not confirmed | Peer confirmed |
|---|---|---|
| outbound final 4xx/5xx | same final status on inbound INVITE | n/a |
| BYE on either leg | n/a | accept it, then BYE the peer with the received SIP cause when present |
| inbound CANCEL while inbound INVITE is pending | send CANCEL on outbound leg | BYE the peer whose 2xx crossed cancellation |
| late CANCEL after both dialogs confirmed | n/a | acknowledge only; it cannot erase either dialog |
| local driver/inbox failure | cancel pending peer | BYE confirmed peer with a local failure cause |

A matching CANCEL after a final response does not tear a dialog down (RFC 3261 §9.2). The coupling
therefore never translates an answered-leg CANCEL into silence: either the two confirmed calls stay
owned, or an independent terminal event ends both through BYE.

## 6. Media policy

There is no signalling-only enum. A fresh coupling has no bridge. `bridge_media` attaches the
existing channel-based bridge to the two sessions and reports whether it transcodes. Dropping or
closing the coupling drops the bridge before the calls and stops both forwarding tasks.

An application implementing RFC 7092 §3.1.3 follows the `CouplingState` actions and relays SDP while
leaving the bridge unattached. An application implementing §3.2.3 negotiates the two sessions at
the B2BUA and calls `bridge_media`.

## 7. Test vectors

| Vector | Input | Required result |
|---|---|---|
| O1 | each `OfferAxis` from two idle legs | relay; source answers, peer offers; completion returns both idle |
| O2 | offer from leg two while leg one's relay is outstanding | 491; no forwarded request |
| O3 | complete O2's outstanding exchange; the remote UAC sends a new retry | the new transaction is relayed |
| O4 | a second offer while this coupling owes an answer on its source leg | 500; state unchanged |
| L1 | BYE on leg one of two confirmed calls | leg one answers 200; leg two receives BYE; driver ends |
| L2 | CANCEL before outbound confirmation | outbound action is CANCEL |
| L3 | CANCEL after both dialogs confirm | no teardown action and both dialogs remain owned |
| L4 | outbound 486 or 503 before peer confirmation | reject the peer INVITE with the same status |
| L5 | final failure after peer confirmation | BYE the peer; never synthesize an INVITE response |
| L6 | inbound CANCEL while `EarlyCoupling` owns a pending outbound INVITE | outbound receives CANCEL; driver returns cancelled |
| E1 | reliable answers on both early legs, followed by PRACK and an offer-carrying UPDATE | both PRACKs complete; UPDATE offer and answer cross before either INVITE final |
| M1 | fresh coupling | no media bridge and no forwarding task |
| M2 | `bridge_media` | audio crosses in both directions over the existing bridge |
