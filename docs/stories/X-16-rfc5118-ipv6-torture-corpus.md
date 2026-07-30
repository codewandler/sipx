---
id: X-16
title: Assert against the RFC 5118 IPv6 torture corpus
pillar: Build
status: done
priority: 12
design:
epic: conformance
areas: [sipx-testkit, sipx-sip, sipx-sdp]
note: M12 · RFC 5118 · the IPv6 twin of the corpus X-2 already imported
---

# Assert against the RFC 5118 IPv6 torture corpus

## Goal
Run the one published torture corpus sipx has never run, with the same discipline `X-2` applied to
RFC 4475: recovered bit-exactly from the RFC's own archive, and classified by which layer must
object to each message.

## Acceptance
- [x] The corpus is recovered from RFC 5118's **Appendix A** — "an encoded, gzip compressed TAR
      archive of files that represent each of the example messages discussed in Section 4" — not
      retyped from the body text. Retyped IPv6 literals are exactly the fixtures that quietly stop
      being the RFC's.
- [x] Every message in §4.1 through §4.10 is present and classified from what the RFC says about it,
      not from what sipx happens to do with it. §4.2 is titled "Invalid SIP Message with an IPv6
      Reference" and must be rejected; the rest are the RFC's demonstrations that a parser has to
      *accept* things it may not expect, and the classification records which layer, if any, objects.
- [x] The classification reuses `X-2`'s layer vocabulary rather than inventing a second one, so a
      reader comparing the two corpora is comparing like with like.
- [x] §4.3's ambiguity (`Port Ambiguous in a SIP URI`) gets an assertion about what sipx *decides*,
      with the decision recorded. A message that parses two ways is not a parser bug to fix but a
      choice to write down — and an unwritten choice is what makes two releases disagree.
- [x] §4.6 and §4.8 exercise SDP, so `sipx-sdp` is in the harness for this corpus and not only
      `sipx-sip`. IPv6 in `c=` and `o=` lines is a different code path from IPv6 in a `Via`.
- [x] The converse assertion `X-2` makes is made here too: no valid message in the corpus is rejected.
      A corpus that only proves rejections is half a test.
- [x] The RFC registry entry for RFC 5118 moves off "not started" in the same change, citing the
      harness.
- [x] Failing-first test: `every_rfc5118_message_is_classified_and_behaves_as_the_rfc_says`.

## Progress

Done, with one measured defect handed on rather than fixed here — per this story's own note that
"this story is the measurement".

**What was built**

- `scripts/import-rfc5118-corpus.sh` (+ `--check`), shaped after the RFC 4475 importer. Recovers
  all twelve Appendix A files into `crates/sipx-testkit/corpus/rfc5118/`, keeping the archive's own
  file names so each fixture still matches the "Message Details:" label in the RFC's prose.
- `crates/sipx-testkit/src/rfc5118.rs` — the case table and classification, importing `Expect` and
  `Fault` from `rfc4475` rather than defining a second vocabulary.
- `crates/sipx-sip/tests/rfc5118_corpus.rs` — 14 tests, SIP layer.
- `crates/sipx-sdp/tests/rfc5118_sdp.rs` — 4 tests, SDP layer (§4.6, §4.8, §4.9). Reads the corpus
  at run time: `sipx-sdp` is published, so a compile-time `include_bytes!` reaching outside the
  crate would not survive packaging.

**Ten sections, twelve files.** §4.5 and §4.10 each carry a contrast pair.

**Two things the corpus turned up that the RFC's body text does not tell you**

1. *The archive is not wire bytes.* RFC 5118's files are terminated with bare LF — not one CR octet
   in any of the twelve — and the §4.10 pair has no terminating blank line. RFC 4475's archive has
   real CRLFs, so this is specific to 5118. The corpus is stored bit-exactly regardless (that is
   what `--check` verifies) and `Case::wire()` performs the documented conversion.
2. *The declared `Content-Length` is wrong on all three SDP messages.* Not one of them matches
   either the LF-terminated body or a CRLF-terminated one:

   | case | declared | archive body (LF) | wire body (CRLF) |
   | --- | --- | --- | --- |
   | §4.6 `ipv6-in-sdp` | 268 | 242 | 251 |
   | §4.8 `mult-ip-in-sdp` | 181 | 180 | 189 |
   | §4.9 `ipv4-mapped-ipv6` | 236 | 236 | 245 |

   RFC 5118's only verified erratum (1311) is about §4.3's wording and does not mention this.
   `Case::wire()` corrects the value, because framing on the RFC's arithmetic would have measured
   that instead of sipx's IPv6 handling — and would have cut §4.8's and §4.9's SDP off mid-body
   before `sipx-sdp` ever saw the `c=` lines those sections exist to exercise. Framing itself is
   covered by RFC 4475, which has cases built for it and correct lengths elsewhere.

**§4.3's decision, recorded.** Everything inside `[` `]` is the address; a port is read only after
the `]`. So `sip:[2001:db8::10:5070]` is host `2001:db8::10:5070` with **no** port — not host
`2001:db8::10` on port 5070. Erratum 1311 confirms this reading: "the intended port number becomes
the last octet *pair* of the reference". `port_ambiguous_uri_takes_the_port_into_the_address`
asserts both the decision and its negation, so the opposite reading cannot pass.

**The one defect found — for a follow-up story.** §4.10's `[2001:db8:::192.0.2.1]` is rejected with
`ParseError::StartLine(UriError::Host)`. RFC 5118 is normative: "an implementation **must** tolerate
both of the above constructs." The extra colon is an artefact of RFC 3261's ABNF, inherited from the
obsoleted RFC 2373 and fixed by RFC 4291, and it can only arise immediately before an embedded IPv4
address — which is what makes a narrow fix possible (`:::` read as `::` in that one position).

That fix changes how a published crate parses hostile input, so it is not folded into the story that
measures the corpus. It is recorded as the single entry in `rfc5118::DEVIATIONS`, carrying what the
RFC requires, what sipx does, and why it stands. `recorded_deviations_still_hold` asserts the
deviation is still real and prints delete-this-entry instructions when it stops being — so the
record cannot rot into a lie, and the classification keeps saying what the *RFC* says rather than
drifting towards what sipx does.

Everything else passes: 11 of 12 messages accepted, §4.2 rejected with the right fault, byte-exact
re-serialization, stream framing at every chunk boundary, and no panic on any of the twelve in
either the archive or the wire form.

## Notes
- Cheapest story in M12 by a distance: the corpus is fixed, the archive is bit-exact, and `X-2` built
  the extraction and classification machinery already. Take it first for that reason.
- The expected outcome is that most of it passes on the first run. That is not an argument against
  doing it — the value is the ten or so cases where it does not, and none of them can be predicted
  from here.
- If a message *does* fail, the fix is a defect story of its own, the way `X-6` was. This story is the
  measurement.
