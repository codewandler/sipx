---
id: M-26
title: Echo and verify the SDES tag, which RFC 4568 requires twice
pillar: Media
status: in-progress
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
- [x] The answer echoes the `tag` of the accepted `a=crypto` line (RFC 4568 §5.1.2). A **MUST**
      that sipx does not honour today.
- [x] An answer whose tag names a suite that was never offered is refused rather than used
      (§5.1.3), and the call fails in the direction that says why instead of negotiating keys
      nobody agreed on.
- [x] The failure is reported to the application through the existing error vocabulary, not by a
      silently unencrypted or silently dropped stream.
- [x] Byte-level: `docs/specs/srtp.md` §10.4 already restates a published `a=crypto` line ready to
      be asserted against `Crypto::parse` — assert it here, since that parser is currently tested
      only against its own output.
- [x] Failing-first test: an answer echoing a tag that was not offered is accepted today; name the
      test that proves it stops being.

## Progress
- **Done in `sipx-sdp` and `sipx-media`. One wiring change is left over and has an owner** — see
  the last bullet, and `docs/specs/srtp.md` §12.3.
- §5.1.2: `Crypto::accepting` builds the answer's attribute from the *accepted* offer's tag and
  suite with this side's own key; `answer_stream` uses it. Failing-first evidence: with only
  `answer.rs` reverted to the merge base, `the_answer_echoes_the_tag_of_the_accepted_offer` fails
  `left: 1, right: 9` — sipx answered tag 1 to every offer, because `Capabilities::with_srtp` fixes
  its own tag at 1.
- §5.1.3: `Crypto::verify_answer` is the three-part check (offered suite, accompanying tag, a key)
  and returns the *offered* attribute the answer accepted, so the caller keys with the half it
  sent. `SrtpKeys::from_answer` is the only route from an answer to keys, and it returns `Result`,
  not `Option` — a mismatch is `SdpError::Invalid`, which `sipx-call` already maps to `Error::Sdp`,
  rather than a stream that drops to unencrypted with nobody told.
- An answer naming a suite that was never offered arrives as *no usable attribute at all*, because
  `Crypto::parse` refuses a suite sipx cannot key. `verify_answer` therefore takes an `Option` and
  refuses `None`; `an_answer_naming_a_suite_that_was_never_offered_is_refused` pins it.
- §10.4's published `a=crypto` line is now asserted against `Crypto::parse` (vector 11 in the spec's
  table). **It passed on the first run** — the parser was already right. That is the usual outcome
  of a published-vector test and not a reason to have skipped it: nothing distinguished this case
  from `sipx-rtp`'s `n_a` defect (§12.1) beforehand.
- **Left for a new story, in `sipx-call`:** `srtp_keys` in `crates/sipx-call/src/call.rs` still
  pairs `capabilities.crypto` with whatever `a=crypto` the answer carried, comparing nothing, and
  returns `Option`. Until it is moved onto `SrtpKeys::from_answer` and `establish` propagates the
  error, a *live* call still accepts an answer that echoed a tag nobody offered. `sipx-call` was
  outside this story's write set and held by a concurrent story.

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
