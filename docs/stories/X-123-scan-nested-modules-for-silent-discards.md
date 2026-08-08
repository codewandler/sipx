---
id: X-123
title: Scan nested modules for silent discards
pillar: Build
status: done
priority:
design:
epic: test-surfaces
areas: [sipx-transport, sipx-call]
predicate:
announcement:
note: the discard guard reads one directory level, so every file X-67 moved into call/ and coupling/ has been unscanned since the split
---

# Scan nested modules for silent discards

## Goal

Make the §12.1 discard guard reach every source file in a crate, not only the ones directly under
`src/`.

## Acceptance

- [x] `crates/sipx-transport/tests/discards.rs` walks a crate's sources recursively, so nested
      modules are scanned.
- [x] A failing-first test proves the guard reports an unexplained discard placed in a nested
      module — it does not today.
- [x] The scan asserts it found a plausible number of files, so a future layout change cannot make
      it silently scan nothing again. The file's own documentation already argues for this: *"a
      guard that silently scans nothing is indistinguishable from a codebase with nothing to find."*
- [x] Every discard site the widened scan newly reveals is resolved — counter or reason — or filed.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `M-77`'s adjacent findings and verified: `sources_of` calls
  `std::fs::read_dir` once, without recursion, so **nine files** — `crates/sipx-call/src/call/*.rs`
  and `crates/sipx-call/src/coupling/*.rs` — have never been scanned. `X-67` moved the call module
  into exactly that layout, and `C-7` added `coupling/`, so the guard stopped covering the largest
  module in the workspace at the moment it became worth covering. `M-77` deliberately placed
  `audio_feed.rs` at the top level of `src/` so its own work stayed under the scan.

- 2026-08-08: implemented. `sources_of` now walks the tree instead of listing one directory, and
  the widened scan immediately revealed **seven unexplained discard sites**, all in
  `crates/sipx-call/src/call/mod.rs` — two capability-refusal logs from `M-58`/`M-59`'s
  renegotiation follow-up, and five best-effort teardown awaits on error paths. Each carries a
  `// discard:` reason rather than a counter: none is a media frame, so counting them would put a
  per-call capability ending in a frame-loss statistic. A `sipx-call`-specific assertion now fails
  if the scan ever covers no nested module again, because the previous shape returned a
  plausible-looking file list while covering none of `call/` or `coupling/`.

## Notes

- This is the same class as `X-122`'s vacuous assertions and `X-116`'s coverage figure counting its
  own tests: an artefact that reads as evidence while measuring less than it claims. Expect the
  widened scan to reveal real sites; budget for them rather than assuming a clean result.
- `M-45` separately found that `crates/sipx-media/tests/discards.rs` only fires on `let _ = …` at
  statement start or a `tracing::` line containing a loss word, so a bare expression statement
  dropping a discard slips through. Different guard, same weakness — worth fixing together.
