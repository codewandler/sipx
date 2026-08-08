---
id: M-58
title: Detect voice activity with typed call events
pillar: Media
status: in-progress
priority: 13
design: docs/designs/call-audio-analysis.md
epic: call-audio-analysis
areas: [sipx-audio, sipx-call, app-sdk, audio-analysis, vad, m16]
predicate:
announcement:
note: after M-57 and M-54 · start, end and hangover through CallEvent and SDK
---

# Detect voice activity with typed call events

## Goal

Report deterministic voice-start, voice-end and hangover transitions for live call audio without a
speech model or a device-specific runtime.

## Acceptance

- [x] A failing-first sample corpus produces stable voice-start and voice-end events at the sample
      positions specified by M-57, including the declared hangover behavior.
- [x] Direction, call identity, observation sequence and sample time reach `CallEvent` and generated
      SDK bindings without polling or an implementation-specific handle.
- [x] Silence, discontinuity, format change, reset and call cancellation each have one documented
      transition sequence and cannot leave activity latched after teardown.
- [x] Event delivery is bounded and cannot block call media; coalescing/drop policy preserves the
      latest state and terminal reset.
- [x] Tests cover two simultaneous calls and prove their calibration, sequence and events never
      cross; no fixed wall-clock sleep establishes ordering.
- [x] Existing DTMF events remain unchanged, generated bindings and docs are updated, and the full
      gate is green.

## Progress

- **The analyser is `sipx-audio`'s `analysis` module**, implementing `docs/specs/call-audio-processing.md`
  whole: §5's integer window predicates with the `i64` width proof asserted rather than assumed,
  §6's activity/hangover/silence-timeout state machines, §7's typed reset versus refusal, and §8's
  bounds — a preallocated observation ring that coalesces overflow into a counted `Lost` marker and
  allocates nothing after construction. `crates/sipx-audio/tests/call_audio_analysis.rs` replays all
  25 `CAP-*` vectors of §11, and it is the failing-first test: at the merge base it does not compile,
  because the module it names does not exist.
- **One vocabulary, not two.** `AudioDirection` and `DiscontinuityKind` moved down into
  `sipx-audio`, where the analyser that defines them lives; `sipx-media`'s seam re-exports them, so
  `sipx_media::AudioDirection` still resolves and no second spelling exists. The seam's coalescing
  severity order stayed in `sipx-media`, since it is that seam's rule rather than shared vocabulary.
- **The call side is `sipx-call`'s `voice` module.** `Call::detect_voice_activity(profile)` attaches
  through `M-54`'s seam — no second tap — and from then on `CallEvent::VoiceStarted` and
  `CallEvent::VoiceEnded` arrive on the call's existing stream: no handle to hold, nothing to poll.
  Each carries the `Call-ID`, the direction, this call's observation sequence, and the sample
  position with the rate it is counted at.
- **Only transitions are reported, so a drop may not lie.** A transition the consumer had no room
  for is retried against the *latest* state rather than queued, which collapses flapping and leaves
  the application holding where the call actually is. The terminal cut travels through a slot
  reserved when detection starts, and the call **cancels and joins** the watcher before
  `EventSink::end`, so the cut always precedes `Ended` and the stream keeps its one ordering
  promise. Detection is call-owned policy like the RTCP hook: a re-INVITE that replaces the media
  session stops and joins the old watcher (delivering its cut) and re-attaches to the new one, so
  activity is never latched across a renegotiation.
- **The wire is `call.voice.started` / `call.voice.ended`** (`app-contract.md` §5.3, a compatible
  `v1` addition under §4). Both rows are derived-tested: the event-type set and the two inline
  value lists are read out of the spec by `tests/spec_tables.rs`. Which call an observation belongs
  to is the envelope's §5.2 `call.id`, not a second spelling in the event body. `call.dtmf` and
  every other existing event are untouched.
- No speech model is loaded anywhere on this path, and none is reachable from it.
- **`M-57`'s last acceptance row is now satisfiable**: the API its implementing stories owed exists
  (`sipx_audio::analysis`, `Call::detect_voice_activity`, the two wire events) and the focused
  spec/vector checks are green against it. Closing that row is `M-57`'s to do, not this story's.
