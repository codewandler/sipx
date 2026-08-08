---
id: M-54
title: Expose bounded call PCM processing and resampling
pillar: Media
status: done
priority:
design: docs/designs/local-speech.md
epic: local-speech
areas: [sipx-media, sipx-audio, app-sdk, speech, audio-analysis, m16]
predicate:
announcement:
note: after A-25 and M-43 · shared seam for local speech and deterministic analysis
---

# Expose bounded call PCM processing and resampling

## Goal

Attach bounded application media processors to a live call through one direction-aware PCM seam,
reusing M-43's unopinionated format conversion rather than creating speech-specific media plumbing.

## Acceptance

- [x] A failing-first test attaches processors to received and transmitted audio and observes PCM
      frames with direction, sample format, sample time, sequence and discontinuity metadata.
- [x] A processor requests one supported sample format and rate; conversion reuses M-43 and a typed
      refusal names unsupported conversion rather than distorting or dropping the call.
- [x] Per-call queues are finite and a slow processor follows a documented frame-loss policy,
      receives a discontinuity and cannot block RTP decode, encode, playback or capture.
- [x] Attach, detach, call cancellation and processor failure release every buffer and task, with
      observable completion and no fixed sleep standing in for ordering.
- [x] Two simultaneous consumers — one speech provider and one deterministic analyser — receive the
      declared fan-out semantics without sharing mutable state across calls.
- [x] Existing playback, recording, DTMF and RTCP behavior remains green under the full gate.

## Progress

- Backlog. Depends on A-25 and M-43; shared by both M16 epics.
- 2026-08-08: **readiness audit — ready.** One instruction for the implementor: the seam
  specification is in scope, and the loss policy derives from `call-audio-processing.md` §8.3
  together with `speech-providers.md` §8 rather than being invented here.
- 2026-08-08: **implemented.** The seam is specified first in
  [`docs/specs/call-audio-seam.md`](../specs/call-audio-seam.md) — the document both
  `call-audio-processing.md` §9 and `speech-providers.md` §1 delegate to and neither defined — and
  implemented in `crates/sipx-media/src/processing.rs` behind `MediaSession::attach_processor`.

  Decisions worth carrying forward:

  - **Two taps, and no third.** Received audio is tapped at the jitter buffer's output inside
    `deliver`, so a processor observes the played order; transmitted audio is tapped in `send_loop`
    after the mute gate and before encoding, so a muted call is never reported as transmitting.
  - **The loss policy is derived, not invented.** Drop-oldest and "one `Discontinuity` names the
    accumulated loss" is `speech-providers.md` §8/§5; coalescing into the retained entry rather than
    blocking or growing is `call-audio-processing.md` §8.3, applied at the head because this queue
    drops from the head. The capacity domain 2..=4,096 is `call-audio-processing.md` §5.1's and the
    default of 32 frames is `speech-providers.md` §8's.
  - **Conversion is deferred to the consumer.** The media loops queue session-rate samples; each
    handle owns its `LinearResampler` and converts in `recv`. That keeps the media loops' per-frame
    cost independent of how many processors want how many formats.
  - **The seam owns no task.** It adds no handle to `media-runtime.md` §2.1's owner set. Attach,
    detach, drop, `stop`, `shutdown` and `reconfigure` are all synchronous registry transitions.
  - **Attachments survive renegotiation.** `reconfigure` carries them to the replacement generation
    and re-anchors them with `Realign`, so an application does not re-attach across a re-INVITE.

  Left for the stories that consume it: `M-57`'s processor type is not built here (this story is its
  prerequisite, not its closure), and nothing is surfaced through `sipx-call` or the application SDK
  — `Call` still does not hand out its `MediaSession` (`C-6`), so `A-26`/`A-27`/`M-58` own that.
- 2026-08-08: closed against a green gate — `./scripts/gate.py` reported **40 steps, all green** on `main` at `1256b8e`. An earlier run on the same tree failed two `sipx-cli` audio tests; both pass in isolation and in the full 83-test `cli.rs` binary, and `M-59` independently hit the same class on a sibling test while five other checkouts were building on this shared box. `X-118` owns that flakiness.
