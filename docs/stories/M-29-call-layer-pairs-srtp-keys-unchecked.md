---
id: M-29
title: Make a live call run the SDES answer check it already owns
pillar: Media
status: ready
priority: 2
design: docs/specs/srtp.md
epic: media
areas: [sipx-call]
note: found by M-26 — verify_answer and SrtpKeys::from_answer exist and sipx-call calls neither, so a live call still keys on an answer nobody checked
---

# Make a live call run the SDES answer check it already owns

## Goal
Move `sipx-call` onto `SrtpKeys::from_answer`, so RFC 4568 §5.1.3's check runs on a real call and
not only in `sipx-sdp`'s and `sipx-media`'s tests.

## Acceptance
- [ ] `srtp_keys` (`crates/sipx-call/src/call.rs:3002`) stops pairing `ours` with `theirs` and
      comparing nothing. Today it takes two `Option`s, unwraps both and builds `SrtpKeys` from
      whatever arrived — so an answer echoing a tag this side never offered is keyed on, which is
      the exact outcome `M-26` made impossible one layer down.
- [ ] It takes the offered attributes as a slice, not a single `ours`. `verify_answer` returns
      *which* offer the answer accepted, and that is the half a caller must key with; picking the
      first offer works only while sipx offers exactly one.
- [ ] The failure reaches the application as `Error::Sdp`, which already wraps `SdpError::Invalid`.
      The call fails saying which tag came back; it does not place an unencrypted call and it does
      not drop the stream silently.
- [ ] `establish` and every other caller propagate the error rather than flattening it back to
      `Option`. A `Result` collapsed to `None` at the call site restores the defect with more code.
- [ ] Failing-first test: a call whose answer echoes a tag that was never offered completes today
      with keys neither end agreed on; name the test that makes it fail.

## Progress
- Not started.

## Notes
- **Filed by `M-26` at integration, not by its implementor's choice.** `sipx-call` was outside that
  story's write set and held by a concurrent story (`S-22`), so stopping at the crate boundary was
  correct. The finding is that `M-26`'s Acceptance items 2 and 3 — "the call fails in the direction
  that says why" and "reported to the application through the existing error vocabulary" — were
  ticked `[x]` when they are true only below `sipx-call`. They now read `[~]`, and this story is
  the remainder.
- **Fourth instance of the pattern `M-28` named**, and the first found in a story that had just
  finished. `M-22` built ICE no call can offer (`M-27`), RFC 3311 claimed a role the caller could
  not reach (`S-22`), `M-15` built DTLS-SRTP no call can select (`M-28`), and here a check that
  exists, is tested, and no call runs. A capability whose only caller is its own test suite is
  indistinguishable from a shipped one in `docs/compliance.md`.
- The RFC 4568 registry note already says this out loud — it names the missing wiring as
  **"Still missing"** rather than claiming the MUSTs are honoured end to end. When this lands, that
  sentence comes out and the row is re-examined against `status = "partial"`.
- Reads with `M-28`: both change how `sipx-call` turns a negotiated description into keys, and
  `M-28` adds the keying *selector* this story's check has to run underneath. If they run near each
  other, one is rebasing — and `M-28` is the one that should go second, since it is easier to add a
  keying choice above a checked path than to retrofit the check under a new one.
