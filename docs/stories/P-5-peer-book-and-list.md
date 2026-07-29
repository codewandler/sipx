---
id: P-5
title: List what can be called with `sipx peers`
pillar: Phone
status: ready
priority: 15
design: docs/designs/discovery.md
epic: discovery
areas: [sipx-cli]
note: the epic's first story — a peer book and one command, with no protocol work
---

# List what can be called with `sipx peers`

## Goal
Give sipx an answer to "who is there to call?". A peer book the user can put names in, and one
command that prints them in the machine-readable form `P-1` established — the foundation the
registrar and local-link sources later merge into.

## Acceptance
- [ ] `sipx peers` lists known peers in both the human and machine-readable forms `P-1` set for
      every other command, and exits non-zero with a typed error rather than an empty list when it
      cannot read its source.
- [ ] Each entry carries **which source it came from**, even though there is only one source in
      this story. A list that flattens a typed-in peer and a multicast response into one shape
      cannot be extended by `S-24` and `T-24` without breaking every script that read it.
- [ ] The peer book's location and format are decided and written down — the design leaves this
      open on purpose. Whatever is chosen must be readable and writable from a shell script.
- [ ] A peer entry carries enough to dial it: a URI, and a name that `P-6` can look up.
- [ ] Nothing in this story consults the network. The registrar (`S-24`) and the local link
      (`T-24`) are separate stories, and this one must be useful with neither.
- [ ] Failing-first test: `a_peer_written_to_the_book_is_listed_by_name`.

## Progress
- Not started.

## Notes
- First story of the `discovery` epic; see [the design](../designs/discovery.md) for why the peer
  book comes before either protocol source. It needs no protocol work, it is what makes `P-6`
  expressible, and it is what the other two degrade into when a registrar refuses a subscription
  or a network blocks multicast.
- **Do not let this become a dial plan.** The vision's non-goals are explicit that routing engines
  and dial plans are built *with* sipx, not into it. This book is consulted when a person names a
  peer, never while routing an inbound INVITE.
- `RFC 3263` is implemented and is *not* this: it resolves a URI you have already chosen to a host
  and transport. This story produces the URIs in the first place.
