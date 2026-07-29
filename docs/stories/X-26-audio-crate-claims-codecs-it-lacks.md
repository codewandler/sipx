---
id: X-26
title: Stop sipx-audio advertising a codec and a resampler it does not have
pillar: Build
status: done
priority:
design: docs/designs/media.md
epic: media
areas: [sipx-audio, docs]
note: found by X-25 — the published crate description promises G.722 and resampling; neither exists
---

# Stop sipx-audio advertising a codec and a resampler it does not have

## Goal
Make `sipx-audio`'s published description true. It claims G.722 and resampling; the crate
implements neither, and the CLI tells the user to resample before dialling.

## Acceptance
- [x] `crates/sipx-audio/src/lib.rs:1` and `crates/sipx-audio/Cargo.toml:3` describe what the crate
      actually provides. These are the two strings a user meets first — the package description on
      the registry listing and the front page of the API reference.
- [x] `docs/roadmap.md`'s `media` epic stops repeating the G.722 claim, or says plainly that it is
      planned rather than delivered.
- [x] The decision is recorded: either the claims are removed because the codecs are not coming,
      or they are marked as planned with the story that will deliver them. `X-25` searched for why
      G.722 was dropped and found no record anywhere — a story, a spec, or a commit message — so
      whichever answer is right, this story is where it gets written down.
- [x] Failing-first test: a check that the crate's own description names no codec it cannot
      encode or decode. `docs/compliance.md`, `X-22`'s gate drift check and `X-24`'s pool-key check
      are the house pattern for "a claim that cannot quietly lag its source", and a crate promising
      a codec is the same shape. If a check is not worth it here, say why and pin the claim with a
      doc test instead.

## Progress
- **The decision: the claims go. G.722 is not coming, and neither is resampling.** The evidence,
  gathered before deciding, is that nothing anywhere ever expected either — `git log -S 'G.722'`
  turns up the scaffolding commit that wrote the blurb and no commit that implemented or cut it,
  there is no `M-*` story for it among twenty-five, and the stack's behaviour is specified in the
  opposite direction: `Codec::from_payload_type(9)` returns `None`, `sipx-sdp` answers an offer of
  G.722 with port 0, and `sipx-call` refuses a call offering nothing else — three tests assert it.
  The wideband slot G.722 would have filled is Opus's (`M-13`). Marking it "planned" would have
  meant inventing a story to justify a blurb, which is the wrong way round. Recorded in
  [`docs/designs/media.md`](../designs/media.md), where `X-25` had filed it as gap 3 of the
  decisions it could not find; that gap is now closed rather than restated.
- A third false claim turned up while checking: the description also promised **RFC 4733 DTMF**,
  which lives in `sipx-rtp` (`crates/sipx-rtp/src/dtmf.rs`) and never was in this crate. Removed
  with the others. The description also *omitted* Opus, which the crate does have — now named,
  with its feature, since a codec that is off by default is not one a reader should assume.
- A fourth copy of the claim was in `website/docs/guides/as-a-library.md`'s "Which crate" table,
  which is the version a user actually reads. Fixed, and it is one of the three strings the new
  check reads.
- `scripts/check-audio-claims.py` is the check, wired into the gate (`docs`) with its suite
  (`gate`); the gate is 17 steps and `--check` reports no drift. It failed first on all three
  front doors for G.722, resampling and DTMF — see the story's commit for the recorded output.
- Not done here: resampling has no story of its own. If it is ever wanted, the CLI's refusal at
  `crates/sipx-cli/src/dial.rs:215` is the place it would have to change, and that wants a story
  rather than a line in a blurb.

## Notes
- Found by `X-25` while writing the media design record: it went looking for the argument behind
  dropping G.722 so it could record it, and found the claim still being made in three places
  instead.
- **This is the most user-visible untruth the docs currently carry.** The compliance table is
  careful, the specs are careful, and the gate now checks the gate — while the package blurb on the
  crate a user installs first names a codec that is not there. `sipx-cli/src/dial.rs:215` tells the
  user to resample the file themselves, which is the crate's own admission.
- Cheap. It is priority 2 because it ships with every publish and costs a user their first
  impression, not because it is hard.
