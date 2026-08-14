---
id: A-41
title: Freeze the v1 endpoint configuration surface
pillar: Application
status: done
priority: 4
design: docs/specs/v1-endpoint-library.md
epic: v1-release
areas: [sipx-call, sipx-ua, sipx-transport, api]
predicate:
announcement:
note: endpoint surface implemented, focused-green and integrated-gate green
---

# Freeze the v1 endpoint configuration surface

## Goal

Graduate the smallest high-level Rust surface required to configure, register, place and answer a
production endpoint, while keeping optional product and low-level backend surfaces explicitly
experimental.

## Acceptance

- [x] The v1 endpoint specification maps every Supported operation to its public entry point and
      every public type it exposes; no supported path depends on an item still classified
      Experimental.
- [x] Codec, media-security, ICE, local media address, identity and initial-direction policy use a
      symmetric non-exhaustive configuration shape for dial and answer roles, with typed validation
      before acquiring I/O resources.
- [x] Registration configuration exposes lease, Outbound lifetime and cancellation without making
      callers assemble protocol messages or retain internal flow state.
- [x] Defaults are explicit and unchanged: G.711, no ICE unless selected, and no silent downgrade
      from requested media security or an unavailable optional feature.
- [x] A registry-only downstream fixture compiles and exercises register, dial, answer, hold,
      resume, transfer, ICE and encrypted-media configuration using only Supported items.
- [x] Failing-first compile fixtures pin at least one former asymmetry and one Experimental
      dependency that the new surface removes.
- [x] Crate stability declarations, rustdoc, examples, feature-off builds and the complete gate are
      synchronized and green.

## Progress

- 2026-08-14: `CallConfig` is the opaque shared dial/answer boundary; the Supported stability map
  names every endpoint operation and policy type. Registration `Config` is opaque and exposes its
  requested lease through a builder while the existing `Flows::start` lifetime retains joined
  cancellation.
- 2026-08-14: failing-first registry-shaped compilation named the missing symmetric answer/config
  surface and the unwanted direct `sipx-media` dependency. The positive consumer now compiles with
  exact registry versions and exercises registration, identity, call setup/lifecycle, transfer,
  TURN and exact encrypted-media policy. Both negatives fail for their intended reason.
- 2026-08-14: focused tests prove answer direction, credential preflight before socket binding,
  SHA-256-authenticated TURN selection with bidirectional real audio, and direct Host selection
  after relay refusal. Call/UA/CLI clippy, rustdoc, feature-off builds, provenance, links and gate
  consistency are checked locally. The complete wave gate remains for the coordinator, so the last
  acceptance row and story status stay open.
- 2026-08-14: the coordinator's integrated 55-step repository gate passed, including rustdoc,
  examples, feature-off builds, package consumers and API classification checks.

## Notes

- This freezes Supported Rust APIs, not the experimental application wire contract, browser API,
  DSP graph, speech provider, QUIC mapping or backend-specific handshake module.
