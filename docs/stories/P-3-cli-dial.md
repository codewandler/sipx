---
id: P-3
title: Implement `sipx dial`
pillar: Phone
status: done
priority: 3
design: docs/designs/phone.md
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
- [x] `--duration` bounds the established call and `--timeout` bounds the attempt, so a
      script cannot hang forever on either.
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
- `--timeout` bounds the attempt, and the bound lives in `sipx-call` rather than around it.
  Wrapping the call future in a timeout and dropping it would abandon the exchange partway —
  after a 200 OK but before the ACK, leaving the far end streaming into a closed port. Only
  code inside the exchange can send the CANCEL that stops a phone ringing, and only it can ACK
  a 200 that arrives during the race CANCEL cannot close.
- Its default is 20 seconds, deliberately *under* the transaction layer's 64·T1. Equal values
  make which one fires a matter of scheduling, and the error a script reads changes run to run.
- One real bug the acceptance test caught: `dial` hung up while packets were still in the
  paced send queue, so the last DTMF digit never left. `MediaSession::flush` now drains it
  first — sending is paced, so `play` returns long before the audio is on the wire.
