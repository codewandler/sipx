---
id: M-15
title: Key SRTP with DTLS
pillar: Media
status: done
priority:
design:
epic: conformance
areas: [sipx-media]
note: M6 · RFC 5764 · M-14 unblocked it
---

# Key SRTP with DTLS

## Goal
Keying that does not require trusting the signalling path, and the only keying a browser will
accept.

## Acceptance
- [x] DTLS-SRTP handshake over the media path, with the fingerprint carried in SDP
      (RFC 5763 / 8122).
- [x] The fingerprint in the SDP is checked against the certificate presented, or the media is
      dropped — an unchecked fingerprint makes the whole exchange decorative.
- [x] Works with the WebSocket transports, since that is the combination browsers use.
- [x] Failing-first test: `a_mismatched_fingerprint_stops_the_media`.

## Progress
- Done. SDES (`M-14`) keys over the signalling path, which means every proxy on it has held the
  key; this keys on the media path, and the SDP carries only a hash of the certificate that will
  appear there.
- Split so the C dependency is not load-bearing. **Everything RFC 5764 decides is compiled
  always** — `a=fingerprint`/`a=setup` negotiation, §5.1.2's demultiplexing, §4.2's key
  derivation, §6.2's check — behind a `Handshake` trait. Only the handshake is behind the
  off-by-default `dtls` feature, which is where OpenSSL lives. The default build stays pure Rust.
- **A pure-Rust DTLS was considered and rejected**, and the choice is the user's rather than
  mine: there is none with comparable scrutiny, and a hand-rolled handshake for a
  security-critical protocol is the liability this project declines elsewhere — the same
  reasoning that has SRTP's AES come from RustCrypto. Same pattern as Opus, and off by default
  for the same reason.
- **The mutation that survived is the finding worth recording.** The first version of the
  key-split test asserted "what the client protects, the server unprotects" — and that passes
  under *any* consistent permutation of §4.2's exported block, because both ends run the same
  split. Reading key-and-salt per side instead of keys-then-salts survived the whole suite,
  including the real two-socket handshake, because sipx was talking to sipx. The test now asserts
  the RFC's **literal byte offsets**. A bug of this shape is invisible until a foreign
  implementation answers, which is exactly what `X-17` exists to find.
- **The fingerprint check happens where OpenSSL cannot see it.** The peer's certificate is
  requested and deliberately not validated by the TLS stack: §5 expects a self-signed
  certificate, so there is no chain to validate, and what authenticates it is a value that
  arrived in the *signalling*. `establish` performs §6.2's check before returning any keys, and a
  mismatch is an error rather than a pair of contexts a caller might use anyway.
- **A peer that sent no fingerprint is refused before the handshake runs**, not after. An
  unverified DTLS handshake authenticates nobody, and discovering that afterwards means having
  established a channel to an unknown party.
- MD5 and MD2 are refused at the parser rather than carried and checked later — §5 forbids acting
  on them, and a parser that returns one hands a caller a value it is not allowed to use. A
  digest whose length does not match its hash is refused too: a truncated one that compared equal
  against a prefix would verify almost nothing.
- The offerer sends `a=setup:actpass` and the answerer takes `active` (RFC 5763 §5), so the
  *answerer's* ClientHello opens the NAT it sits behind. The role is answered, never copied —
  two ends that both say `active` both send a ClientHello and neither answers one.
- A session-level `a=fingerprint` is found as well as a media-level one. A browser puts it there
  and on no `m=` line at all, so reading only the media level refuses a perfectly good offer.
- Mutation-tested six ways: skipping the fingerprint check, splitting the exported block
  key-and-salt per side, ignoring the role, changing the exporter label, answering a DTLS offer
  that carried no fingerprint, and copying the `setup` role instead of answering it.

## Notes
- The `dtls` feature needs OpenSSL headers to build (`libssl-dev` on Debian). CI installs them
  the way it already installs `libopus-dev`.
- Not covered: the `a=connection` attribute of RFC 4145, and TCP media transport. sipx carries
  media over UDP, and §4145's `setup` is borrowed by RFC 5763 for who opens the connection —
  which is the only part of it in scope.
