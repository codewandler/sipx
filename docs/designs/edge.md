# Design: Edge / B2BUA

**Status:** resolved — the product is out of scope, the primitive is scheduled · **Pillar:**
Application · **Epic:** `edge` · **Stories:** `C-1`, `C-7`

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
already exists. Both are now built, as two objects rather than one with a switch — see "signalling-
only is absence" below, which `C-7` reopened and answered differently. Nothing here makes sipx a
proxy: a B2BUA is two user agents, which is exactly why it belongs in a user-agent stack while
forking and Record-Route insertion do not.

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
both pending legs, and a crossed target final is ACKed and ended with BYE.

### The off-media role (`C-7`)

`OffMediaCoupling` owns two `Dialog`s instead of two `Call`s. That is the whole design: a `Call`
binds an RTP socket before it can offer anything, so any coupling built from two of them terminates
media whether or not a bridge forwards between them, and no flag on it can be truthful about
§3.1.3. Owning dialogs directly removes the media session rather than switching it off.

What the two legs exchange is the endpoints' own descriptions with one line replaced.
`sipx_sdp::relay::DescriptionRelay` rewrites `o=` — per-dialog identity, and a version that
advances only when the description does (RFC 3264 §8) — and leaves everything else exactly as the
endpoint wrote it: addresses, ports, transport profile, `a=crypto`, `a=fingerprint`, ICE
credentials and candidates, direction, and lines this stack has never heard of. The rewrite is
textual for that last reason. Re-serializing a parsed description normalizes line order, multicast
TTLs and `m=` port counts, which is a liberty a media-terminating endpoint may take with a
description it authored and an off-media element may not take with one it is only carrying.

The lifecycle is the same `CouplingState`: glare 491 before anything is forwarded, BYE mapped onto
the peer, a target 4xx/5xx returned as the source INVITE's own final response, CANCEL withdrawing
the owned target invitation. What the role refuses rather than half-does is the carriers that would
require it to *author* a description: an offerless INVITE in either direction, and the reliable
provisional, which its target INVITE forecloses by not offering `100rel`.

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
- **Resolved, then corrected — signalling-only is absence at the *bridge*, and a distinct owner at
  the *session*.** `C-1` concluded that signalling-only was the absence of `bridge_media`, and
  `C-1`'s own final review found the hole: both legs are `Call`s, a `Call` binds and advertises a
  local media endpoint before it can offer anything, and so "no bridge" only ever meant "media
  terminated and then dropped". `C-7` therefore adds a second owner, `OffMediaCoupling`, and not a
  second signalling machine — the offer/answer and lifecycle state remains the one `CouplingState`,
  which is the part of the original answer that held. Media behaviour still does not create another
  state machine; it does decide what the two legs are made of.

## Acceptance / done

`C-1`: two dialogs are driven as one call by a single policy, an offer relayed from either leg is
answered on the other, either leg ending ends the other, and audio passes between them with no
shared mutable session.

`C-7`: the same policy drives two dialogs with no media session on either leg, the endpoints'
descriptions cross with only their `o=` line replaced, and the packets go from one endpoint to the
other — proved by them arriving on a socket the test bound, at the port the relayed re-INVITE
named.
