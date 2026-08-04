# Design: Edge / B2BUA

**Status:** resolved — the product is out of scope, the primitive is scheduled · **Pillar:**
Application · **Epic:** `edge` · **Stories:** `C-1`

## Why

A programmable SIP and media edge — transports, endpoints and routes, with dialog bridging and
selected session-border behaviour — is the natural product built on this stack. It was recorded
here so the layers beneath it were designed with it in mind, and deliberately not scheduled:
building an edge on an unproven core is how a stack acquires workarounds it can never remove.

The open question below has since been answered, so this record now says where the line falls
rather than leaving a reader to guess.

## Approach

**Out of scope for this repository:** listeners, endpoint and route configuration, a location
service, a registrar, call recording, and any form of dial plan. Those are a product, and the
[vision](../vision.md) already rules out "a configuration-driven PBX" — routing engines are things
you build *with* sipx.

**In scope, and scheduled as `C-1` in M9:** the primitive such a product cannot build without
reaching inside sipx — **two dialogs driven as one call**. One policy object owns both legs; an
offer arriving on either leg is relayed to the other and the answer relayed back; a re-INVITE, a
BYE or a failure on one leg has a defined consequence on the other; and the media bridge `M-11`
built carries the audio in between.

RFC 7092 is the vocabulary for this. §3.1.3 (SDP-modifying signalling-only) is the role sipx should
be able to hold with no media path at all, and §3.2.3 (media termination) the one whose media half
already exists. Nothing here makes sipx a proxy: a B2BUA is two user agents, which is exactly why
it belongs in a user-agent stack while forking and Record-Route insertion do not.

### The coupling primitive (`C-1`)

`sipx-call::Coupling` is one owner for two confirmed `Call`s and their two routed request inboxes.
Its driver selects over the legs independently; it never places either call or either media session
behind a shared lock. A BYE accepted on one leg ends the peer with a BYE before the driver returns.
The optional `sipx-media::Bridge` remains two channel-fed forwarding tasks and is rebuilt from the
sessions the calls own, so a stalled media direction cannot hold the signalling driver or its peer.

Offer/answer policy is a separate sans-I/O value, `CouplingState`, because the same decisions are
needed before a confirmed `Call` exists. It owns one negotiation state per leg and names every legal
offer axis: the initial INVITE, reliable provisional, PRACK, UPDATE and re-INVITE. Beginning an
exchange marks the receiving leg as owing an answer and the peer leg as having an offer outstanding;
only completing or failing that exchange clears both. If the other leg is already exchanging an
offer, the arrival is refused 491 while the confirmed driver continues polling that routed inbox.
The request's UAC owns the later randomised retry; retaining an `Incoming` after its final 491 would
attempt to reuse a finished server transaction. The full table and lifecycle mapping are normative in
[`specs/call-coupling.md`](../specs/call-coupling.md).

The confirmed driver also asks the source `Call` whether it can accept an offer before opening the
policy exchange or sending on the other leg. This is a read-only use of the same renegotiation
preparation the call later applies, including codec and keying checks. Consequently a wrong-dialog,
malformed, or well-formed-but-unnegotiable offer is refused entirely on its source leg; it cannot
move the peer and then fail while the source call tries to answer.

The split is deliberate: endpoint routing decides which two user-agent legs to create.
`EarlyCoupling` then owns the inbound `Invitation`/`Ringing`, outbound `Dialing`, and their routed
inboxes; it executes PRACK, early UPDATE, CANCEL and final-status mapping before producing a
`ConfirmedCoupling`. `CouplingState` supplies the shared decisions and `Coupling` takes over the
confirmed calls. Listener configuration, target selection, forking and location lookup remain
product work.

`EarlyCoupling::dial` is the owning initial constructor: it consumes the inbound invitation, maps
the source offer's direction onto a fresh target-leg offer, and retains cancellation responsibility
while the outbound early dialog is created. The two user-agent legs regenerate endpoint-specific
SDP rather than copying addresses or keys. The delayed-offer branch is carried by the ordinary call
types: `ring_offer_early` originates an offer in a reliable provisional,
`dial_early_without_offer` answers it in PRACK, and `Ringing::on_prack` adopts that answer before it
acknowledges the exchange. The coupling consequently does not carry a parallel SDP interpreter or a
second dialog sequence space for those axes. It suspends the target PRACK while a fresh reliable
provisional offer obtains the source leg's PRACK answer; malformed answers and cancellation clean
both pending legs, and a crossed target final is ACKed and ended with BYE. Omitting `bridge_media`
creates no forwarding task but does not make the two terminating `Call` sessions off-path; `C-7`
owns transparent SDP mapping for RFC 7092 section 3.1.3.

## Alternatives considered

- **Build the edge here, as a `sipx-edge` crate.** Rejected. It would put configuration,
  persistence and policy in a repository whose non-negotiables are about a sans-IO protocol core,
  and the pressure to reach around the public API from inside the same workspace is exactly how the
  workarounds this document was written to avoid get in.
- **Ship nothing for it and let the product own the dialog coupling too.** Rejected: coupling two
  dialogs correctly needs the offer/answer state of both, which is `sipx-call`'s to know. Rebuilding
  it outside means a second, diverging model of a state sipx already tracks — and it is the state
  whose edge cases (an early offer, a glare-losing re-INVITE) are hardest to get right.

## Risks & resolved questions

- **Resolved:** whether this belongs in this repository at all. It does not; a separate platform
  ([sipx-clstr](https://github.com/codewandler/sipx-clstr)) builds the proxy, registrar and cluster
  roles on this kernel, and its
  [upstream ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md) records
  each kernel gap it depends on as a story here.
- The application side drew the line differently: the programmable call server hosting webhook-
  and TypeScript-driven handlers over the `sipx.app.v1` contract lives *in this workspace* as
  `crates/sipx-app` — a leaf crate under the [app-host](app-host.md) design's ground rules. A
  B2BUA product forwards other people's calls and stays out; the app host terminates its own,
  which is exactly the user-agent business this repository is in. The contract's kernel half is
  the [app-sdk design](app-sdk.md).
- **Resolved — protocol policy is data, application policy stays outside.** `CouplingState` is a
  concrete state table, not a callback trait. Whether glare is 491, whether a CANCEL may cross, and
  which answer closes which offer are protocol invariants, so making them overridable would let an
  application configure an invalid B2BUA. Routing, admission and target choice are not fields on
  it; the application performs those before giving the two legs to the coupling.
- **Resolved — signalling-only is absence, not a second mode.** A coupling has no bridge unless
  `bridge_media` is called. The offer/answer and lifecycle state is identical either way, which is
  RFC 7092's useful distinction: media behaviour classifies the B2BUA but does not create another
  signalling machine. The application relays the descriptions selected by `CouplingState` directly
  when it stays off the media path; attaching the bridge terminates the two negotiated sessions.

## Acceptance / done

`C-1`: two dialogs are driven as one call by a single policy, an offer relayed from either leg is
answered on the other, either leg ending ends the other, and audio passes between them with no
shared mutable session.
