---
id: P-5
title: List what can be called with `sipx peers`
pillar: Phone
status: in-progress
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
- [x] `sipx peers` lists known peers in both the human and machine-readable forms `P-1` set for
      every other command, and exits non-zero with a typed error rather than an empty list when it
      cannot read its source.
- [x] Each entry carries **which source it came from**, even though there is only one source in
      this story. A list that flattens a typed-in peer and a multicast response into one shape
      cannot be extended by `S-24` and `T-24` without breaking every script that read it.
- [x] The peer book's location and format are decided and written down — the design leaves this
      open on purpose. Whatever is chosen must be readable and writable from a shell script.
- [x] A peer entry carries enough to dial it: a URI, and a name that `P-6` can look up.
- [x] Nothing in this story consults the network. The registrar (`S-24`) and the local link
      (`T-24`) are separate stories, and this one must be useful with neither.
- [x] Failing-first test: `a_peer_written_to_the_book_is_listed_by_name`.

## Progress
- Done. `crates/sipx-cli/src/peers.rs` is the command; the format and location decision is
  recorded in [the design](../designs/discovery.md) under *Decided: the peer book*, and the
  command is documented in `website/docs/reference/cli.md`.
- **The book** is one peer per line, `name` whitespace `uri`, `#` for a comment — the only shape
  a shell already has verbs for (`echo … >> "$book"`, `while read -r name uri`). Nothing
  structured is appendable from a script without a parser or a `sed` that corrupts the file the
  second time it runs, and none of them is worth a dependency for two fields. No new dependency
  was taken.
- **It is looked for** in `--book`, then `$SIPX_PEERS`, then `$XDG_CONFIG_HOME/sipx/peers`, then
  `$HOME/.config/sipx/peers` — the flag/environment/default order `register` already uses for a
  password.
- **Every entry carries `status`, `name`, `uri`, `source`.** `source` is `book`; `S-24` and `T-24`
  add their own values to the same stream. `status` is what tells a peer line from the refusal
  line a registrar's "no" will need, so `select(.status == "peer")` survives both.
- **A book that cannot be read is a typed error**, never an empty list: missing, unreadable, or a
  line that is not a name and a URI all exit non-zero and name the file and the line. Only a book
  that exists, parses and holds no peers is an empty list with exit 0 — an empty list on a fresh
  machine reads as "there is nobody to call" when the truth is "you have not been told about
  anyone".
- **Strict about malformed lines** rather than skipping them: a skipped line prints a list that is
  short by one and says so nowhere, which is the partial-list-as-complete the design forbids. A
  third field is refused for the same reason — a SIP URI has no whitespace in it, so
  `alice sip:a@b # home` would otherwise become an undiallable URI that complains only at dial
  time.
- **Not a dial plan.** The command opens no socket and is not even async; it is consulted when a
  person names a peer, never while routing anything.
- Left for later, deliberately: no staleness/age field (a book entry has no age, and inventing one
  would report a number that means nothing — the sources with a real one add it), no name lookup
  or filtering (that is `P-6`), and duplicate names are listed as written rather than rejected
  (`P-6` owns what a name resolves to).

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
