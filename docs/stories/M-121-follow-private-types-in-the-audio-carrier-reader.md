---
id: M-121
title: Follow private types in the raw-audio carrier reader
pillar: Media
status: backlog
priority: 42
design:
epic:
areas: [tooling, audio, security, media]
predicate:
announcement:
note: after M-68 · the checker reads one level deep, and M-68 found a carrier two levels down
---

# Follow private types in the raw-audio carrier reader

## Goal

Make `scripts/check-audio-claims.py`'s raw-audio rule catch a public `Debug` that reaches call audio
through a private field, which is the shape `M-68` found by hand after the checker passed.

## Acceptance

- [ ] The carrier reader resolves a public type's fields through private types in the same crate,
      to a stated depth, and reports a `Debug` that can reach a PCM buffer however many hops away.
- [ ] `M-68`'s own finding is a regression fixture: a public `Debug` over a private struct holding a
      `Vec<i16>` is reported, and the same type with a hand-written `Debug` is not.
- [ ] Whatever the widened reader newly reports across the workspace is either fixed or argued at
      its site; the check does not land reporting known-unfixed carriers.
- [ ] The docstring states the new depth, why that depth, and what a carrier past it still costs a
      reviewer — the check narrows deliberately or not at all.
- [ ] The full gate is green.

## Progress

- Backlog. Filed by `M-68`, which found `sipx_media::dsp::DspGraph`'s derived `Debug` rendering
  every sample of the frame in flight, the staging buffer and the scratch region — and up to eight
  more frames from a supervised stage's buffer pool. The checker passed the tree that contained it,
  because `DspGraph` holds no PCM field of its own: it holds a `SlotRef`, which holds an
  `Arc<Mutex<Slot>>`, which holds the buffers. `M-107`'s reader looks at a public type's own fields,
  which was the right first cut and is one hop short of this.
