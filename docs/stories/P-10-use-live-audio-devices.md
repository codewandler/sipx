---
id: P-10
title: Use live audio devices without putting device IO in the media core
pillar: Phone
status: done
priority: 8
design: docs/designs/phone.md
epic: phone
areas: [sipx-cli]
note: bounded CLI-only device IO; Linux release targets, macOS and Windows compile checks
---

# Use live audio devices without putting device IO in the media core

## Goal

Let a person speak and listen through the diagnostic phone while keeping every core/media crate
independent of platform device APIs.

## Acceptance

- [x] A feature-gated CLI driver lists stable device identifiers and opens explicitly selected input
      and output devices as specified in §3.
- [x] Device format conversion is bounded and observable; a busy, absent or unsupported device fails
      rather than falling back silently.
- [x] Builds without the feature retain every WAV/generator command and take no device dependency.
- [x] Linux x86_64/arm64 runs have loopback-device tests; macOS and Windows compile-check the feature.
- [x] `DPH-7` and `DPH-12` are failing-first tests, and shutdown joins every device worker.

## Progress

- Done. `sipx devices` lists stable backend identifiers without opening streams, while `dial` and
  `answer` accept exact device endpoints behind the leaf-only `device-audio` feature. Callbacks
  exchange PCM through 50-frame queues, convert only bounded advertised formats, and report dropped
  input/output plus inserted output silence. DPH-7 holds typed pre-signalling failure; DPH-12 drives
  a file-backed ALSA microphone, checks discovery, compares its received clip with the WAV path, and
  opens the same identifier for bounded output shutdown. CI runs that vector on Linux x86_64 and
  arm64 and compile-checks macOS and Windows; the feature matrix proves the small build has no device
  dependency. `./scripts/gate.py` passed all 25 steps before this story was closed.
