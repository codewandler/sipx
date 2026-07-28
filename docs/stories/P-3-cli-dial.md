---
id: P-3
title: Implement `sipx dial`
pillar: Phone
status: done
priority: 3
design: docs/designs/cli.md
epic: cli
areas: [sipx-cli]
note:
---

# Implement `sipx dial`

## Goal
Place a call from the command line, play audio into it and record what comes back.

## Acceptance
- [x] `sipx dial sip:target` places a call and reports when it connects.
- [x] `--play FILE.wav` plays a clip into the call; `--record FILE.wav` records the far end.
- [x] `--duration` and `--hangup-after` bound the call so a script cannot hang forever.
- [x] `--dtmf 1234` sends digits once the call is up.
- [x] The exit code distinguishes answered, rejected, busy and timed out.
- [x] Failing-first test: `dial_plays_a_file_and_records_the_far_end`, between two sipx
      processes.

## Progress
- Done. `sipx dial`, with `--play`, `--record`, `--dtmf` and `--duration`.
- The clip is read *before* the call is placed. Failing afterwards means hanging up on someone
  for a mistake that was visible beforehand.
- A clip at the wrong sample rate is refused by name. Playing 44.1 kHz samples at 8 kHz
  produces audio that is recognisably wrong rather than obviously broken, which is harder to
  diagnose than a refusal.
- One real bug the acceptance test caught: `dial` hung up while packets were still in the
  paced send queue, so the last DTMF digit never left. `MediaSession::flush` now drains it
  first — sending is paced, so `play` returns long before the audio is on the wire.
