---
id: M-10
title: Expose media quality statistics
pillar: Media
status: ready
priority: 8
design: docs/designs/media.md
epic: depth
areas: [sipx-media]
note:
---

# Expose media quality statistics

## Goal
Make a call's quality readable while it is running, from the library and from the CLI.

## Acceptance
- [ ] Loss, jitter, round-trip time and MOS estimate are readable mid-call.
- [ ] Round-trip time is computed from the RTCP report round trip (RFC 3550 §6.4.1), not
      guessed from anything else.
- [ ] `sipx dial --stats` reports them on exit, in both output formats.
- [ ] Failing-first test: `statistics_report_the_loss_that_was_actually_injected`.

## Progress
- Not started. `M-6` computes the underlying counters; this is about making them reachable.
