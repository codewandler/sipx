---
id: X-75
title: Be quiet by default and easy to test against
pillar: Build
status: in-progress
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

- [x] **Audit what the library emits.** Progress records where sipx logs today and at what levels.
      A library that writes to a host's log without being asked is the reported irritant; the audit
      says whether sipx has it.
- [x] All library output goes through `tracing` with no global subscriber installed by any library
      crate — only the binaries install one. A test asserts that using the library without a
      subscriber produces no output.
- [x] Log levels follow a stated policy documented once: what is `error`, `warn`, `info`, `debug`
      and `trace`, and specifically that per-message signalling detail is not `info`.
- [x] A supported call-testing harness is public: an application can place and answer a call in its
      own test suite without sockets, using the in-process link `sipx-testkit` already has
      internally. It is documented in a guide with a runnable example.
- [x] The harness is exercised by an example under `crates/*/examples/` that CI compiles, so it
      cannot rot, and the guide inlines it via `sync-website.py`.
- [x] Whether `sipx-testkit` becomes a published crate is decided here and recorded — it is
      currently unpublished, and a harness nobody can depend on is not a harness.
- [ ] `./scripts/gate.py` green.

## Progress
- 2026-08-05: implementation started. The existing loopback link and transaction timer queue are
  the call harness substrate; the public boundary, output audit, logging policy and executable
  documentation are being added without a socket or wall-clock wait.
- 2026-08-05: the library audit found `sipx-call` at 1 error / 11 warn / 0 info / 8 debug,
  `sipx-media` at 1 / 7 / 1 / 17, `sipx-transport` at 2 / 18 / 0 / 42 and `sipx-ua` at
  0 / 4 / 1 / 0, with no trace sites. There were no direct output macros or library subscriber
  installers. `library_output.rs` now ratchets the source rule, proves the harness installs no
  global subscriber, and compares isolated process output against a no-library control.
- 2026-08-05: `sipx-testkit` is now public by package metadata, has a crate README and stability
  labels, and packages successfully against the registry versions of its dependencies. The
  supported boundary is `CallHarness`, `Link` and `Virtual`; corpus and certificate utilities stay
  Experimental. The public guide is byte-synchronised from the compiled `test_a_call` example.
- 2026-08-05: focused verification passed: all 43 `sipx-testkit` unit/integration/example targets,
  clippy with warnings denied, the runnable example, package listing and registry-style package
  verification, release-helper tests, public-doc synchronisation/tests, capability-front-door
  checks, maturity, provenance and formatting. The full gate is deliberately left for integration,
  so the story remains `in-progress` and its final acceptance item remains open.
- 2026-08-05: follow-up review replaced the raw-request public façade with the real `sipx-call`
  application path. `CallHarness` now drives `DialOptions`, `dial`, `answer`, two `Call` values,
  their events and the final ACK over an in-process signalling handle pair. Pending exchanges own
  their invitation and response stream, so stale history cannot satisfy a later call.
- 2026-08-05: `TransactionHarness::advance` now visits deliveries and timer deadlines in order;
  its regression proves one 600 ms jump matches 500 ms plus 100 ms through a dropped INVITE and
  retransmission. `Virtual` stores nanoseconds, and the output ratchet now covers `print`/`eprint`,
  stdout/stderr `write` variants and subscriber initializer variants while its subprocess executes
  a complete call path.
- 2026-08-05: review found that the in-process route table could retain completed exchanges and a
  dropped pending call detached its dial task. The driver now caps routes, refuses excess work,
  removes final and closed-consumer routes, and aborts a pending dial on drop; barrier-based tests
  cover cancellation and cleanup without wall-clock waits.

## Notes
- Two separate recurring themes in the demand survey collapse into this one story: users could not
  silence the library in their tests, and users repeatedly asked how to test call flows and were
  never given a supported answer.
- sipx is better placed than the surveyed stack here — the loopback link and virtual-time harness
  already exist. This is largely a publishing-and-documenting story, which is why it is `X` rather
  than a feature pillar.
- Pairs with `X-69`: a harness with no guide is as unreachable as a feature with no caller.
