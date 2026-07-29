---
id: X-26
title: Stop sipx-audio advertising a codec and a resampler it does not have
pillar: Build
status: ready
priority: 2
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
- [ ] `crates/sipx-audio/src/lib.rs:1` and `crates/sipx-audio/Cargo.toml:3` describe what the crate
      actually provides. These are the two strings a user meets first — the package description on
      the registry listing and the front page of the API reference.
- [ ] `docs/roadmap.md`'s `media` epic stops repeating the G.722 claim, or says plainly that it is
      planned rather than delivered.
- [ ] The decision is recorded: either the claims are removed because the codecs are not coming,
      or they are marked as planned with the story that will deliver them. `X-25` searched for why
      G.722 was dropped and found no record anywhere — a story, a spec, or a commit message — so
      whichever answer is right, this story is where it gets written down.
- [ ] Failing-first test: a check that the crate's own description names no codec it cannot
      encode or decode. `docs/compliance.md`, `X-22`'s gate drift check and `X-24`'s pool-key check
      are the house pattern for "a claim that cannot quietly lag its source", and a crate promising
      a codec is the same shape. If a check is not worth it here, say why and pin the claim with a
      doc test instead.

## Progress
- Not started.

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
