---
id: M-77
title: Carry a refused frame forward in voice detection
pillar: Media
status: done
priority:
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

- [x] When `AudioAnalyzer::process` refuses a frame in `crates/sipx-call/src/voice.rs`, the
      discontinuity is carried forward and applied to the next accepted frame rather than dropped.
      → `crates/sipx-call/src/audio_feed.rs:owed`, set at the refusal and merged into the next
      frame's declared discontinuity; `voice.rs` reaches it through `AudioFeed::offer`.
- [x] A failing-first test proves a voice transition spanning a refused frame is reported with the
      epoch restarted, instead of being missed.
      → `voice::tests::a_frame_the_analyser_refused_breaks_the_epoch_instead_of_vanishing`; at the
      merge base it observed **no events at all**, which is the missed transition.
- [x] The `// discard:` reason at that site is replaced by the behaviour it currently describes as
      missing.
      → `audio_feed.rs`'s refusal arm: the samples are still gone, and what is no longer discarded
      is the break they left.
- [x] `./scripts/gate.py` green, including `sipx-transport`'s discard-site check.
      *The discard-site check is green (`cargo test -p sipx-transport --test discards
      --all-features`, 3 passed), as are `cargo test -p sipx-call -p sipx-audio --all-features`,
      clippy and `cargo fmt --all --check`. The full gate is the wave coordinator's, once per wave.*

## Progress

- 2026-08-08: filed while integrating `M-59`, which found and fixed exactly this hole in its own
  join — a frame the analyser refused used to vanish silently, so a later report summed across it
  and named coverage it never measured. `M-59`'s fix is an `owed: Option<DiscontinuityKind>` carried
  to the next frame; `M-58`'s `voice.rs` still has the unfixed shape. The consequence there is
  qualitative rather than quantitative — a missed transition rather than a wrong number — which is
  why it was annotated rather than counted, and why it needs fixing rather than measuring.

- 2026-08-08: **implemented, and the Notes' shared mechanism was taken.** The fix is not a second
  copy of `M-59`'s `owed`: `crates/sipx-call/src/audio_feed.rs` now holds the whole seam→analyser
  prologue once — the accepted-frame counter, the unflagged-gap detection, the owed break and the
  refusal — and `voice.rs` and `signal_metrics.rs` each keep only their own reading of what the
  analyser observed. The duplicate was the defect's actual shape: the same forty lines existed
  twice, `M-59` fixed one of them, and this story is the other one. Held once, there is no second
  one to forget next time.

  The two joins were already identical up to the shaping step, so nothing needed reconciling except
  the hole itself. The remaining asymmetry is deliberate and is the whole of each module: voice
  latches a state and so retries an undelivered transition through a reserved slot; a metric is
  history and so is emitted once. `M-59`'s point that its reducer takes `&Observation` and could
  fan out from one analyser is a *further* step — one analyser feeding both readings — and is not
  taken here: `Call` attaches each join through its own seam attachment with its own
  `AnalysisProfile`, so merging them is a lifecycle change to `call/mod.rs`, not a refactor of
  these two.

  What the break buys voice is qualitative, as this story said. The failing-first test opens voice,
  offers a frame past the contract's per-frame ceiling (§7.3, refused), then feeds silence and
  voice again. At the merge base **nothing at all was reported**: the analyser never learned the
  stream was holed, so voice stayed open across audio nobody measured and no transition was
  produced on either side of it. With the break owed forward, the next accepted frame restarts the
  epoch — `VoiceEnded { Cut }` at sample 160, the last position anyone actually measured, and the
  next start numbered from the new epoch's own origin rather than from one spanning the hole.

  Two things worth carrying forward:

  - **The refusal is still a discard, and the comment still says so.** A refusal mutates nothing
    (§7.3), so those samples have no second presentation and never will; what is no longer
    discarded is the *break* they left. `sipx-transport`'s scan is green because the site is
    explained truthfully, not because the marker was left behind.
  - **The non-`Signed16` arm owes a break too.** It is unreachable through `Call`, and `M-59`
    already owed there while `M-58` did not; sharing the code settled it in the honest direction.

- 2026-08-08: closed in the `1.0.0-rc.7` boundary.

## Notes

- `M-59`'s `a_frame_the_analyser_refused_breaks_the_epoch_instead_of_vanishing` is the test to
  mirror.
- The two joins should end up sharing this behaviour rather than each carrying its own copy; `M-59`
  notes its reducer already takes `&Observation` and is shaped to fan out from one analyser.
