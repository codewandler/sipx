---
id: S-20
title: Sign and verify caller identity with STIR
pillar: Signalling
status: backlog
priority:
design:
epic: conformance
areas: [sipx-sip, sipx-ua]
note: M11 · RFC 8224 + 8225 · the largest remaining RFC gap; unattested traffic otherwise
---

# Sign and verify caller identity with STIR

## Goal
Sign the caller identity of an outgoing request so a peer network can check it, and verify an
incoming one so sipx can tell an asserted identity from a claimed one.

## Acceptance

**The token (RFC 8225)**
- [ ] A PASSporT is constructed with the claims the RFC defines and no others invented: `iat`
      (§5.1.1), `orig` and `dest` (§5.2.1), with the header parameters `typ` (§4.1), `alg` (§4.2) and
      `x5u` (§4.3).
- [ ] ES256 is implemented — "implementations MUST support ES256 as defined in JWA" — and the
      serialization follows §9's deterministic form. A signature over a differently ordered
      serialization verifies nowhere.
- [ ] An unsupported `ppt` fails validation rather than being ignored (§8.1: "Relying parties MUST
      fail to validate PASSporT objects containing an unsupported 'ppt'"). This is the opposite of the
      usual SIP rule about unknown parameters, which is why it is called out here.
- [ ] Attestation levels and `origid` are **not** implemented and not claimed: they are not in RFC
      8225, and asserting them would be asserting a different specification.

**The header field and the two services (RFC 8224)**
- [ ] The `Identity` header field is parsed to the §4 grammar — the signed digest plus `info`, and
      optionally `alg` and `ppt`. Today RFC 4474's header names parse as opaque values; 8224's form
      carries parameters, and treating it as opaque loses `ppt`, which decides whether the whole
      header may be ignored.
- [ ] `alg` absent means `ES256` (§4.1), not "unknown".
- [ ] The authentication service (§6.1) refuses to assert what it has no authority over — it "MUST
      NOT add an Identity header field if the authentication service does not have the authority to
      make the claim it asserts" — adds a `Date` header field where none exists, and rejects a request
      whose `Date` "contains a time different by more than one minute from the current time".
- [ ] The verification service (§6.2) runs its five steps in order, including the freshness check
      ("sixty seconds is RECOMMENDED") and the signature validation.
- [ ] Verification failures are the codes §6.2.2 names, each for its own cause and none of them a
      generic 400: **428** no usable `Identity`, **436** the credential could not be acquired, **437**
      the credential is unsupported, **438** no valid and supported PASSporT, and **403** for a `Date`
      older than local freshness policy.
- [ ] Credential acquisition from the `info` URI is behind a trait with a caller-supplied fetcher and
      a cache. sipx does not fetch a URL from a network peer's message on its own initiative: an
      unbounded fetch driven by an attacker's header is a request-forgery primitive, and the
      [vision](../vision.md)'s "malformed input is a value" applies to a URI as much as to bytes.
- [ ] The RFC registry entries for RFC 8224 and RFC 8225 move off "not started", and RFC 4474's note
      records that its headers remain parse-only because they still arrive.
- [ ] Failing-first test: `a_request_whose_identity_signature_does_not_verify_is_refused_with_438`.

## Progress
- Not started. RFC 8224 and 8225 are both "not started" in `compliance.md`; RFC 4474's header names
  parse and nothing acts on them.

## Notes
- Take this one alone. It is the only story on the list with a signature, a credential fetch and a
  canonicalisation in it, and each of the three has a way of being subtly wrong that a test against
  sipx's own output cannot catch. Verify against the RFCs' own example tokens, the way `S-14` verified
  digest against RFC 7616 §3.9.1 rather than against sipx's arithmetic.
- The framework here is deliberately generic: 8224 and 8225 are the mechanism, and profiles that sit
  on top of them (attestation, certificate policy for a particular network) are somebody's
  deployment decision, not a stack's.
- Signing needs a private key, and where it comes from is the caller's problem, not sipx's. Same
  posture as the TLS certificate policy in `docs/specs/sip-tls.md`.
