---
id: M-96
title: Fix the preamble reader's off-by-one
pillar: Media
status: ready
priority: 25
design:
epic: media
areas: [scripts]
predicate:
announcement:
note: an item at byte 0 with no blank line above it loses the first character of its preamble · cannot fire on this tree, which is why it needs a test rather than a reader
---

# Fix the preamble reader's off-by-one

## Goal

Make `preamble()` in `scripts/check-audio-claims.py` read the whole preamble of an item that starts
at byte 0, so the rationale guard cannot silently miss a `/// Exhaustive by design:` or
`/// Complete by design:` line.

## Why

Found by `M-92`'s implementor while extending the struct rule, and reported rather than quietly
patched: `preamble()` locates the blank line above an item with `rfind`, which returns `-1` when
there is none, and then adds 2 — so for an item at byte 0 the slice starts at index 1 and **the
first character of the preamble is dropped**.

It cannot fire on this tree, because every file in the workspace opens with a `//!` module comment
and no guarded item is the first byte of its file. That is exactly why it is worth a story: the
guard is one file-layout change away from reading a rationale it cannot see, and a rationale it
cannot see is a finding it reports against code that is already correct — or, in the mirror case, a
guarded item it lets through. It surfaced only in a synthetic test.

## Acceptance

- [ ] A failing-first test builds the shape that triggers it — a guarded item at byte 0 with no
      blank line above it — and shows the rationale being missed.
- [ ] The fix is the arithmetic, not a special case for byte 0: `rfind` returning `-1` and a real
      match at index 0 are different facts and the code should tell them apart.
- [ ] The guard's own blindness assertions cover the shape, so it stays covered when the reader is
      next touched.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-09: filed at integration. `M-92`'s implementor deliberately did not file it mid-wave,
  reasoning that a new story file would leave the fenced board stale and redden the wave gate. That
  is the right call for an implementor and the wrong outcome for the finding, so the coordinator
  files it — which is what the board being fenced is *for*.
