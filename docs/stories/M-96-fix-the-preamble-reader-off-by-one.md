---
id: M-96
title: Fix the preamble reader's off-by-one
pillar: Media
status: done
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

- [x] A failing-first test builds the shape that triggers it — a guarded item at byte 0 with no
      blank line above it — and shows the rationale being missed.
      `ThePreambleReader.test_a_rationale_on_a_file_s_first_line_argues_an_enum_out`,
      `scripts/test-audio-claims.py:1157`.
- [x] The fix is the arithmetic, not a special case for byte 0: `rfind` returning `-1` and a real
      match at index 0 are different facts and the code should tell them apart.
      `scripts/check-audio-claims.py:770`, asserted by
      `ThePreambleReader.test_a_blank_line_at_the_very_top_still_bounds_the_preamble`.
- [x] The guard's own blindness assertions cover the shape, so it stays covered when the reader is
      next touched. `ThePreambleReader` holds both rules and the reader itself against the shape,
      in both directions — the rationale and the attribute — `scripts/test-audio-claims.py:1129`.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-09: filed at integration. `M-92`'s implementor deliberately did not file it mid-wave,
  reasoning that a new story file would leave the fenced board stale and redden the wave gate. That
  is the right call for an implementor and the wrong outcome for the finding, so the coordinator
  files it — which is what the board being fenced is *for*.
- 2026-08-09: implemented on `impl/X-131`. `ThePreambleReader` in `scripts/test-audio-claims.py`
  was written first and ran red at the merge base `d3f6324`, four of six:

  ```
  $ python3 -m unittest scripts/test-audio-claims.py -k ThePreambleReader
  FAIL: test_no_blank_line_above_keeps_the_whole_preamble
  AssertionError: '/// Exhaustive by design: a stream flows one way or the other.\n'
                != '// Exhaustive by design: a stream flows one way or the other.\n'
  FAIL: test_a_rationale_on_a_file_s_first_line_argues_an_enum_out
  AssertionError: [] != ['…/lib.rs:2 `Flow` is reachable from the crate root and exhaustive; add
    `#[non_exhaustive]` or an adjacent `/// Exhaustive by design:` rationale']
  FAIL: test_a_rationale_on_a_file_s_first_line_argues_a_struct_out
  AssertionError: [] != ['…/lib.rs:2 `Encoded` is reachable from the crate root and has public
    fields; add `#[non_exhaustive]` or an adjacent `/// Complete by design:` rationale']
  FAIL: test_an_attribute_on_a_file_s_first_line_guards_an_enum
  AssertionError: [] != ['…/lib.rs:2 `Flow` is reachable from the crate root and exhaustive; …']
  Ran 6 tests — FAILED (failures=4)
  ```

  The first of those is the defect stated plainly: the file's opening `/` is gone. The other three
  are what it costs — the rationale and the attribute both stop matching, and the reader reports a
  type whose guard is present. The mirror direction is worth recording: on this workspace the miss
  produces a **false positive**, not a let-through, because every rule reads its guard as a
  substring that has to be *present*.

  Fix is the arithmetic in `preamble()`: `rfind`'s `-1` and a real match at index 0 are separated
  before the addition, so no-blank-line starts the slice at 0 while a blank line at the very top
  still bounds it. No special case for offset 0.

  After: all 109 tests in `scripts/test-audio-claims.py` pass, and `./scripts/check-audio-claims.py
  --check` exits 0 with its counts unmoved — 5 crates / 100 enums outside, 2 crates / 147 structs
  outside. Unmoved is the point: the shape does not occur on this tree, so a changed count would
  have meant the reader now sees something different about real code.

  **Owed CHANGELOG sentence** (fenced file, not edited here): *Fixed — the audio-claims guard reads
  the whole preamble of an item in a file's first paragraph; it previously dropped the file's first
  character and could report a type whose `#[non_exhaustive]` or rationale was present.*

- 2026-08-09: closed at the `1.0.0-rc.14` boundary, against the wave gate run on this tree.
