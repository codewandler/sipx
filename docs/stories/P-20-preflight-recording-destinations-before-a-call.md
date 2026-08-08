---
id: P-20
title: "Preflight recording destinations before a call"
pillar: Phone
status: done
epic: diagnostic-automation
areas: [sipx-cli]
design: docs/designs/diagnostic-automation.md
note: "external review finding 13 · an uncreatable recording path is discovered only after the call ends"
---

# Preflight recording destinations before a call

## Goal

Refuse a recording destination that cannot be created before signalling begins, while preserving
captured audio safely across the remaining filesystem races and cleaning every temporary resource
on cancellation or failure.

## Acceptance

- [x] The diagnostic-phone spec defines recording output ownership for `--record` and
      `--audio-output wav:…`: preflight point, existing-file policy, temporary-file strategy,
      finalize/rename behavior and cleanup on every terminal call path.
- [x] Failing-first process tests use a nonexistent directory and a controlled unwritable
      destination, prove the current call connects before failure, then require usage/failure before
      transport bind with the exact path named.
- [x] Preflight opens or reserves a sibling destination without truncating an existing final file
      before the call. Captured samples are finalized atomically where the platform permits.
- [x] `dial` and `answer`, including both WAV option spellings, share one implementation and cannot
      drift in validation timing or overwrite policy.
- [x] A destination that becomes unavailable after preflight produces an honest terminal failure
      without discarding an already finalized recording or claiming the call never occurred.
- [x] Cancellation, remote hangup, media failure and write failure leave no orphan temporary files
      or file handles. Paths derived from network data are never used.
- [x] Positive recording/audio tests, CLI docs, platform-focused tests and the complete repository
      gate are green.

## Review evidence

Finding 13 completed a short connected call before discovering that the requested recording's
parent directory did not exist; playback input already failed at the correct pre-signalling point.

## Progress

- The review's connected-call transcript supplies the failing-first reproduction. The replacement
  process test covers both commands, both option spellings, a missing parent and a controlled local
  creation failure while proving that no SIP datagram is emitted.
- One shared reservation owns an exclusively created sibling temporary from preflight through Drop.
  Finalization writes and syncs the complete WAV, then atomically installs it without replacing a
  late competing file. Early return and finalization-failure tests leave no temporary behind.
- Focused unit and process tests, strict all-target/all-feature CLI Clippy, fixed-wait policy, docs
  links and provenance pass. Board regeneration, the complete gate and final status remain deferred
  to the requested push boundary.
