---
id: X-98
title: Specify the neutral comparative load profile
pillar: Build
status: done
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

- [x] `docs/specs/comparative-load.md` normatively defines the byte-level signalling-only flow
      `INVITE -> 2xx -> ACK -> BYE -> 2xx`, identifiers, retransmissions, timeouts and failure classes.
- [x] The result schema records offered and completed rate, response/error classes, setup and teardown
      percentiles, CPU, RSS, descriptors, tasks, active high-water and post-drain state. Unsupported
      measurements are absent, never zero.
- [x] Every process announces readiness; every phase, log and queue is bounded; the supervisor owns a
      process group, installs EXIT/INT/TERM cleanup, terminates descendants and waits for them.
- [x] The protocol fixes a low-rate correctness preflight, driver-headroom proof, six-rate maximum
      ladder, five repetitions, deterministic seeds, ten-second warm-up, sixty-second measurement and
      at-most-forty-second drain, stopping after two consecutive failed rates.
- [x] Capacity requires at least 99.9% completion, zero invalid messages or crashes, declared loopback
      p99 setup at most 250 ms and complete drain. Uncertainty overlap is inconclusive, never ranked.
- [x] Subject-specific identity, pins, commands and evidence are data under `docs/comparison/`; the
      spec, runner and story remain subject-neutral.
- [x] Fixture tests reject unbounded phases, missing cleanup, incomplete metadata and a simulated
      orphan descendant; `./scripts/gate.py` is green.

## Progress

- The normative contract fixes byte flow, identifiers, transaction timers, phase protocol, capacity
  predicates, schema and supervision. `scripts/comparative-load.py` makes the closed manifest/result
  shape and process-group lifecycle executable; adversarial fixtures cover bounds, missing metadata,
  unsupported-resource honesty, duplicate readiness and a blocking descendant.
- Review hardening makes group disappearance independent of pipe EOF, forces cleanup after an
  orderly-stop callback fails, bounds unterminated readiness retention, rejects contradictory time
  and response totals, and covers oversized readiness plus an escaped pipe holder.
- Re-review hardening bounds even a blocking orderly-stop callback before group escalation, makes
  the manifest's `none`/`trying_100` provisional policy executable, and ties exact 200/non-2xx
  response totals to established, completed, rejected and admission-refused dialogs.
- Final supervision review moves the arbitrary orderly-stop action into its own cancellable,
  joinable process group and proves a forever-blocked action leaves no worker. Passed evidence now
  requires a clean, unforced leader exit, with every non-zero status tied to one crash count.
- Descendant cleanup review observes callback process-group disappearance independently of leader
  exit and proves that a returned callback cannot strand a descendant that ignores `TERM`.
- The reviewed M13 branches are integrated and the corrected full gate passed all 36 steps. The
  actual comparative measurement remains M14.
