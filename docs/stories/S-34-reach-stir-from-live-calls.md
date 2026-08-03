---
id: S-34
title: Reach STIR signing and verification from a live call
pillar: Signalling
status: backlog
priority: 12
design: docs/specs/sip-identity.md
epic: conformance
areas: [sipx-ua, sipx-call, interop]
note: M11 evidence gap left explicit by S-20 — the sans-I/O services exist, but no call selects them and no independent verifier has accepted their output
---

# Reach STIR signing and verification from a live call

## Goal

Make S-20's caller-identity services reachable from the call framework and supply the independent
end-to-end evidence M11 requires, without moving credential retrieval, trust, or time into the core.

## Acceptance

- [ ] A caller-owned call policy can select the authentication service for an outbound INVITE and
      the verification service for an inbound one. Authority, current time, credentials, trust and
      retrieval remain explicit inputs; no socket, clock read or built-in URL fetch enters
      `sipx-sip` or `sipx-ua`.
- [ ] An outbound call carries an `Identity` field that an independent verifier accepts. The test
      must not verify solely with sipx's own implementation or share its PASSporT serializer.
- [ ] An inbound call whose signature is invalid is refused with 438 before the application can
      answer it. Missing identity follows the caller's explicit policy and maps to 428 only when
      identity is required.
- [ ] The unselected default remains wire-compatible: ordinary calls add no `Identity` field and
      perform no credential acquisition.
- [ ] RFC 8224 and RFC 8225 claim UAC/UAS roles only in the same change that lands the live-call and
      independent-verifier evidence.
- [ ] Failing-first tests name both unreachable paths: signing selected on an outbound call and
      verification selected on an inbound call.

## Progress

- Filed from S-20's independent review. The pure services, grammar, cryptography, cache and exact
  status mapping are implemented and tested; the registry deliberately lists no UAC/UAS role
  because no call-layer entry point selects either service yet.

## Notes

- This story owns M11's identity demonstration. S-20 remains correctly done at the library-service
  boundary; this is the separate composition and independent-evidence boundary.
- The independent verifier requirement is the same lesson as T-13: two paths through one serializer
  can agree on the same mistake.
