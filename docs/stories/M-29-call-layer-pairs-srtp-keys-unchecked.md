---
id: M-29
title: Make a live call run the SDES answer check it already owns
pillar: Media
status: in-progress
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
- [x] `srtp_keys` (`crates/sipx-call/src/call.rs:3002`) stops pairing `ours` with `theirs` and
      comparing nothing. Today it takes two `Option`s, unwraps both and builds `SrtpKeys` from
      whatever arrived — so an answer echoing a tag this side never offered is keyed on, which is
      the exact outcome `M-26` made impossible one layer down.
      → `srtp_keys` now calls `SrtpKeys::from_answer` and nothing else builds `SrtpKeys` from an
      answer; the pairing that remains is `srtp_keys_answering`, which is the *answering* side and
      has nothing to verify.
- [x] It takes the offered attributes as a slice, not a single `ours`. `verify_answer` returns
      *which* offer the answer accepted, and that is the half a caller must key with; picking the
      first offer works only while sipx offers exactly one.
      → `srtp_keys(offered: &[Crypto], answered: Option<&Crypto>)`; `settle_answer` and `establish`
      take the slice too, and the call sites pass `capabilities.crypto.as_slice()`.
- [x] The failure reaches the application as `Error::Sdp`, which already wraps `SdpError::Invalid`.
      The call fails saying which tag came back; it does not place an unencrypted call and it does
      not drop the stream silently.
      → asserted by the failing-first test, which requires the error to be `Error::Sdp`, to name
      the tag, and *not* to carry the key material.
- [~] `establish` and every other caller propagate the error rather than flattening it back to
      `Option`. A `Result` collapsed to `None` at the call site restores the defect with more code.
      → **True of `establish`, `settle_from` and `dial`**, which propagate with `?`; `dial` ACKs
      and BYEs before returning it, as it does for any post-2xx failure. **Not true of
      `Invitation::adopt_early_answer`**, which returns `()` and has no error channel to use: it
      logs the refusal and leaves the session `Offered`. Nothing is keyed on the refused answer
      there, and the 2xx re-settles through `settle_from` where the same refusal *does* end the
      call — so the defect is not restored, but a caller who never sends a 2xx would see the
      refusal only in a log line. Giving `observe` a fatal path is a change to the early-dialog
      loop `S-22` just landed, with CANCEL semantics attached; see `## Progress`.
- [x] Failing-first test: a call whose answer echoes a tag that was never offered completes today
      with keys neither end agreed on; name the test that makes it fail.
      → `an_answer_echoing_a_tag_that_was_never_offered_fails_the_call` in
      `crates/sipx-call/tests/secure_media.rs`.

## Progress
- **Done, with one bounded remainder** — the fourth Acceptance item's early-dialog half; see below.
- `srtp_keys` is now the offerer's side of RFC 4568 §5.1.3 and delegates to
  `SrtpKeys::from_answer`: `(&[Crypto], Option<&Crypto>) -> Result<Option<SrtpKeys>>`. `Ok(None)`
  survives for exactly one case — this side offered no key at all, which is a plain call. When we
  *did* offer, an answer carrying no usable `a=crypto` is refused rather than treated as a plain
  call, because that is the shape "a suite that was never offered" arrives in (`docs/specs/srtp.md`
  §5.4); this is a behaviour change and the reason the `offered.is_empty()` branch is explicit.
- The **answering** side is a separate function, `srtp_keys_answering`, and keeps the old pairing.
  §5.1.3 is the offerer's check; when this side answers it chose the attribute and echoed its own
  tag (`M-26`), so there is nothing to verify. Folding both into one function would have meant
  deciding at run time which side of the exchange it was on. Its three call sites are `Early::settle`,
  `Early::reanswer` and `answer_negotiated`.
- Failing-first evidence, at the merge base (`79d912d`) with only the test added:
  `cargo test --all-features -p sipx-call --test secure_media` →
  `an_answer_echoing_a_tag_that_was_never_offered_fails_the_call ... FAILED`,
  `panicked at crates/sipx-call/tests/secure_media.rs:188:9: a call keyed on an answer echoing a tag
  nobody offered was allowed to connect`. It is a whole call over WSS — the only transport that
  makes sipx offer a key at all (§7.1) — answered by a hand-built `200` rather than by `answer()`,
  because sipx's own answerer echoes the accepted tag correctly and cannot produce this response.
  The peer's `a=crypto` is well formed and carries a published key from §10.4; the only thing wrong
  with it is the tag, which is what a check on key material alone would miss.
- **`adopt_early_answer` still swallows.** It now logs at `debug` instead of discarding in silence,
  which is the smallest honest thing that fits: the enclosing `observe` returns `()` from inside the
  loop that drives an early dialog, so propagating means deciding to end an invitation from a
  *provisional*, and that carries CANCEL semantics rather than the ACK-then-BYE `dial` uses after a
  2xx. That is a story, not a hunk in this one.
- Registry and spec moved in the same commit: RFC 4568's "Still missing" sentence is gone, its
  evidence now cites `sipx-call`, and `status` stays `partial` — deliberately, and now for what the
  RFC defines *beyond* the offer/answer exchange (no MKI, no key lifetimes, no session parameters,
  no `RTP/SAVPF`) rather than for a MUST that no call ran. `docs/specs/srtp.md` §12.3 is closed and
  §5.4 records where the check runs.
- Gate: `./scripts/gate.py` → **18 steps, all green**.

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
