---
id: P-18
title: "Make the bounded load tools interoperate by default"
pillar: "Phone"
status: ready
priority: 1
epic: diagnostic-automation
areas: [sipx-cli]
design: docs/designs/diagnostic-automation.md
note: "external review finding 5 · default load exits success after one unusable signalling-only call"
---

# Make the bounded load tools interoperate by default

## Goal

Make the documented default `load` and `load-responder` commands measure the same workload. A run
that cannot execute its selected workload must fail explicitly rather than report interruption and
success after one attempted call.

## Acceptance

- [ ] `docs/specs/diagnostic-phone.md` defines one shared workload-mode vocabulary for both commands,
      including the default, SDP/media behavior, terminal reasons and exit mapping.
- [ ] The neutral default remains the bounded signalling workload: `load` does not unconditionally
      start media against a signalling-only responder. Generated media is selected explicitly and
      symmetrically on both sides.
- [ ] A failing-first process test runs the review's default pair and observes the current
      one-attempt `interrupted` success before the fix.
- [ ] The corrected default pair admits the requested 20 calls, reaches the configured concurrency
      and call bounds, drains to zero, reports `completed`, and exits 0 with no media requirement.
- [ ] Selecting generated media on both commands retains the existing RTP proof. Selecting
      incompatible explicit modes is refused before admission with exit 2 and an actionable message.
- [ ] An internal call/media worker error stops admission, cancels and joins owned calls, reports
      `failed`, exits nonzero and cannot be mislabeled as an operator interrupt.
- [ ] Help, the CLI reference and bounded-load examples are generated/synchronized from the same
      mode contract. Focused load tests and the complete repository gate are green.

## Review evidence

Finding 5 ran the documented defaults: the responder created no media, `load` failed its mandatory
playback after one attempt, then emitted `interrupted` and exited 0 without a diagnostic.
