---
id: M-53
title: Ship a runnable RTP echo example
pillar: Media
status: done
priority: 11
design: docs/designs/media.md
epic: test-surfaces
areas: [sipx-media, examples, docs, m13, parity-wave-1]
predicate:
announcement:
note: discovered by X-97 · exercise the public RTP receive/send seam without inventing a second media stack
---

# Ship a runnable RTP echo example

## Goal

Give downstream users a compiled, bounded example that receives RTP audio through the public media
surface and sends the same decoded samples back, making the bidirectional seam visible from a shell.

## Acceptance

- [x] A runnable example under `crates/*/examples/` uses only public sipx APIs, declares a finite run
      bound and shuts down every owned task and socket before exit.
- [x] The example accepts an explicit bind/peer configuration, refuses unbounded or malformed input,
      and never spins or sleeps to stand in for an event.
- [x] A deterministic test feeds a finite RTP stream, proves recognizable audio returns with correct
      sequence/timestamp progression, then observes zero residual media work.
- [x] The public testing guide explains the example's diagnostic scope and does not present echo as
      acoustic-echo cancellation, a production media server or a load test.
- [x] The website inlines the compiled source through `sync-website.py`, and compilation/tests pass
      through `./scripts/gate.py`.

## Progress

- 2026-08-05: implementation started from `docs/specs/rtp-echo-fixture.md`. The fixture is deliberately
  one bounded UDP owner over the public RTP packet and G.711 codec APIs; it does not start a media
  session or detach a worker whose cleanup a downstream test cannot observe.
- 2026-08-05: the compiled example requires explicit bind, peer, packet and whole-run bounds. Its
  deterministic integration test sends three recognizable frames across the 16-bit input sequence
  wrap, proves replies `0/0`, `1/160`, `2/320`, then rebinds the socket and observes the runtime task
  count unchanged. A poll-then-cancel regression proves cancellation uses the same cleanup path.
- 2026-08-05: the public guide inlines the compiled source and fixes the diagnostic scope. Focused
  all-target/all-feature tests, strict clippy, a live three-packet shell run, public-doc sync,
  provenance, fixed-sleep and maturity passed. Adversarial wire tests now prove typed terminal errors,
  socket reuse and zero owned work for foreign-source, oversized and non-PCMU input. The successful
  wire vector also pins marker-clear, padding-free, extension-free and CSRC-free output plus every
  byte of the first three fixed RTP headers. The bounded
  local package-set verifier derives and stages X-75's complete transitive public dependency closure
  for `sipx-testkit` in dependency order, then compiles the archived RTP echo example in a clean
  consumer whose lockfile proves every member, including `sipx-sip` and `sipx-transport`, came from
  staged bytes. The locked pre-publication dry-run now invokes this proof
  unconditionally, and an end-to-end release test executes the real archive/consumer path under
  separate finite command bounds; neither makes a claim about the older registry beta.4 transport.
  The corrected full integration gate passed all 36 steps, including archive-consumer proof, the
  public-site build and the complete workspace test suite.
