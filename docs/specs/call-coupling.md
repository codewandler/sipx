# Two-dialog call coupling

**Status:** normative · **Story:** C-1 · **Crates:** `sipx-call`, `sipx-media` ·
**RFCs:** 3261, 3264, 3262, 3311, 7092

## 1. Scope

This is the primitive a back-to-back user agent is built from, not a listener, router, registrar or
dial plan. RFC 7092 supplies the taxonomy: this story owns the media-terminating call role and its
optional channel-based bridge (§3.2.3). `C-7` owns the distinct §3.1.3 role whose SDP mapping leaves
sipx entirely off the media path.

The primitive has three layers. `CouplingState` is sans-I/O and describes every legal offer
carrier. `EarlyCoupling` owns an inbound `Invitation`/`Ringing`, an outbound `Dialing`, and both
routed inboxes until cancellation, refusal, or confirmation. `Coupling` then owns the two confirmed
`Call`s. `EarlyCoupling::dial` consumes the inbound invitation before it creates the outbound
initial INVITE, so the object owns the first relayed axis as well as all later ones. Routing and
target selection remain application policy and are inputs to that constructor.

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

For a confirmed request, the coupling performs three read-only checks before `begin_offer` and
before sending anything on the peer leg: the request matches the source dialog, its CSeq is fresh,
and the source `Call` can answer the offer. That last check is the call's own renegotiation
preparation without its state-changing half: SDP must parse, contain usable active audio, negotiate
one of the call's codecs, and use a keying mode that supports renegotiation. A wrong dialog receives
481; an unacceptable description receives 488. Neither opens coupling state or traffic on the peer
leg. The eventual source `Call::handle` applies the same preparation, so preflight and acceptance
cannot become two definitions of usable media.

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

### 4.1 Executable early axes

`EarlyCoupling::dial` reads the source initial offer's audio direction before consuming its
`Invitation`, preserves that direction on the target leg, and creates the target INVITE while it is
the sole owner. Direction is endpoint-relative: if the source offers `sendonly`, the coupling
receives on that leg and must offer `sendonly` on the target leg to forward the same flow. Inverting
it would make the coupling receive on both legs. SDP addresses, ports, keys and ICE credentials are
deliberately regenerated per leg: this is a pair of user agents, not a proxy copying endpoint
coordinates between networks.

The call layer exposes both halves of RFC 3262 section 5's delayed-offer shape.
`dial_early_without_offer` sends an offerless INVITE. An SDP-bearing reliable provisional is
therefore an offer rather than an answer; the `Dialing` prepares its negotiated answer and owns the
same media session the confirmed `Call` later inherits. In the other role, `ring_offer_early` puts
an offer in a reliable provisional and `Ringing::on_prack` validates and adopts the answer before
returning a 2xx. Both paths use the early dialog's own sequence space.

`EarlyCoupling` composes those halves without inventing an answer. A target reliable provisional
offer is staged on leg two, relayed as a fresh reliable provisional offer on leg one, and its target
PRACK is held. Only a matching source PRACK with a usable answer releases the target PRACK carrying
the corresponding locally negotiated answer. The per-leg exchange remains non-idle until that
causal chain completes. A missing or malformed source answer receives 488, refuses the source
INVITE, and cancels the pending target invitation without releasing its PRACK. Source cancellation
does the same target cleanup. If a target 2xx crossed the held PRACK, cleanup ACKs that response and
ends its now-confirmed dialog with BYE; it never sends a stale CANCEL or abandons the 2xx.

After either initial shape has settled, offer-carrying UPDATEs on either early dialog are relayed
before its source receives an answer. The driver keeps polling the far inbox during that UPDATE,
so its glare decision remains live.

An unattached media bridge means no forwarding task, but each `Call` still binds and advertises a
local media endpoint. That is media termination without forwarding, not RFC 7092 section 3.1.3's
off-media-path role. A transparent SDP mapping is required before signalling-only coupling is a
truthful claim.

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

There is no signalling-only enum. A fresh coupling has no forwarding bridge, but its two `Call`s
remain media-terminating sessions. `bridge_media` attaches the existing channel-based bridge to
those sessions and reports whether it transcodes. Dropping or closing the coupling drops the bridge
before the calls and stops both forwarding tasks.

An application implementing RFC 7092 §3.2.3 negotiates the two sessions at the B2BUA and calls
`bridge_media`. Section 3.1.3 remains open: it needs a distinct transparent SDP mapping whose calls
do not bind or advertise local media endpoints.

## 7. Test vectors

| Vector | Input | Required result |
|---|---|---|
| O1 | each `OfferAxis` from two idle legs | relay; source answers, peer offers; completion returns both idle |
| O2 | offer from leg two while leg one's relay is outstanding | 491; no forwarded request |
| O3 | complete O2's outstanding exchange; the remote UAC sends a new retry | the new transaction is relayed |
| O4 | a second offer while this coupling owes an answer on its source leg | 500; state unchanged |
| O5 | syntactically valid offer names the wrong source dialog | source receives 481; peer leg receives no request |
| O6 | malformed SDP, no common codec, or unsupported renegotiation keying | source receives 488; peer leg receives no request and coupling state remains idle |
| L1 | BYE on leg one of two confirmed calls | leg one answers 200; leg two receives BYE; driver ends |
| L2 | CANCEL before outbound confirmation | outbound action is CANCEL |
| L3 | CANCEL after both dialogs confirm | no teardown action and both dialogs remain owned |
| L4 | outbound 486 or 503 before peer confirmation | reject the peer INVITE with the same status |
| L5 | final failure after peer confirmation | BYE the peer; never synthesize an INVITE response |
| L6 | inbound CANCEL while `EarlyCoupling` owns a pending outbound INVITE | outbound receives CANCEL; driver returns cancelled |
| E1 | reliable answers on both early legs, followed by PRACK and an offer-carrying UPDATE | both PRACKs complete; UPDATE offer and answer cross before either INVITE final |
| E2 | source initial INVITE carries `sendonly`; owning coupling creates target leg | target initial INVITE carries `sendonly`; both pending legs remain owned by the coupling |
| E3 | source offerless INVITE; target reliable 183 carries an offer | source reliable 183 carries a fresh offer; no target PRACK leaves before the source PRACK answer; then target PRACK carries the answer and both early sessions are retained |
| E4 | E3 with malformed SDP in the source PRACK | source PRACK and INVITE receive 488; target receives CANCEL and no PRACK |
| E5 | source CANCEL after both reliable offers but before source PRACK | source CANCEL receives 200 and INVITE receives 487; target receives CANCEL and no PRACK |
| E6 | target 2xx crosses the held PRACK, then the E4 failure occurs | target receives ACK then BYE; no target PRACK or stale CANCEL leaves |
| M1 | fresh coupling | no media bridge and no forwarding task |
| M2 | `bridge_media` | audio crosses in both directions over the existing bridge |
