---
id: A-1
title: Finish the host configuration and failure-semantics schema
pillar: Application
status: ready
priority: 6
design: docs/designs/app-host.md
epic: app-host
areas: [sipx-app]
note: app-host phase 1 · spec work, no dependency on the app-sdk stories
---

# Finish the host configuration and failure-semantics schema

## Goal
Turn [specs/host-config.md](../specs/host-config.md) from draft to normative: concrete syntax,
listener schema, app/binding/grants/failure tables, reload semantics — with vectors.

## Acceptance
- [ ] The spec's §3 open points are closed and every normative point has at least one vector
      (valid document, rejected document with the reason, reload accepted, reload rejected,
      live-call policy retention across reload).
- [ ] Failure-semantics fields are byte-identical in name and default to
      [`app-contract.md`](../specs/app-contract.md) §9.2.
- [ ] Secrets are by-name references; a vector shows a document with no secret material in it.
- [ ] The multi-app-vs-multi-process stance is recorded as explicitly open with what phase 4
      needs preserved either way.

## Progress
- Not started.

## Notes
- No dependency on the `app-sdk` stories — this can run beside them.
