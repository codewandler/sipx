---
id: M-31
title: Make the answer and the negotiated codec agree, once
pillar: Media
status: in-progress
priority: 4
design: docs/designs/media.md
epic: media
areas: [sipx-sdp, sipx-call]
note: found by M-30's review — `sipx-sdp/src/answer.rs:423-427` compares an rtpmap clock rate as a string while `codec_named` parses it to `u32`, so an offer with `a=rtpmap:0 PCMU/08000` settles on PCMU while the answer names only `8`
---

# Make the answer and the negotiated codec agree, once

## Goal
Stop the answer sipx puts on the wire and the codec sipx configures its media session with from being
derived by two different rules, so they cannot name different formats for the same offer.

## Acceptance
- [ ] **An offer that settles on a codec is answered with that codec's format number.** Reproduced with
      `Codecs::G711` and an offer of `0 8` carrying `a=rtpmap:0 PCMU/08000` and `a=rtpmap:8 PCMA/8000`:
      negotiation settles on `Pcmu` with payload type `Some(0)` while the answer names `["8"]`. sipx
      would then send µ-law on a number the answer never offered, and decode the peer's PCMA(8) through
      a µ-law session. The leading zero is what splits them — `crates/sipx-sdp/src/answer.rs:423-427`
      compares the clock rate as a **string**, and `codec_of`/`codec_named` in `sipx-call` parses it to
      `u32`.
- [ ] **The rule is implemented once, not twice.** It is currently written in both crates, and
      `crates/sipx-call/src/call.rs:3945-3952` claims "the same rule the answer was built with … The two
      have to agree" while nothing enforces it. One of the two has to become the authority and the other
      has to call it; say which and why, given that `sipx-sdp` is the lower crate and must stay free of
      anything `sipx-call` owns.
- [ ] **The agreement is tested, not asserted in a comment.** A test that takes an offer, computes both
      the answer and the negotiated codec, and requires the negotiated payload type to appear in the
      answer's formats — driven over a table of offers including the leading-zero case, a codec sipx does
      not carry, and a dynamic number.
- [ ] **`m=` line format order is respected.** Whatever becomes authoritative must still honour the
      offerer's preference order, which is what the `find_map` in negotiation is for. `M-30` fixed a bug
      in that exact area — `carries` applied after the search rather than inside it — so the regression
      test `negotiation_does_not_settle_outside_the_selected_set` must stay green.
- [ ] Failing-first test: name the assertion that fails while the two rules disagree. The
      leading-zero offer above is a sufficient witness and needs no network.

## Notes
- **Found by the independent review of `M-30`**, not by the suite, and it is pre-existing rather than
  introduced there. `M-30` genuinely **narrowed** the class — an `8 iLBC/8000` offer now agrees where it
  did not before — which is why this is a separate story rather than a rework finding.
- **The class is wider than the leading zero.** A string comparison of a numeric field will disagree with
  a parsed one for whitespace, leading zeros, and any other spelling that is numerically equal and
  textually different. Fixing only `08000` would leave the shape in place, which is the
  "rule fitted to the data it was tested on" failure this repo keeps warning about.
- **Why it is a real defect and not cosmetic.** The two consequences are asymmetric: sending on a
  number the peer never offered may be discarded by a strict peer, but decoding the peer's PCMA through
  a µ-law session produces audible garbage rather than silence, and nothing in the stack would report an
  error.
- Reads with `M-30`, which made codec selection reachable and whose comment currently over-claims the
  agreement, and with `docs/specs/` for wherever the answer construction rule belongs normatively.
