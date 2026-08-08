---
id: P-18
title: "Make the bounded load tools interoperate by default"
pillar: Phone
status: done
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

- [x] `docs/specs/diagnostic-phone.md` defines one shared workload-mode vocabulary for both commands,
      including the default, SDP/media behavior, terminal reasons and exit mapping.
- [x] The neutral default remains the bounded signalling workload: `load` does not unconditionally
      start media against a signalling-only responder. Generated media is selected explicitly and
      symmetrically on both sides.
- [x] A failing-first process test runs the review's default pair and observes the current
      one-attempt `interrupted` success before the fix.
- [x] The corrected default pair admits the requested 20 calls, reaches the configured concurrency
      and call bounds, drains to zero, reports `completed`, and exits 0 with no media requirement.
- [x] Selecting generated media on both commands retains the existing RTP proof. Selecting
      incompatible explicit peer modes is refused before dialog admission with a nonzero terminal
      result and an actionable message; an invalid local mode is refused before I/O with exit 2.
- [x] An internal call/media worker error stops admission, cancels and joins owned calls, reports
      `failed`, exits nonzero and cannot be mislabeled as an operator interrupt.
- [x] Help, the CLI reference and bounded-load examples are generated/synchronized from the same
      mode contract. Focused load tests and the complete repository gate are green.

## Review evidence

Finding 5 ran the documented defaults: the responder created no media, `load` failed its mandatory
playback after one attempt, then emitted `interrupted` and exited 0 without a diagnostic.

## Progress

- Failing-first: `default_load_pair_completes_the_requested_signalling_workload` observed one
  attempted call, zero connected calls, `status: interrupted` and exit 0 before the fix.
- The shared typed mode now drives a bodyless signalling default or an explicit generated-media
  workload. The paired marker turns mismatches into a pre-admission 488 and failed terminal result.
- Internal worker failures retain their reason and exit 1 after owned work drains; signalling
  summaries contain no synthetic media samples.
- Focused validation is green: strict all-target/all-feature clippy, the CLI reference and docs-link
  checks, provenance and fixed-sleep checks, the five P-18 process tests, load unit tests, responder
  unit tests, and `cargo test -p sipx-cli --all-features`.
- Per the working-session instruction, derived regeneration and the complete gate are deferred to
  push time; the final acceptance item therefore remains open.
