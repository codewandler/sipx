---
id: X-102
title: Publish the peer endpoint load comparison
pillar: Build
status: in-progress
priority: 3
design: docs/designs/comparative-load.md
epic: comparative-load
areas: [load, comparison, website, m14]
predicate:
announcement:
note: compare endpoint responder capacity under the neutral profile; leave proxy workloads to sipx.clstr
---

# Publish the peer endpoint load comparison

## Goal

Extend the comparative load evidence from one endpoint result to a reproducible side-by-side
endpoint responder comparison, and explain what the harness can and cannot measure without
conflating endpoint signalling with proxy or registrar workloads.

## Acceptance

- [ ] The public comparison explains the harness capabilities in plain language: fixed open-loop
      offered load, correctness qualification, driver headroom, six rates, five repetitions,
      latency, resources, cleanup and retained raw evidence.
- [ ] One pinned peer endpoint is adapted to the same deterministic UDP dialog profile and passes
      the correctness preflight before any capacity result is admitted.
- [ ] Both endpoint responder results were captured on the same host with the same driver artifact,
      ceiling, seed, provisional-response policy and pass/fail predicates.
- [ ] The generated report publishes each measured rate, the supported capacity point and the
      responder-only limitation without ranking the implementations.
- [ ] The report explicitly excludes proxy, registrar, routing and cluster behavior and directs
      those future benchmarks to sipx.clstr.
- [ ] Raw manifests, environment inventories, artifact hashes, commands, cleanup evidence and
      per-repetition records regenerate the internal and public reports.
- [ ] Failing-first checker tests cover multi-endpoint rendering and reject incompatible runs;
      comparison checks, website sync, provenance and the full gate are green.

## Progress

- 2026-08-05: the pinned peer adapter passed the 20-dialog preflight and 100-dialog qualification,
  then completed all six rates at 5/5 repetitions through the 1,024 calls/s ceiling. Run
  `f13e4cb0dbabd467ffada90872654a85` retains its manifest, environment, commands, build and
  dependency identities, headroom result, cleanup evidence and all thirty repetition records.
- 2026-08-05: rendering and failing-first cross-run checks now cover the plain-language harness
  contract, multi-endpoint rows, same-host inventory and identical ceiling, seed, policy, resource
  limits, phase durations and ladder. The generated report keeps the endpoint-only scope explicit
  and sends proxy, registrar, routing and cluster workloads to sipx.clstr.
- 2026-08-05: profiling the first sipx result filed `X-103` and expanded the normative responder
  state table for a valid BYE that reaches the application before its earlier ACK. That deliberate
  contract change invalidates both recorded runs by hash, and the checker refuses publication as
  designed. Fresh side-by-side runs wait for `X-103`'s implementation to have an immutable
  revision; recorded hashes will not be edited forward.
- 2026-08-05: the optimized endpoint was retained under the current contract as run
  `056391cbff0d7d2345a61a11c93e15b3`. The incompatible prior runs were removed rather than edited
  forward. A compatible peer refresh is deliberately deferred; the generated comparison therefore
  says `not measured` for that direction instead of presenting old or partial evidence.
