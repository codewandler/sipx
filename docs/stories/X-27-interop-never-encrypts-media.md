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
- [~] `tests/interop/` places at least one call with SRTP against a real peer and asserts that
      audio arrives, for both keyings: SDES (`RTP/SAVP` with `a=crypto`) and DTLS-SRTP
      (`UDP/TLS/RTP/SAVP` with `a=fingerprint`). Today `grep -i "srtp\|savp\|dtls\|sdes"` over
      `tests/interop/` matches nothing at all.
      **SDES done** — `crates/sipx-cli/tests/interop_srtp.rs`. **DTLS-SRTP blocked**: nothing in
      `sipx-call` reaches `Capabilities::with_dtls_srtp` or `sipx_media::dtls::establish`, so
      `dial` offers SDES and only SDES and there is no sipx side to the call. See Progress.
- [x] The assertion is that the *peer* accepted our packets, not that a round trip through our own
      stack succeeded. A test that only proves sipx agrees with sipx is the thing that failed here.
- [x] It runs in CI the way the existing peer matrix does, per peer, and the harness says which
      peers support which keying rather than silently skipping.
- [x] Failing-first test: reverting `M-25`'s one-line `SESSION_AUTH_LEN` fix makes the new interop
      case fail. That revert is the exact defect this story exists to have caught, so it is the
      honest measure of whether the new coverage would have caught it.

## Progress
- **SDES half done and proved.** `crates/sipx-cli/tests/interop_srtp.rs` places a TLS-signalled
  call with `RTP/SAVP` media against asterisk and makes three assertions, all of them the far
  end's: the negotiation actually chose SAVP (so the case cannot pass by degrading to the
  cleartext call already covered), the peer logged no authentication failure, and the audio it
  echoed is the audio sipx sent. A new `media-security` role in `tests/interop/run.sh` runs it,
  and CI picks it up with no workflow change because the matrix already calls `run.sh --peer`.
- **The falsification holds.** With `SESSION_AUTH_LEN = 20` the case passes; reverted to `94` it
  fails on the peer's own words — `SRTP unprotect failed on SSRC … because of authentication
  failure 10`, with asterisk's media counters showing `Receive Count 0`. That is the fourth
  Acceptance item met against a live peer, not reasoned about.
- **DTLS-SRTP is blocked on library work, not on the harness.** `sipx-media::dtls` implements
  RFC 5764's parts and `Capabilities::with_dtls_srtp` exists, but **neither has a caller outside
  its own crate's tests**: `dial` (`crates/sipx-call/src/call.rs:1707`) hardcodes
  `.with_srtp(transport.is_secure())`, and `DialOptions` has no keying selector. Writing the
  interop test is the last step, not the first — the sipx side of a DTLS call has to exist. This
  needs a sibling story against `sipx-call`: add the keying choice to `DialOptions`, offer
  `UDP/TLS/RTP/SAVP` with `a=fingerprint`, and run the handshake on the media path. The harness
  is already shaped for it — declare the test name in `KEYING_TESTS[dtls]`, drop the
  `KEYING_UNIMPLEMENTED[dtls]` entry, and it runs.
- The keying axis reports all three outcomes by name (peer cannot / sipx cannot / ran), so the
  DTLS gap is printed on every run rather than being an absence. asterisk declares
  `PEER_KEYINGS=(sdes dtls)` honestly — it does support both; the gap is ours.
- Not addressed: the Notes' wider sweep for crypto modules lacking off-stack vectors. That is a
  separate story and is listed as adjacent in the handoff.

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
