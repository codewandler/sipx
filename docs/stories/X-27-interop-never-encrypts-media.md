---
id: X-27
title: Place an interop call with encrypted media
pillar: Build
status: in-progress
priority: 1
design: docs/designs/media.md
epic: conformance
areas: [tests, sipx-rtp, sipx-media]
note: found by M-25 — the n_a defect shipped in v0.3.0 through v0.8.0 because nothing ever exchanged SRTP with a non-sipx peer
---

# Place an interop call with encrypted media

## Goal
Make the interop harness exercise the media security path against a real third-party stack, so a
defect that both ends of a sipx-to-sipx call agree on cannot pass for correct.

## Acceptance
- [ ] `tests/interop/` places at least one call with SRTP against a real peer and asserts that
      audio arrives, for both keyings: SDES (`RTP/SAVP` with `a=crypto`) and DTLS-SRTP
      (`UDP/TLS/RTP/SAVP` with `a=fingerprint`). Today `grep -i "srtp\|savp\|dtls\|sdes"` over
      `tests/interop/` matches nothing at all.
- [ ] The assertion is that the *peer* accepted our packets, not that a round trip through our own
      stack succeeded. A test that only proves sipx agrees with sipx is the thing that failed here.
- [ ] It runs in CI the way the existing peer matrix does, per peer, and the harness says which
      peers support which keying rather than silently skipping.
- [ ] Failing-first test: reverting `M-25`'s one-line `SESSION_AUTH_LEN` fix makes the new interop
      case fail. That revert is the exact defect this story exists to have caught, so it is the
      honest measure of whether the new coverage would have caught it.

## Progress
- Not started.

## Notes
- **Why this is priority 1.** `M-25` found that `sipx-rtp` keyed HMAC-SHA1 with 94 octets where
  RFC 3711 §5.2 and §8.2 fix `n_a` at 160 bits. HMAC's block size is 64, so RFC 2104 reduced the
  key to `SHA1` of itself — a *different* key from the one every conformant peer computes, not a
  weaker one. Every tag sipx produced failed at a correct peer and vice versa, on the first
  packet, in both directions, for six released versions.
- **Nothing caught it because nothing could.** All 17 SRTP tests were round-trips or
  tamper-negatives, which pass identically whether the key is 20 octets or 94 — both ends wrong
  the same way. The suite was blind to it rather than agreeing with it. `M-25` added the file's
  first off-stack tag vector; this story adds the other half, which is a peer that did not come
  from this repository.
- The gap is structural, not local: **nothing in the gate can tell a self-consistent stack from a
  conformant one.** Every crypto module here should own at least one vector computed off-stack,
  and it is not known which others do. Worth a sweep as part of this story or as its sibling.
- Sibling in kind to `X-16` (the RFC 5118 corpus) and `X-17` (a second interop peer): all three
  exist because agreeing with oneself is not evidence.
