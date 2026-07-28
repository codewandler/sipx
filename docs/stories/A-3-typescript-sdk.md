---
id: A-3
title: The TypeScript SDK and the two reference applications
pillar: Application
status: backlog
priority:
design: docs/designs/ts-sdk.md
epic: app-host
areas: [sipx-app]
note: app-host phase 2 · the reference apps are the contract's exit-from-experimental gate
---

# The TypeScript SDK and the two reference applications

## Goal
`@sipx/app` (working name): the awaitable handler API over session mode, plus the inbound IVR
and outbound notifier reference applications that prove it — and the contract.

## Acceptance
- [ ] The SDK surfaces every contract verb and event and adds no vocabulary; types are
      generated from the contract schema, the imperative layer is hand-written.
- [ ] Snapshot discipline: state replaced wholesale per event; a test proves a missed-then-
      redelivered event leaves the SDK's view equal to the wire's.
- [ ] Both reference applications pass under the harness and against real calls.
- [ ] Killing the app process mid-call yields the declared failure outcome, observed from the
      call side.
- [ ] The package states the contract's experimental status at install/import.

## Progress
- Not started. Needs `A-4`'s session binding (or ships together — the design allows either).

## Notes
- Record-verb payload delivery is a contract question — settle it in
  [`app-contract.md`](../specs/app-contract.md) before shipping `record` in the SDK (see the
  design's open question).
