---
id: A-21
title: Build a deterministic realtime peer
pillar: Application
status: backlog
priority: 2
design: docs/designs/openai.md
epic: openai
areas: [sipx-testkit, interop]
predicate:
announcement:
note: starts when A-19's spec lands — the peer implements the spec's other side, vector by vector
---

# Build a deterministic realtime peer

## Goal

Give `sipx-testkit` a loopback WSS server that speaks `docs/specs/openai-realtime.md` from
the far side, so the bridge's whole loop runs in the default CI matrix with no account, no
credential and no network — the interop peer criteria kept intact.

## Acceptance

- [ ] The peer accepts a `wss` upgrade with a bearer it is configured to expect, refuses a
      wrong or absent bearer the way the spec says the vendor does, acknowledges the
      session, consumes append events, and emits delta events carrying a distinct known
      tone — each behaviour holding to the spec's vectors, cited by vector name in the
      tests.
- [ ] Cancel is honoured mid-response: after the client's cancel event the peer sends no
      further deltas for that response, so a bridge test can assert truncation as a fact.
- [ ] Negative modes are first-class configuration: wrong-bearer refusal, a malformed event,
      a mid-call stall, an oversize frame — each drives one row of the spec's failure
      taxonomy, and each mode has a test proving the peer actually misbehaves (a stand-in
      whose negatives are vacuous proves nothing).
- [ ] Deterministic and bounded: no fixed wall-clock duration standing in for a
      happens-before (`check-fixed-sleep.py` clean), every task cancellation-safe, the peer
      shuts down when dropped.
- [ ] Runs in the default `cargo test` matrix — no Docker, no credentials, fixture
      certificates generated per run and never committed.

## Progress

- (running log / checklist — a resuming agent reads this to know exactly where things stand)

## Notes

- Design: `docs/designs/openai.md` component 3. Blocked on A-19 (the spec is what this peer
  implements). Uses A-20's client only in its own tests, if at all — the peer is a server.
- Precedent: the webhook vectors run "against a real loopback HTTP peer" in
  `crates/sipx-app/tests/`; this story gives the realtime spec the same treatment.
