---
id: S-31
title: Tolerate RFC 5118 §4.10's three-colon IPv6 reference
pillar: Signalling
status: in-progress
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
- [x] **`[2001:db8:::192.0.2.1]` parses.** RFC 5118 §4.10 shows the three-colon form beside the
      two-colon form and says an implementation "**must** tolerate both of the above constructs".
      sipx rejects it today with `StartLine(Uri(Host))`, which `X-16` recorded as the single entry in
      `crates/sipx-testkit/src/rfc5118.rs`'s `DEVIATIONS`.
- [x] **The tolerance is narrow, and the narrowness is the story.** `:::` must read as `::` **only
      immediately before an embedded IPv4 address** — the one position RFC 3261's ABNF can produce it,
      because that ABNF was inherited from the obsoleted RFC 2373 and corrected by RFC 4291. Accepting
      `:::` anywhere else would widen what sipx treats as a valid address on the strength of a
      typo. State the rule where the parser states its other rules, with both RFCs cited.
- [x] **The deviation record is deleted, not edited.** `rfc5118::DEVIATIONS` must become empty, and
      `recorded_deviations_still_hold` already prints delete-this-entry instructions for exactly this
      moment. The count guards in `no_valid_message_in_the_corpus_is_rejected` move with it —
      eleven valid, zero deviations, eleven covered.
- [x] **RFC 5118 moves off `partial`.** It is `partial` only because of this unmet MUST; with §4.10
      tolerated, the registry row and `docs/compliance.md` move in the same commit, and the note stops
      describing the gap.
- [x] **No panic, and no new tolerance elsewhere.** The corpus's no-panic test already covers all
      twelve messages in archive and wire form at every chunk boundary; it must stay green, and a
      malformed reference that is *not* the §4.10 shape must still be a typed error rather than an
      address parsed on a guess.
- [x] Failing-first test: the corpus already fails this case before the fix. Name the assertion that
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

## Progress

Done, pending review. `status: done` is the coordinator's flip.

**The fix** — `crates/sipx-sip/src/uri.rs`, new `parse_ipv6_reference`, called from the single
bracketed-reference arm of `parse_hostport`. RFC 4291 (`Ipv6Addr`'s own parser) is tried first and is
unchanged for every input it already accepted. Only on its failure is one `:::` rewritten to `::`
and **retried through that same parser**, and only when the text after the `:::` parses as an
`Ipv4Addr`. There is no second address grammar, so the accepted language is exactly RFC 4291 plus
RFC 3261's one derivation.

`split_once(":::")` takes the *first* occurrence, which is what makes the single IPv4 check
sufficient: a second `:::`, a fourth colon, or anything after the embedded address leaves a tail
that is not an `IPv4address`. `2001:db8::1:::192.0.2.1` rewrites to two `::` runs and the retry
rejects it.

**The rule** — `docs/specs/sip-parser.md` §4.8, new. States the `]`-keying decision (§4.3/§4.4),
the RFC 4291 address grammar, and the carve-out with the RFC 2373 ABNF derivation quoted, a table
of six accept/reject cases, and the "tolerated is not normalised" decision. RFC 4291 and RFC 5118
added to §1 normative references.

**Red → green** — `abnf_bug_reference_is_tolerated` in
`crates/sipx-sip/tests/rfc5118_corpus.rs`, the test name `X-16` had already promised in the §4.10
comment of `rfc5118.rs` but could not write. It failed
`RFC 5118 ipv6-bug-abnf-3-colons must parse: StartLine(Uri(Host))`. Three more went red with it
once `DEVIATIONS` was emptied — `every_rfc5118_message_is_classified_and_behaves_as_the_rfc_says`,
`no_valid_message_in_the_corpus_is_rejected`, `valid_messages_reserialize_byte_exactly`.

Note for a resuming agent: at the merge base all 14 corpus tests **pass**, because `X-16` recorded
the defect rather than leaving it unasserted. Emptying `DEVIATIONS` is what arms the red, and it is
Acceptance item 3 in any case — so the failing-first edit and the record deletion are one step.

**Narrowness** — `three_colons_anywhere_but_before_an_embedded_ipv4_address_stay_rejected` in
`uri.rs` holds thirteen near-misses to a typed error, including the unbracketed host path and the
short/long/out-of-range IPv4 tails.

**`docs/maturity.md` drifts** and is fenced: RFC 5118 moves from partial to implemented in its
coverage table. Left for the coordinator to regenerate with `./scripts/maturity.py`.

## Progress — rework round 1

Review found the code correct and the *artifact* wrong. Both findings were real; both fixed.

**B1 — §4.8's table named the wrong error for the unbracketed row.** It said `UriError::Host`;
the answer is `UriError::Port`. The cause is structural, not a typo, and worth the paragraph the
spec now gives it: an unbracketed `host` is split at its **first** `:` (`uri.rs:709-711`), so
`db8:::192.0.2.1` goes to `parse_port` and the text never reaches `parse_ipv6_reference` at all.

I did not just correct the variant. The row as written implied the carve-out was declining to
apply, which is the wrong lesson — so the table now carries the **valid** two-colon address
unbracketed as a second row, failing identically as `Port`. That pair says what the row is really
about: RFC 3261 §19.1.1's brackets are what make an IPv6 address reachable, and RFC 5118 §4.2 is
the message that omits them. A reader can no longer take the row as a statement about `:::`.

**B1's second half — the table was unchecked.** Nine checkable claims, and the suite asserted only
`is_err()`. Fixed at both levels:
- `three_colon_table_rows_parse_exactly_as_the_spec_says` (new file
  `crates/sipx-sip/tests/ipv6_reference_tolerance.rs`) transcribes every row and asserts the exact
  `Ok` address or exact `UriError` variant. Verified it catches the real defect: flipping that row
  back to `Host` fails with `left: Some(Port) right: Some(Host)`.
- `three_colons_anywhere_but_before_an_embedded_ipv4_address_stay_rejected` in `uri.rs` upgraded
  from `is_err()` to exact-variant assertions on all fifteen rows.

**B2 — two accepted shapes were in neither table nor test.** Both are RFC 2373's *other* `hexpart`
production, so they were inside the stated rule rather than holes, but an unenumerated table is the
same problem one step removed. `[:::192.0.2.1]` and `[1:2:3:4:5:::192.0.2.1]` are now table rows,
pinned in the table test, and exercised through `Uri::parse` in the `uri.rs` positive test. §4.8
now says explicitly that **both** `hexpart` productions are covered and nothing else is.

**The narrowness is now a permanent guard, not a review artefact.**
`the_tolerance_admits_nothing_but_the_rfc2373_derivation` enumerates 21 heads x 6 separators x 14
tails = 1764 combinations (1428 distinct references) and asserts the three-way property: accepted
by both parsers means identical addresses; accepted only by sipx means exactly one `:::`, no
`::::`, an `IPv4address` tail, a `::`-free `hexseq` head, and a valid two-colon rewrite; rejected
by sipx means rejected by RFC 4291 too. It also pins the **whole** beyond-RFC-4291 set as 24
enumerated strings, so a widening cannot land as a changed count — it shows up as a diff of
literal addresses.

Numbers are measured on **this** grid, not carried over from the review's: 86 plain accepts and 24
distinct carve-out accepts here, against the review's 115/27 on its own head and tail lists. I did
not copy those figures; my first attempt asserted them and failed, which is the test doing its job.

**Test suite, not `fuzz/`, and the file says why.** `fuzz/fuzz_targets/parse_uri.rs` already hunts
panics on arbitrary URI bytes — the right tool for "does anything crash". This is the other kind of
question, "is the accepted set exactly this set", which is decidable over a fixed grid, runs in
under a millisecond, and is only worth anything if it runs on every commit.

`docs/maturity.md` still drifts and is still fenced: wire layer implemented 2 to 3, partial 3 to 2.
