---
id: M-30
title: Let a call select Opus, or stop shipping a codec nothing can reach
pillar: Media
status: in-progress
priority: 4
design: docs/designs/media.md
epic: media
areas: [sipx-call, sipx-media, sipx-sdp]
note: M-13 built the codec, not the selection — sipx-call hardcodes G.711 at six sites and Codec::from_payload_type deliberately never returns Opus, so X-33 demoted RFC 6716 and 7587 to partial
---

# Let a call select Opus, or stop shipping a codec nothing can reach

## Goal
Make the Opus codec reachable from a call. `M-13` is `done` and built the encoder, the decoder and the
SDP half; nothing built the **selection**, so no call has ever carried an Opus packet.

## Acceptance
- [x] **A call can offer and answer Opus.** Four locks have to come off, and all four are verified:
      1. `sipx-call` hardcodes `Capabilities::g711` at `call.rs:606, 752, 955, 1728, 2860, 3161`, so
         payload type 111 is never offered.
      2. `Codec::from_payload_type` (`sipx-media/src/session.rs:115-124`) **deliberately** never
         returns Opus — there is a comment saying so — so even a hand-written peer offer cannot arrive
         at it. This is the interesting one: it is a closed door, not an unfinished wire.
      3. `Capabilities::with_opus` (`sipx-sdp/src/answer.rs:85`) has no caller outside `sipx-sdp`'s own
         tests.
      4. No `sipx-call` entry point accepts caller-supplied `Capabilities` — not `dial`, `dial_early`,
         `answer`, `answer_ringing`, `answer_early`, nor any of `DialOptions`' builders — and
         `crates/sipx-call/Cargo.toml` has no `[features]` block at all, so the `opus` feature cannot
         even be turned on through it.
- [x] **Which codec a call offers is the application's choice, with a stated default.** The default
      stays G.711: it is mandatory-to-implement and needs no C library. Opus links one and is off by
      default, so it cannot become the default by accident.
- [x] **RFC 6716 and 7587 go back to `implemented` in the same commit that makes them true**, and the
      published table's counts move with it. `X-33` demoted both to `partial` because nothing could
      reach them; the demotion is the honest state until this closes, and reversing it without the
      code would be the exact defect that check exists to catch.
- [x] **The public docs move in the same commit too.** `X-35` scoped every Opus mention to the crates
      — `README.md`, `website/docs/intro.md`, `does-this-fit.md`, `website/src/pages/index.js` — and
      `check-audio-claims.py` now holds all 44 front doors to agreement. When a call can select Opus
      those sentences become under-claims, and the guard will not catch an under-claim.
- [x] Failing-first test: a call placed with Opus selected offers payload type 111 and carries Opus
      packets. It cannot pass today because there is no selector. Name it.

## Progress
- Filed at `X-33`'s integration, from its explicit request: *"it wants a story, and so does wiring
  Opus to a call — `M-13` is `done` and built the codec, not the selection."*
- **Done, on `impl/M-30`.** A first implementor was killed mid-flight by an infrastructure error
  and left `f9f322e` — a `Codecs` enum, the `[features]` block, `tests/opus.rs`, and roughly half
  the call sites converted. That work was kept and finished rather than restarted; its design
  decisions were sound and are recorded below.
- **The selector is `Codecs`**, `G711` by default and `Opus` behind `sipx-call`'s new `opus`
  feature. Offering side: `DialOptions::with_codecs`, which reaches `dial`, `dial_once` and
  `dial_early`. Answering side: `answer_with`, `answer_ringing_with`, `answer_replacing_with`,
  `Invitation::answer_with` and `ring_early_with`. `Call` and `Early` carry the set, so a re-INVITE
  or a pre-200 UPDATE is answered from the set the call was placed with instead of narrowing to
  G.711 mid-call.
- **Lock 2 came off without changing the door, and that was the right call.** The story said to
  read `Codec::from_payload_type`'s refusal comment first because "the reason may still be good".
  It is good: RFC 7587 §7 assigns Opus no static type, so 111 alone means nothing and returning
  Opus for it would decode somebody else's G.729 as Opus. The lock is therefore opened one level
  up — `negotiated` matches a format by its `a=rtpmap` (RFC 8866 §6.6 makes the map authoritative
  even over a static number), and the number the far end assigned travels with the codec on
  `Config::payload_type` rather than being reassumed. `from_payload_type` still refuses Opus and
  should. A side effect worth having: an offer of `8` remapped to iLBC is no longer read as PCMA.
- **`negotiated` will not settle outside the selected set.** An Opus offer reaching a G.711 call is
  answered G.711, because the answer this side builds never named Opus and a session started on a
  codec no answer named sends packets the far end cannot place.
- **The `carries` check had to be inside the search, not applied to its result.** The interrupted
  WIP filtered after `find_map`, which stopped at the offerer's first choice and refused the whole
  description when that one format was outside our set: an Opus-first offer to a default G.711 call
  came back `NoCommonCodec` while the answer on the wire named the PCMU further down the same list.
  Invisible in the default build — where no rtpmap can name Opus at all — and live under
  `--all-features`, which is how the gate builds. Caught by
  `negotiation_does_not_settle_outside_the_selected_set`, added for exactly this reason.
- **Both feature configurations are built and tested**, not just `--all-features`. `call.rs`'s test
  module runs in both and asserts the default build's promise; `tests/opus.rs` is gated on the
  feature.
- **Registry**: 6716 and 7587 restored to `implemented`, citing `sipx-call/src/call.rs` and
  `sipx-call/tests/opus.rs`, and `docs/compliance.md` regenerated — implemented 29 → 31, partial
  24 → 22. Neither row claims `roles`, deliberately: they never did, and adding a role claim is a
  new assertion this story did not ask for. Worth a look if the coordinator disagrees — both roles
  are in fact exercised, in the same one-direction shape RFC 3711's note already describes.
- **What is still not true, and is now written into the rows rather than left implied**: there is
  no Opus `a=fmtp` in either direction, so §7.1's optional parameters — `maxaveragebitrate`,
  `useinbandfec`, `usedtx`, `maxplaybackrate`, `cbr`, `stereo`/`sprop-stereo` — are neither offered
  nor read; and `sipx-cli` still takes no flag for Opus, so the codec is reachable from the library
  and not from the binary.
- **Gate: 20 steps, all green**, from a real run on this branch after the final commit. The first
  run failed 5 steps and every one was mine, not the merge base: `fmt`, `clippy`
  (`too_many_arguments` on the eighth parameter, `too_many_lines` on `dial_with`, and
  `push_str(&format!(..))` in the new test module), `maturity`/`maturity tests` (`docs/maturity.md`
  needed regenerating for the registry move), and `rfc report tests` — see below.
- **`X-33` left a guard that this story had to invert, and that is the intended interaction.**
  `scripts/test-rfc-report.py` asserted RFC 6716 and 7587 were `partial` *by number*, so promoting
  them failed the suite. It was rewritten to hold the rule instead of the verdict: the rows may say
  `implemented` only while they cite the call layer, `sipx-call` actually calls
  `Capabilities::with_opus`, and 7587's note still states the `a=fmtp` boundary. The guard keeps its
  teeth — it now fails a reversal *and* a hollow promotion.
- **`main` moved while this was parked, and merging needs one hand resolution.** `S-25` landed on
  `main` and rewrote `Dialing::adopt_early_answer` to return `Result<()>`, propagating through `?`
  rather than logging and returning; this story added a third argument to `settle_answer`. Both
  intents compose, but they touch the same lines, so `git merge` reports one conflict in
  `crates/sipx-call/src/call.rs`. **Resolution: take `main`'s body and add the argument** —
  `settle_answer(self.capabilities.crypto.as_slice(), &answer, self.options.codecs)?`. Nothing else
  in the file conflicts. `docs/maturity.md` also conflicts, in its generated burn-down row only;
  run `./scripts/maturity.py` after the merge and it settles.

## Notes
- **This is the sixth instance of the project's recurring defect**, after ICE (`M-27`), UPDATE
  (`S-22`), DTLS-SRTP (`M-28`), the SDES answer check (`M-29`) and RFC 8122 — a capability
  implemented and tested inside one crate that nothing above it can select, reading as shipped.
- **It is the first one found in the public docs rather than in the registry.** Two independent
  read-only sweeps found Opus advertised on the README, in `intro.md`, in `does-this-fit.md`'s *"It
  fits if you want to"* list and on the landing page. The registry rows said `implemented` and escaped
  `X-30`'s check because they carry no `roles` field at all — which is the hole `X-33` then closed.
- **What makes this case sharper than the others**: `Codec::from_payload_type` refuses Opus with a
  comment explaining the refusal. The other five were unfinished wiring; this was a door someone shut
  on purpose while the front page advertised it as open. Read that comment before changing it — the
  reason may still be good, in which case the honest answer is to remove the codec's claims rather
  than the door.
- Compare `M-28`: the same "reachable from no call" shape, but that one is blocked on a genuine
  ordering problem (`establish` runs before the ACK). This one has no such obstacle recorded — which
  means either it is simpler than it looks, or the obstacle has not been found yet.
- Priority 4, above `M-28`'s 5: G.711-only is a real limitation for anyone on a lossy network, the
  codec is already written and tested, and the missing piece is a selector rather than a protocol
  exchange.
