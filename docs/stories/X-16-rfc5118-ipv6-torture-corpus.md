---
id: X-16
title: Assert against the RFC 5118 IPv6 torture corpus
pillar: Build
status: ready
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
- [ ] The corpus is recovered from RFC 5118's **Appendix A** — "an encoded, gzip compressed TAR
      archive of files that represent each of the example messages discussed in Section 4" — not
      retyped from the body text. Retyped IPv6 literals are exactly the fixtures that quietly stop
      being the RFC's.
- [ ] Every message in §4.1 through §4.10 is present and classified from what the RFC says about it,
      not from what sipx happens to do with it. §4.2 is titled "Invalid SIP Message with an IPv6
      Reference" and must be rejected; the rest are the RFC's demonstrations that a parser has to
      *accept* things it may not expect, and the classification records which layer, if any, objects.
- [ ] The classification reuses `X-2`'s layer vocabulary rather than inventing a second one, so a
      reader comparing the two corpora is comparing like with like.
- [ ] §4.3's ambiguity (`Port Ambiguous in a SIP URI`) gets an assertion about what sipx *decides*,
      with the decision recorded. A message that parses two ways is not a parser bug to fix but a
      choice to write down — and an unwritten choice is what makes two releases disagree.
- [ ] §4.6 and §4.8 exercise SDP, so `sipx-sdp` is in the harness for this corpus and not only
      `sipx-sip`. IPv6 in `c=` and `o=` lines is a different code path from IPv6 in a `Via`.
- [ ] The converse assertion `X-2` makes is made here too: no valid message in the corpus is rejected.
      A corpus that only proves rejections is half a test.
- [ ] The RFC registry entry for RFC 5118 moves off "not started" in the same change, citing the
      harness.
- [ ] Failing-first test: `every_rfc5118_message_is_classified_and_behaves_as_the_rfc_says`.

## Progress
- Not started. `compliance.md`: "The IPv6 counterpart of 4475. sipx parses IPv6 hosts but has never
  been asserted against this corpus."

## Notes
- Cheapest story in M12 by a distance: the corpus is fixed, the archive is bit-exact, and `X-2` built
  the extraction and classification machinery already. Take it first for that reason.
- The expected outcome is that most of it passes on the first run. That is not an argument against
  doing it — the value is the ten or so cases where it does not, and none of them can be predicted
  from here.
- If a message *does* fail, the fix is a defect story of its own, the way `X-6` was. This story is the
  measurement.
