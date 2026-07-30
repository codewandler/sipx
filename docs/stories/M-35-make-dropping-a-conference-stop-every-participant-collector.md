---
id: M-35
title: Make dropping a conference stop every participant collector
pillar: Media
status: done
priority: 4
design: docs/designs/media-runtime-safety.md
epic: media-runtime-safety
areas: [sipx-media]
predicate: 4
note: R-05 in the 2026-07-30 repository review — Conference Drop aborts only the mixer while detached collectors retain participant sessions
---

# Make dropping a conference stop every participant collector

## Goal

Make a conference own and terminate all of its participant workers so dropping it without an explicit
close cannot keep media sessions, sockets and collector tasks alive.

## Acceptance

- [ ] Specify conference worker ownership and the behavior of explicit close versus `Drop` before
      implementation.
- [ ] The conference retains cancellation and completion ownership for every participant collector;
      removing a participant, explicit close and `Drop` all use one idempotent shutdown mechanism.
- [ ] Dropping a conference terminates the mixer and every collector without requiring participants to
      produce another frame or close their own sessions first.
- [ ] Repeated close/drop paths do not double-signal, leak join handles or panic.
- [ ] Failing-first test: drop a conference with live participants without calling `close()`, retain
      weak references or equivalent task probes, and assert every collector and participant session is
      released within a bounded test deadline.
- [ ] Existing join, leave and N-1 mixing behavior remains covered after ownership changes.

## Progress

- Filed from R-05 in `docs/reviews/2026-07-30T07-50-49+02-00-repository-review.md`.
- M-12 proves functional mixing and explicit membership changes, but it does not test destructor
  cleanup. This is a new defect story rather than a duplicate of M-12.

## Notes

- The destructor must initiate bounded cleanup; it must not perform unbounded blocking in `Drop`.
