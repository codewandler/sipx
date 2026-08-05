---
id: X-98
title: Specify the neutral comparative load profile
pillar: Build
status: backlog
priority: 12
design: docs/designs/comparative-load.md
epic: comparative-load
areas: [load, testkit, docs, m13, parity-wave-1]
predicate:
announcement:
note: late M13 dependency · signalling-only common workload, safe supervisor and stable result schema
---

# Specify the neutral comparative load profile

## Goal

Define the exact bounded workload, supervisor and evidence schema used for cross-process endpoint
comparison before implementing or running it.

## Acceptance

- [ ] `docs/specs/comparative-load.md` normatively defines the byte-level signalling-only flow
      `INVITE -> 2xx -> ACK -> BYE -> 2xx`, identifiers, retransmissions, timeouts and failure classes.
- [ ] The result schema records offered and completed rate, response/error classes, setup and teardown
      percentiles, CPU, RSS, descriptors, tasks, active high-water and post-drain state. Unsupported
      measurements are absent, never zero.
- [ ] Every process announces readiness; every phase, log and queue is bounded; the supervisor owns a
      process group, installs EXIT/INT/TERM cleanup, terminates descendants and waits for them.
- [ ] The protocol fixes a low-rate correctness preflight, driver-headroom proof, six-rate maximum
      ladder, five repetitions, deterministic seeds, ten-second warm-up, sixty-second measurement and
      at-most-forty-second drain, stopping after two consecutive failed rates.
- [ ] Capacity requires at least 99.9% completion, zero invalid messages or crashes, declared loopback
      p99 setup at most 250 ms and complete drain. Uncertainty overlap is inconclusive, never ranked.
- [ ] Subject-specific identity, pins, commands and evidence are data under `docs/comparison/`; the
      spec, runner and story remain subject-neutral.
- [ ] Fixture tests reject unbounded phases, missing cleanup, incomplete metadata and a simulated
      orphan descendant; `./scripts/gate.py` is green.

## Progress

- Backlog. Begins after the endpoint feature branches settle; P-15 consumes the contract before the
  M14 comparison run.
