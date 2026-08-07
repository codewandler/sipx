---
id: P-22
title: "Handle supervisor termination consistently"
pillar: "Phone"
status: in-progress
epic: phone-lifecycle
areas: [sipx-cli]
design: docs/designs/phone-lifecycle.md
note: "follow-up external review finding 5 · load-responder handles SIGINT but loses cleanup and its summary on SIGTERM"
---

# Handle supervisor termination consistently

## Goal

Give every long-running diagnostic command the same bounded shutdown path for interactive
interrupt and supervisor termination. A stop signal must request cancellation, join owned work and
emit the command's terminal record instead of invoking the platform's default immediate death.

## Acceptance

- [x] The diagnostic-phone spec defines supported stop signals, repeated-signal behavior, terminal
      reason/exit mapping and the finite grace period for every long-running command.
- [x] Failing-first process tests reproduce the review matrix: natural and SIGINT
      `load-responder` runs emit summaries, while SIGTERM currently exits by signal after only its
      readiness record.
- [x] `load-responder`, `load`, `dial` and `answer` route both interactive interrupt and supervisor
      termination through one cancellation abstraction on supported platforms.
- [x] Admission closes before cleanup. Dialog, media and transport tasks join within the configured
      bound, and exactly one terminal JSON/text result follows every earlier readiness record.
- [x] A second termination during graceful cleanup has explicit bounded semantics and cannot leave
      an orphan process group or duplicate a terminal report.
- [x] Platform-specific signal support is feature-gated or typed rather than silently promised;
      unsupported platforms retain compile-tested cancellation through the library boundary.
- [ ] Focused signal/process tests, help/reference documentation and the complete repository gate
      are green.

## Review evidence

The follow-up review observed `load-responder` complete naturally and handle SIGINT with a
`status=interrupted` summary, but SIGTERM exited `-15` after readiness with no cleanup summary—the
signal used by common process supervisors.

## Progress

- The diagnostic-phone contract now defines portable SIGINT and Unix SIGTERM support, first- and
  repeated-signal semantics, per-command cleanup bounds, terminal fields and exit mapping. Board
  regeneration and the complete gate remain deferred to the requested push boundary.
- One shared process-stop listener now drives `dial`, `answer`, `load` and `load-responder`.
  Focused process tests cover natural completion, both supported Unix signals, pending and
  confirmed calls, waiting admission, and a repeated SIGTERM while responder cleanup is blocked on
  BYE. CLI contract, documentation-link, fixed-wait, provenance and strict all-feature lint checks
  pass; the complete gate remains deferred to push.
