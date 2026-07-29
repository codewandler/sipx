---
id: M-26
title: Echo and verify the SDES tag, which RFC 4568 requires twice
pillar: Media
status: ready
priority: 2
design: docs/specs/srtp.md
epic: media
areas: [sipx-sdp, sipx-media]
note: found by M-25 — RFC 4568 §5.1.2 and §5.1.3 are both MUSTs and sipx honours neither
---

# Echo and verify the SDES tag, which RFC 4568 requires twice

## Goal
Answer an `a=crypto` offer the way RFC 4568 requires: echo the tag of the crypto suite actually
chosen, and verify the tag that comes back rather than assuming the peer chose what was offered.

## Acceptance
- [ ] The answer echoes the `tag` of the accepted `a=crypto` line (RFC 4568 §5.1.2). A **MUST**
      that sipx does not honour today.
- [ ] An answer whose tag names a suite that was never offered is refused rather than used
      (§5.1.3), and the call fails in the direction that says why instead of negotiating keys
      nobody agreed on.
- [ ] The failure is reported to the application through the existing error vocabulary, not by a
      silently unencrypted or silently dropped stream.
- [ ] Byte-level: `docs/specs/srtp.md` §10.4 already restates a published `a=crypto` line ready to
      be asserted against `Crypto::parse` — assert it here, since that parser is currently tested
      only against its own output.
- [ ] Failing-first test: an answer echoing a tag that was not offered is accepted today; name the
      test that proves it stops being.

## Progress
- Not started.

## Notes
- Filed by `M-25` as `docs/specs/srtp.md` §12.3, which is where the normative statement now lives.
- **Two MUSTs, one story**, because the echo and the verification are the two halves of the same
  handshake and fixing either alone leaves the other end of it unchecked.
- The write set is `sipx-sdp` and `sipx-media`, which is why `M-25` could not do it: that story's
  fence was `docs/specs/` plus `sipx-rtp`.
- **The same blind spot that hid `M-25`'s `n_a` defect applies here.** `sipx-sdp`'s `Crypto::parse`
  and `Fingerprint::parse` are tested only against their own output, so a parser that is
  self-consistently wrong reads as correct. `docs/specs/srtp.md` §10.4 and §10.6 have the vectors
  written and the tests unwritten; see also `X-27`.
