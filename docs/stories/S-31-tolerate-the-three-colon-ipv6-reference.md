---
id: S-31
title: Tolerate RFC 5118 §4.10's three-colon IPv6 reference
pillar: Signalling
status: ready
priority: 4
design: docs/specs/sip-parser.md
epic: sip-core
areas: [sipx-sip]
note: found by X-16's corpus — `[2001:db8:::192.0.2.1]` is rejected with `StartLine(Uri(Host))`, and RFC 5118 §4.10 is normative that an implementation MUST tolerate it; the one recorded entry in `rfc5118::DEVIATIONS`
---

# Tolerate RFC 5118 §4.10's three-colon IPv6 reference

## Goal
Accept the malformed-looking IPv6 reference that RFC 3261's own ABNF can produce, so that sipx stops
rejecting a message the RFC requires it to parse.

## Acceptance
- [ ] **`[2001:db8:::192.0.2.1]` parses.** RFC 5118 §4.10 shows the three-colon form beside the
      two-colon form and says an implementation "**must** tolerate both of the above constructs".
      sipx rejects it today with `StartLine(Uri(Host))`, which `X-16` recorded as the single entry in
      `crates/sipx-testkit/src/rfc5118.rs`'s `DEVIATIONS`.
- [ ] **The tolerance is narrow, and the narrowness is the story.** `:::` must read as `::` **only
      immediately before an embedded IPv4 address** — the one position RFC 3261's ABNF can produce it,
      because that ABNF was inherited from the obsoleted RFC 2373 and corrected by RFC 4291. Accepting
      `:::` anywhere else would widen what sipx treats as a valid address on the strength of a
      typo. State the rule where the parser states its other rules, with both RFCs cited.
- [ ] **The deviation record is deleted, not edited.** `rfc5118::DEVIATIONS` must become empty, and
      `recorded_deviations_still_hold` already prints delete-this-entry instructions for exactly this
      moment. The count guards in `no_valid_message_in_the_corpus_is_rejected` move with it —
      eleven valid, zero deviations, eleven covered.
- [ ] **RFC 5118 moves off `partial`.** It is `partial` only because of this unmet MUST; with §4.10
      tolerated, the registry row and `docs/compliance.md` move in the same commit, and the note stops
      describing the gap.
- [ ] **No panic, and no new tolerance elsewhere.** The corpus's no-panic test already covers all
      twelve messages in archive and wire form at every chunk boundary; it must stay green, and a
      malformed reference that is *not* the §4.10 shape must still be a typed error rather than an
      address parsed on a guess.
- [ ] Failing-first test: the corpus already fails this case before the fix. Name the assertion that
      goes from red to green, and confirm it is red at the merge base.

## Notes
- **Found by measurement, exactly as intended.** `X-16` imported the corpus and deliberately did not
  fix what it found, because the fix changes how a published crate parses hostile input and belongs in
  a story that can be reviewed as such. The measurement is already in the tree, so this story starts
  with its failing test written.
- **This is a tolerance, not a correctness fix, and the distinction matters for review.** The
  three-colon form is not valid IPv6 under RFC 4291; it is valid under the ABNF RFC 3261 shipped, so
  real implementations emit it. Being liberal here is required by 5118 and is *not* licence to be
  liberal about addresses generally — sipx's posture is typed errors on network input, and this is a
  single documented carve-out.
- **Do not reach for a wider address parser.** The temptation is to relax the host rule and let a
  general-purpose parser sort it out. That trades one unmet MUST for an unknown surface on
  unauthenticated input.
