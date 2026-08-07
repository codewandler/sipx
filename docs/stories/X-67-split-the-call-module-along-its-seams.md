---
id: X-67
title: Split the call module along its seams
pillar: Build
status: in-progress
priority: 20
design:
epic: depth
areas: [sipx-call]
predicate:
announcement:
note: call.rs is 6560 lines, ~6100 of them production · hold, transfer, session timers, re-INVITE and ICE restart in one file · follow-up
---

# Split the call module along its seams

## Goal

Break the largest module in the workspace into files that each hold one concern, so a reader can
find the state machine they need without reading six thousand lines, and a diff stops touching an
unrelated feature's neighbourhood.

## Acceptance

- [ ] `crates/sipx-call/src/call.rs` is decomposed along concern seams — at minimum hold and resume,
      transfer, session timers, re-INVITE and offer/answer, and ICE restart — into sibling modules,
      with `call.rs` retaining the `Call` type and its lifecycle.
- [ ] **The public API is unchanged.** No path, name, or signature a user can observe moves. Proven by
      the existing test suite passing without edits to test code, and by `cargo doc` producing the
      same public item set.
- [ ] This is a pure move. **No behaviour change, no bug fix, and no cleanup rides along** — anything
      found while moving is filed as its own story rather than fixed in this diff.
- [ ] Doc comments move with the code they document, and every intra-doc link still resolves under
      `RUSTDOCFLAGS=-D warnings`.
- [ ] Test modules move with their subject and keep their `#[allow]` annotations at module scope per
      `AGENTS.md`.
- [ ] `./scripts/gate.py` green.

## Progress
- (not started)

## Notes
- Found by the 2026-08-04 capability review. Nine files exceed 1,500 lines; this is the worst and the
  one whose concerns separate most cleanly. `crates/sipx-media/src/session.rs` (4,403) and
  `crates/sipx-transport/src/endpoint.rs` (3,256) are candidates for the same treatment, but the
  endpoint's size follows from the single-serialized-event-loop design and should not be split just
  to hit a number — file a separate story if either is worth doing.
- Follow-up and deliberately low priority: it is a readability investment with a real review cost and
  no user-visible result. Schedule it when it is not competing with reachability work.
- Keeping it a pure move is what makes the diff reviewable at all. A mixed diff of this size is not.
