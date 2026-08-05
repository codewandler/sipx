---
id: M-53
title: Ship a runnable RTP echo example
pillar: Media
status: backlog
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

- [ ] A runnable example under `crates/*/examples/` uses only public sipx APIs, declares a finite run
      bound and shuts down every owned task and socket before exit.
- [ ] The example accepts an explicit bind/peer configuration, refuses unbounded or malformed input,
      and never spins or sleeps to stand in for an event.
- [ ] A deterministic test feeds a finite RTP stream, proves recognizable audio returns with correct
      sequence/timestamp progression, then observes zero residual media work.
- [ ] The public testing guide explains the example's diagnostic scope and does not present echo as
      acoustic-echo cancellation, a production media server or a load test.
- [ ] The website inlines the compiled source through `sync-website.py`, and compilation/tests pass
      through `./scripts/gate.py`.

## Progress

- Discovered by the checked capability inventory. Not started.
