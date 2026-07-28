---
id: A-2
title: Implement the document-mode host over the contract interpreter
pillar: Application
status: backlog
priority:
design: docs/designs/app-host.md
epic: app-host
areas: [sipx-app]
note: app-host phase 1 · needs C-3, C-4, C-5 and M-17 first
---

# Implement the document-mode host over the contract interpreter

## Goal
The first running host in `crates/sipx-app`: answer calls, drive the `sipx-app-protocol`
interpreter (`C-5`), deliver envelopes to a webhook app per
[specs/webhook-binding.md](../specs/webhook-binding.md), execute returned programs.

## Acceptance
- [ ] The webhook binding spec's open points are closed and its vectors pass.
- [ ] The phase-1 shell proof passes: scripted webhook app + sipx CLI far end → answer,
      prompt, gather, asserted outcome; app stopped → declared `on_unreachable` outcome.
- [ ] Declared failure semantics are exercised for timeout, 5xx-past-budget and 4xx — under
      the harness (`A-7`) and once for real.
- [ ] No interpretation of instructions happens outside the `sipx-app-protocol` interpreter
      (review-level check named in the design).
- [ ] `sipx-app` stays a leaf: no kernel crate gains a dependency on it, and its own
      dependencies (HTTP, serialization) appear in no other crate's tree.

## Progress
- Not started. Needs `C-3`, `C-4`, `C-5` and `M-17` — same board, `app-sdk` epic.

## Notes
- `M-18` (mute) and `C-6` (bridge) are not needed for the phase-1 proof; their verbs surface
  as contract errors until those land, which the harness should assert rather than hide.
