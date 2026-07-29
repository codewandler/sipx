---
id: P-6
title: Dial a peer by name
pillar: Phone
status: backlog
priority:
design: docs/designs/discovery.md
epic: discovery
areas: [sipx-cli]
note: needs P-5 — `sipx dial alice` where alice came out of `sipx peers`
---

# Dial a peer by name

## Goal
Let `sipx dial alice` mean what a person expects, where `alice` is a name `sipx peers` printed —
so a script can place a call without a URI written anywhere in it.

## Acceptance
- [ ] `sipx dial <name>` resolves the name through the peer book and places the call. A name that
      resolves to nothing fails with a typed error naming the name, not a URI parse failure.
- [ ] **A URI still dials as a URI.** `P-3`'s behaviour is unchanged for every input that is
      already a URI, and the story says how the two are told apart — a bare word and a `sip:` URI
      are not ambiguous, but `alice@example.com` is, and that case needs a stated rule.
- [ ] Resolution is a lookup *followed by* a normal dial, not a second dial path. Everything `P-3`
      proved about placing a call keeps holding because it is the same code placing it.
- [ ] An ambiguous name — two sources offering the same name — is reported rather than silently
      resolved to whichever was found first, and the report says which sources disagreed.
- [ ] Failing-first test: `a_name_from_the_peer_book_places_a_call_without_a_uri`.

## Progress
- Not started. Blocked on `P-5` for the book and the name.

## Notes
- Second story of the `discovery` epic; see [the design](../designs/discovery.md).
- **The epic's end-to-end proof lands here**: a shell script that runs `sipx peers`, takes a name
  out of the machine-readable output, passes it to `sipx dial`, and places a call — with no URI in
  the script. That is the same shape of proof `P-1` set for the CLI as a whole, and vision
  principle 6 is why it is the measure.
- The ambiguity rule matters more once `S-24` and `T-24` land, since a registrar and the local link
  can easily both know an `alice`. Getting the reporting right with one source is cheaper than
  retrofitting it with three.
