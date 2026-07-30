---
id: P-10
title: Use live audio devices without putting device IO in the media core
pillar: Phone
status: backlog
priority: 8
design: docs/designs/phone.md
epic: phone
areas: [sipx-cli]
note: blocked by P-9; Linux release target, macOS and Windows compile checks
---

# Use live audio devices without putting device IO in the media core

## Goal

Let a person speak and listen through the diagnostic phone while keeping every core/media crate
independent of platform device APIs.

## Acceptance

- [ ] A feature-gated CLI driver lists stable device identifiers and opens explicitly selected input
      and output devices as specified in §3.
- [ ] Device format conversion is bounded and observable; a busy, absent or unsupported device fails
      rather than falling back silently.
- [ ] Builds without the feature retain every WAV/generator command and take no device dependency.
- [ ] Linux x86_64/arm64 runs have loopback-device tests; macOS and Windows compile-check the feature.
- [ ] `DPH-7` and `DPH-12` are failing-first tests, and shutdown joins every device worker.

## Progress

- Not started.
