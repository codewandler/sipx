---
id: M-77
title: Carry a refused frame forward in voice detection
pillar: Media
status: ready
priority: 9
design: docs/designs/call-audio-analysis.md
epic: call-audio-analysis
areas: [sipx-call, sipx-audio]
predicate:
announcement:
note: a frame the analyser refuses vanishes without breaking the epoch, so a voice transition spanning it can be missed
---

# Carry a refused frame forward in voice detection

## Goal

Make a refused frame break the analyser's epoch in voice detection, the way `M-59`'s signal reducer
already does, so a gap in the fed stream cannot be silently summed across.

## Acceptance

- [ ] When `AudioAnalyzer::process` refuses a frame in `crates/sipx-call/src/voice.rs`, the
      discontinuity is carried forward and applied to the next accepted frame rather than dropped.
- [ ] A failing-first test proves a voice transition spanning a refused frame is reported with the
      epoch restarted, instead of being missed.
- [ ] The `// discard:` reason at that site is replaced by the behaviour it currently describes as
      missing.
- [ ] `./scripts/gate.py` green, including `sipx-transport`'s discard-site check.

## Progress

- 2026-08-08: filed while integrating `M-59`, which found and fixed exactly this hole in its own
  join — a frame the analyser refused used to vanish silently, so a later report summed across it
  and named coverage it never measured. `M-59`'s fix is an `owed: Option<DiscontinuityKind>` carried
  to the next frame; `M-58`'s `voice.rs` still has the unfixed shape. The consequence there is
  qualitative rather than quantitative — a missed transition rather than a wrong number — which is
  why it was annotated rather than counted, and why it needs fixing rather than measuring.

## Notes

- `M-59`'s `a_frame_the_analyser_refused_breaks_the_epoch_instead_of_vanishing` is the test to
  mirror.
- The two joins should end up sharing this behaviour rather than each carrying its own copy; `M-59`
  notes its reducer already takes `&Observation` and is shaped to fan out from one analyser.
