---
id: A-5
title: Implement the embedded TypeScript runtime
pillar: Application
status: backlog
priority:
design: docs/designs/embedded-runtime.md
epic: app-host
areas: [sipx-app]
note: app-host phase 3 · needs A-6 (the binding spec) and A-3 (the SDK it hosts)
---

# Implement the embedded TypeScript runtime

## Goal
Run a `.ts` handler in-process on the chosen engine, behind the engine binding: same SDK API,
deny-by-default capabilities, one handler failure = one call's declared outcome.

## Acceptance
- [ ] The phase-2 reference applications run **unmodified** as embedded handlers; the parity
      suite (same files, session vs engine binding, same outcomes) passes under the harness.
- [ ] A handler reaching beyond its grants gets the same typed refusal an ungranted verb
      gets; tested.
- [ ] A throwing handler takes exactly its own call to `on_unreachable`; a sibling call on
      the same host is proven undisturbed.
- [ ] Exactly one crate imports the engine, and no kernel crate's dependency tree changes.

## Progress
- Not started.

## Notes
- The engine decision (`deno_core`, with fallback recorded) is in the design; this story does
  not relitigate it, it validates it.
