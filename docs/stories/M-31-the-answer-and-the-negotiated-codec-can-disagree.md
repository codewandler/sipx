---
id: M-31
title: Make the answer and the negotiated codec agree, once
pillar: Media
status: done
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
- [x] **An offer that settles on a codec is answered with that codec's format number.** Reproduced with
      `Codecs::G711` and an offer of `0 8` carrying `a=rtpmap:0 PCMU/08000` and `a=rtpmap:8 PCMA/8000`:
      negotiation settles on `Pcmu` with payload type `Some(0)` while the answer names `["8"]`. sipx
      would then send µ-law on a number the answer never offered, and decode the peer's PCMA(8) through
      a µ-law session. The leading zero is what splits them — `crates/sipx-sdp/src/answer.rs:423-427`
      compares the clock rate as a **string**, and `codec_of`/`codec_named` in `sipx-call` parses it to
      `u32`.
- [x] **The rule is implemented once, not twice.** It is currently written in both crates, and
      `crates/sipx-call/src/call.rs:3945-3952` claims "the same rule the answer was built with … The two
      have to agree" while nothing enforces it. One of the two has to become the authority and the other
      has to call it; say which and why, given that `sipx-sdp` is the lower crate and must stay free of
      anything `sipx-call` owns.
- [x] **The agreement is tested, not asserted in a comment.** A test that takes an offer, computes both
      the answer and the negotiated codec, and requires the negotiated payload type to appear in the
      answer's formats — driven over a table of offers including the leading-zero case, a codec sipx does
      not carry, and a dynamic number.
- [x] **`m=` line format order is respected.** Whatever becomes authoritative must still honour the
      offerer's preference order, which is what the `find_map` in negotiation is for. `M-30` fixed a bug
      in that exact area — `carries` applied after the search rather than inside it — so the regression
      test `negotiation_does_not_settle_outside_the_selected_set` must stay green.
- [x] Failing-first test: name the assertion that fails while the two rules disagree. The
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

## Progress
**Complete on `impl/M-31`, gate green at 22 steps.** Acceptance is fully ticked; the `status: done`
flip and the CHANGELOG entry are the coordinator's, not this branch's.

**The table found more than the story reported: three live disagreements in the default build, four
with `opus`** — not one. The leading-zero clock rate (the reported witness), a leading zero in the
*channel* count, and a **signed** clock rate; plus a leading zero on Opus's own rate under the
feature. The signed one was not predicted by anybody, including this implementor's own commit
message on `1cc0dac`, which lists it among the rows that already agreed: `u32::from_str` accepts a
leading `+`, so the parsing rule read `+8000` as eight thousand while the textual rule did not —
the same split as a leading zero, reached from the other side. Enumerated by instrumenting the
table at `1cc0dac` in a throwaway worktree, so this is measured rather than reasoned.

That also demonstrates the fix is one *rule* and not a normalisation pass: a leading zero is
resolved by accepting it and a sign by refusing it — opposite verdicts, and both callers follow
each. Agreement comes from there being one reader, not from any particular verdict.

**The authority is `sipx-sdp`.** `crates/sipx-sdp/src/rtpmap.rs` is new and is the only place
RFC 8866 §6.6 format identity is decided: `Rtpmap::parse` reads `<name>/<rate>[/<params>]` into a
typed value, and `same_format` compares the name case-insensitively and the rate and channel count
**by value**. `answer.rs`'s `rtpmap_matches` is *deleted* rather than reduced to a wrapper — a
one-line pass-through is the thing that later grows a second opinion — and `supports` calls
`crate::rtpmap::same_format` directly (`answer.rs:403`). `codec_named` in `sipx-call` no longer
parses anything: it asks the same predicate once per codec it can run, against that codec's offered
rtpmap (`call.rs:4103`, with `offered_rtpmap` at `call.rs:4134`). `opus_named` and its
`cfg(not(opus))` stub are gone with it.

Why that direction and not the other: `sipx-call` depends on `sipx-sdp`, so `answer` — which builds
the answer that goes on the wire — cannot call up. The only arrangement where one implementation
serves both is the lower crate holding it. Format identity is the largest piece of the question that
carries nothing `sipx-call` owns: no codec set, no selection policy, no preference order. Those stay
above, in `offered_rtpmap` (which rtpmaps map to a codec sipx can run) and `Codecs::carries` (which
the application selected).

**Spec:** `docs/specs/sdp-format-identity.md` is new and normative. §2.3 records the direction and
the argument, §3 the grammar and everything refused, §4 the vectors both test modules are derived
from, §5 what is deliberately *not* unified.

**One judgement call worth re-opening if you disagree** (§3.2): leading zeros are *tolerated*
(`08000` reads as 8000) although RFC 8866 §9's `integer` starts at a non-zero digit. A sign,
whitespace, a digit separator, an empty field, a `u32` overflow and a fourth field are all refused as
typed `RtpmapError`s, and therefore as non-matches at both callers. Refusing `08000` instead would
also have satisfied the Acceptance — the two rules would agree by both declining format 0 — but it
declines a format the peer plainly named, so the tolerance fails in the interoperating direction.
It is safe to tolerate *only because* there is now one reader.

**Not changed, on purpose** (spec §5.2): `answer::encoding_of` and
`call::telephone_event_payload_type` each take the text before the first `/` to spot
`telephone-event`. They compare no numeric field and they agree with each other, so they are not
instances of this story's split — and routing them through §3's grammar would change behaviour, by
declining a rate-less `a=rtpmap:101 telephone-event` that yields working DTMF today. That needs a
story that argues for it.

**Registry:** RFC 8866 gains `rtpmap.rs` as evidence and both 8866 and 3264 have their notes
extended; no status or role changed. This justifies claims that already existed rather than earning
new ones. `docs/compliance.md` regenerated.
