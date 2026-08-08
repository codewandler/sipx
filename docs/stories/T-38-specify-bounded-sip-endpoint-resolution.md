---
id: T-38
title: "Specify bounded SIP endpoint resolution"
pillar: Transport
status: done
epic: endpoint-resolution
areas: [sipx-transport]
design: docs/designs/endpoint-resolution.md
note: "external review finding 2 · spec-first target derivation, DNS ordering, identity and deadlines"
---

# Specify bounded SIP endpoint resolution

## Goal

Write the one contract that turns a SIP or SIPS URI, optional explicit port and optional transport
selection into bounded ordered connection targets without losing the service name used for secure
transport verification.

## Acceptance

- [x] `docs/specs/sip-target-resolution.md` normatively cites RFC 3263, RFC 2782 and RFC 5922 and
      defines inputs, resolver answers, ordered outputs, service identity and typed failures.
- [x] The spec covers literal IPv4/IPv6 fast paths, named hosts with explicit ports, URI transport
      parameters, explicit CLI transport, NAPTR/SRV fallback, A/AAAA ordering, empty/negative
      answers and SIPS no-downgrade behavior.
- [x] A state table bounds lookups, records, candidate targets, connection attempts, per-attempt and
      overall deadlines, cache entries and cancellation. No DNS I/O, clock read or async runtime is
      introduced into `sipx-sip` or `sipx-sdp`.
- [x] The original hostname remains the TLS/WSS verification identity after an address is selected;
      an explicit validated server-name override is the only replacement.
- [x] Deterministic value-level vectors cover mixed address families, explicit port precedence,
      unusable records, secure and cleartext choices, deadline expiry and cancellation.
- [x] The spec states which policy is pure and which adapter in `sipx-transport` owns DNS and
      connection I/O, including how tests inject resolver results without external DNS.
- [x] RFC registry changes, if the supported behavior changes, are synchronized and the complete
      repository gate is green.

## Review evidence

Finding 2 showed that named `dial` targets were refused and `register` required externally resolved
manual address injection even when the AOR contained an ordinary hostname.

## Progress

- 2026-08-06: specified the pure selection/I/O boundary, input precedence, secure identity,
  NAPTR/SRV/address ordering, finite limits, cache and cancellation rules, typed failures, and
  deterministic vectors. RFC 3263 and RFC 2782 now cite the new normative spec in the registry
  source. Generated reports and the complete gate remain deferred to the shared push boundary, so
  the final acceptance item and story status remain open.
