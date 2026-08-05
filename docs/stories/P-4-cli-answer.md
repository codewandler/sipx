---
id: P-4
title: Implement `sipx answer`
pillar: Phone
status: done
priority: 4
design: docs/designs/phone.md
epic: cli
areas: [sipx-cli]
note:
---

# Implement `sipx answer`

## Goal
Answer incoming calls from the command line, which is what makes the dial tests possible
without a third-party server.

## Acceptance
- [x] `sipx answer` waits for a call, answers it, and reports the caller.
- [x] `--play FILE.wav` plays a clip to the caller; `--record FILE.wav` records them.
- [x] `--reject` and `--busy` answer with the corresponding status instead.
- [x] `--once` exits after one call; otherwise it keeps answering.
- [x] DTMF the caller sends is reported as it arrives.
- [x] Failing-first test: `answer_accepts_a_call_and_records_the_caller`.

## Progress
- Done. `sipx answer`, with `--play`, `--record`, `--reject` and `--busy`.
- It announces the port it bound before waiting, which is what makes the tests race-free: a
  script starts the caller only once the answerer is listening, on a port the OS chose rather
  than one anybody guessed.
- DTMF the caller sends is reported in the result.
