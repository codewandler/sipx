---
id: X-148
title: Freeze the v1 endpoint-library content contract
pillar: Build
status: done
priority: 1
design: docs/specs/v1-endpoint-library.md
epic: v1-release
areas: [release, docs, public-api]
predicate:
announcement:
note: stable promotion needs a bounded content contract as well as the five existing evidence predicates
---

# Freeze the v1 endpoint-library content contract

## Goal

Turn “a complete SIP library” into a bounded, checkable stable-release contract: a production UAC
and UAS audio endpoint library with an explicit protocol, NAT, security and API surface, rather than
an unbounded promise to implement every SIP role and extension.

## Acceptance

- [x] A normative specification defines the v1 endpoint roles, transports, call flows, audio media,
      security and NAT traversal requirements, with primary RFC references and links to the
      subsystem specifications that own wire details.
- [x] The specification explicitly excludes proxy, registrar, PBX, IMS, video, browser SDK, AI,
      speech and DSP products without demoting capabilities already governed by crate-level
      Stability declarations.
- [x] A dependency and exit table makes `M-24`, `M-72`, `T-47`, `M-130`, `A-41` and `X-149`
      blocking work in an implementable order without copying their live status.
- [x] `docs/roadmap.md` requires the bounded content gate and preserves all five existing v1
      evidence predicates as a separate, conjunctive stable-promotion gate.
- [x] Focused documentation-link, provenance and story checks pass.
- [x] The complete local gate passes after the integrated wave; implementors leave this row open
      for the coordinator.

## Progress

- 2026-08-14: selected as the specification gate for the v1 implementation wave. The contract is
  deliberately endpoint-shaped: “complete” describes both SIP call roles and production network
  reachability, not proxy/server roles or all-RFC coverage.
- 2026-08-14: the integrated 55-step local gate passed after every blocking content row landed.

## Notes

- The five roadmap predicates measure whether the supported surface is truthful, exercised and
  safe to freeze. This story adds a content boundary; it does not renumber or replace them.
- The future child IDs were preallocated by the coordinator. This story does not create their story
  files or allocate alternatives.
