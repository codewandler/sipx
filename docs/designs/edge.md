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

## Alternatives considered

- **Build the edge here, as a `sipx-edge` crate.** Rejected. It would put configuration,
  persistence and policy in a repository whose non-negotiables are about a sans-IO protocol core,
  and the pressure to reach around the public API from inside the same workspace is exactly how the
  workarounds this document was written to avoid get in.
- **Ship nothing for it and let the product own the dialog coupling too.** Rejected: coupling two
  dialogs correctly needs the offer/answer state of both, which is `sipx-call`'s to know. Rebuilding
  it outside means a second, diverging model of a state sipx already tracks — and it is the state
  whose edge cases (an early offer, a glare-losing re-INVITE) are hardest to get right.

## Risks & open questions

- **Resolved:** whether this belongs in this repository at all. It does not; a separate platform
  ([sipx-clstr](https://github.com/codewandler/sipx-clstr)) builds the proxy, registrar and cluster
  roles on this kernel, and its
  [upstream ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md) records
  each kernel gap it depends on as a story here.
- A second downstream product does the same for the application side: a programmable call server
  (working name `sipx-app`) hosting webhook- and TypeScript-driven call handlers over the
  `sipx.app.v1` contract. The kernel's half of that boundary is the [app-sdk design](app-sdk.md).
- Open: how much of the coupling policy is data and how much is a trait. A B2BUA that can only be
  configured is a PBX; one that is only a trait is a tutorial. `C-1` has to pick.
- Open: whether a signalling-only B2BUA (RFC 7092 §3.1.2/§3.1.3, no media path) is worth a separate
  mode, or falls out of the same coupling with the bridge left unattached.

## Acceptance / done

`C-1`: two dialogs are driven as one call by a single policy, an offer relayed from either leg is
answered on the other, either leg ending ends the other, and audio passes between them with no
shared mutable session.
