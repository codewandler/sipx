---
id: X-75
title: Be quiet by default and easy to test against
pillar: Build
status: ready
priority: 5
design: docs/designs/test-surfaces.md
epic: test-surfaces
areas: [sipx-call, sipx-ua, sipx-testkit, m13, parity-wave-1]
predicate:
announcement:
note: recurring complaint against the surveyed stack · a library that spams logs and cannot be tested against
---

# Be quiet by default and easy to test against

## Goal

Make sipx silent unless the host asks for output, and give applications a supported way to drive a
call in their own tests.

## Acceptance

- [ ] **Audit what the library emits.** Progress records where sipx logs today and at what levels.
      A library that writes to a host's log without being asked is the reported irritant; the audit
      says whether sipx has it.
- [ ] All library output goes through `tracing` with no global subscriber installed by any library
      crate — only the binaries install one. A test asserts that using the library without a
      subscriber produces no output.
- [ ] Log levels follow a stated policy documented once: what is `error`, `warn`, `info`, `debug`
      and `trace`, and specifically that per-message signalling detail is not `info`.
- [ ] A supported call-testing harness is public: an application can place and answer a call in its
      own test suite without sockets, using the in-process link `sipx-testkit` already has
      internally. It is documented in a guide with a runnable example.
- [ ] The harness is exercised by an example under `crates/*/examples/` that CI compiles, so it
      cannot rot, and the guide inlines it via `sync-website.py`.
- [ ] Whether `sipx-testkit` becomes a published crate is decided here and recorded — it is
      currently unpublished, and a harness nobody can depend on is not a harness.
- [ ] `./scripts/gate.py` green.

## Progress
- (not started)

## Notes
- Two separate recurring themes in the demand survey collapse into this one story: users could not
  silence the library in their tests, and users repeatedly asked how to test call flows and were
  never given a supported answer.
- sipx is better placed than the surveyed stack here — the loopback link and virtual-time harness
  already exist. This is largely a publishing-and-documenting story, which is why it is `X` rather
  than a feature pillar.
- Pairs with `X-69`: a harness with no guide is as unreachable as a feature with no caller.
