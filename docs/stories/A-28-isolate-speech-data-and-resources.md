---
id: A-28
title: Isolate speech data and resources with no default retention
pillar: Application
status: in-progress
priority: 12
design: docs/designs/local-speech.md
epic: local-speech
areas: [app-sdk, speech, privacy, security, m16]
predicate:
announcement:
note: after A-25 · gates provider delivery · explicit opt-in for retention or off-host processing
---

# Isolate speech data and resources with no default retention

## Goal

Make local speech processing private and bounded by default, with per-call ownership and explicit
host policy for every operation that retains data or sends it off the machine.

## Acceptance

- [x] The spec classifies audio, transcript, synthesized text, model state, credentials and derived
      caches, and sets no retention beyond the live operation as the default for user data.
- [x] Debug capture, persistent derived data and an off-host provider each require explicit host
      configuration and are visible through provider discovery and call events.
- [x] Every call has independent bounded queues, execution budget, cancellation and provider state;
      a failing-first concurrency test proves one call cannot receive another call's data or events.
- [x] Ordinary logs redact credentials, model paths, transcript text and synthesis input, while
      still reporting typed provider identity, lifecycle, limits and failure causes.
- [x] Cancellation and provider failure erase transient buffers and release accelerator and CPU
      resources; tests inspect cleanup instead of relying on elapsed time.
- [ ] The public privacy guide states local/offline defaults, opt-ins and operational limits, and the
      full gate is green.

## Progress

- Backlog. Follows A-25 and precedes the shipped providers.
- 2026-08-08: **readiness audit — the subject of this story does not exist yet.** `A-25` delivered a
  specification only: there is no provider registry, session type or speech logging anywhere under
  `crates/*/src`. Acceptance rows 3, 4 and 5 have nothing to run against, and row 2's "visible
  through call events" depends on `A-26`, which has not started. Deferred out of the rc.4 wave
  behind a new predecessor that builds the contract skeleton the spec describes.
- 2026-08-08: **implemented on `A-39`'s contract and `A-40`'s driver.** The subject now exists, so
  the three rows that had nothing to run against have types to run against.

  The spec gained **§11** (`docs/specs/speech-providers.md`): §11.1 classifies the six classes and
  says whose each one is, §11.2 makes "no retention beyond the live operation" the default for user
  data, §11.3 names the three opt-ins and widens §4 step 2 to admit the whole privacy declaration
  rather than the locality half, §11.4 puts redaction in the types, and §11.5 fixes the erase →
  release → close order at a stop. §3 gained `debug_capture` and `derived_cache`; §10 gained the
  `PRV-1`…`PRV-5` vectors.

  In `crates/sipx-media/src/speech/`: a new `privacy` module (`DataClass`, `Retention`,
  `RetentionOptIn`, `ProviderPrivacy`, `SpeechPrivacy`, `SpeechAdmission`, `Secret`, `Redacted`);
  `RefusalReason::RetentionRefused` and `SelectionContext::with_privacy` in `selection`;
  `Selected::admission` as the call event; redacting `Debug` on `Utterance`, `RecognitionFrame`,
  `SynthesisChunk` and `SynthesisInput`; and in `driver`, audio erasure at the stop, the ordered
  release of provider and seam attachment before the output stream closes,
  `SynthesisDriver::retained_audio`, and the ordinary `tracing` records the redaction makes safe.

  The vectors are a sibling test file, `crates/sipx-media/tests/speech_privacy.rs`, rather than more
  of `speech.rs` — that file is already ~2,700 lines of §10's `DIS`/`SEL`/`REC`/`SYN`/`LIF` vectors,
  and the `PRV` vectors are a different question about the same contract.

  Row 6 is left unticked deliberately: its guide half is shipped
  (`website/docs/reference/privacy.md`, in the sidebar's Reference section), and its second clause —
  a green full gate — is the wave gate, which this change did not run. Focused verification was
  green: `cargo test -p sipx-media --all-features`, `cargo clippy -p sipx-media --all-targets
  --all-features --no-deps -- -D warnings`, `cargo fmt --all --check`,
  `./scripts/check-app-surface.py --check`, `./scripts/check-provenance.sh`,
  `./scripts/check-fixed-sleep.py --check`, `./scripts/sync-website.py --check` and
  `./scripts/check-docs-links.py`.

  Still open for the epic, and not this story's: `A-26`/`A-27` carry the admission event onto the
  application SDK's own stream, and `X-105` turns the `PRV` vectors into the public testkit suite a
  downstream provider runs.
