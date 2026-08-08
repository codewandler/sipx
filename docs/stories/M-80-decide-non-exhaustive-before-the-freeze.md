---
id: M-80
title: Decide non-exhaustive for the two relayed packet structs
pillar: Media
status: ready
priority: 7
design:
epic: media
areas: [sipx-media, sipx-rtp]
predicate:
announcement:
note: two field additions in two stories, both breaking · the choice stops being reversible at 1.0
---

# Decide non-exhaustive for the two relayed packet structs

## Goal

Settle, while it is still reversible, whether `sipx_media::Encoded` and `sipx_rtp::Packet` carry
`#[non_exhaustive]` — and record the reason either way.

## Why

`M-75` added a field to `Packet`, `M-79` added one to `Encoded`, and each broke in-tree struct
literals. Both are public structs with public fields and a `new` constructor, so they already have
the shape `#[non_exhaustive]` would force, and `crates/sipx-media/src/lib.rs` currently reserves the
right to add fields in a minor release. That reservation and the missing attributes are two answers
to the same question, and `docs/roadmap.md`'s v1 predicate 4 is the point after which only one of
them can still be chosen.

The enum guard in `scripts/check-audio-claims.py` matches `pub enum` only, so nothing in the
repository has an opinion about these two. That is the gap.

## Acceptance

- [ ] The decision is made **for both types together** — marking one and not the other is arbitrary
      — and recorded where a reader meets the type, not only in a story.
- [ ] `crates/sipx-media/src/lib.rs`'s stability sentence and the attributes agree. Whichever way it
      goes, the other side moves.
- [ ] If `#[non_exhaustive]` is added, every in-tree construction site uses the constructor, the
      `CHANGELOG.md` entry says what a downstream literal should become, and a test proves the
      constructor reaches every field a literal could set.
- [ ] Whatever rule is chosen is **enforced**, so the next field addition cannot re-open this by
      accident: extend the audio-claims guard to public structs on the declared media surface, or
      state in the checker why structs are deliberately out of its scope.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from the independent review of `M-79`'s diff, which established the precedent
  chain and the deadline rather than merely raising the question.
