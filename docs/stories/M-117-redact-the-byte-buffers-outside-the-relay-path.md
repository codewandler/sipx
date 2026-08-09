---
id: M-117
title: Redact the byte buffers outside the relay path
pillar: Quality
status: backlog
priority: 6
design:
epic: media
areas: [sipx-app-protocol, sipx-testkit, sipx-ua]
predicate:
announcement:
note: after M-110 · four named carriers outside RELAY_PATH — two are call audio, one is PCM in a document, one is a nonce-signing key
---

# Redact the byte buffers outside the relay path

## Goal

Answer the four reachable public types outside `M-110`'s scope whose derived `Debug` renders bytes
nobody meant to print, and decide — with the answers in hand — whether the checker's scope should
name their crates.

## Why

`M-110` made the encoded half of `M-107` checkable by narrowing to a **scope** rather than to an
element type: on `sipx-media` and `sipx-rtp`, the two crates the call's own bytes pass through as
bytes, every reachable public type holding a byte buffer implements `Debug` or says beside itself
that its bytes are neither the call nor a participant. Eight carriers, all answered.

Fifty carriers are outside it, and the scope is honest about that: `check-audio-claims.py` counts
them on every run and its `RELAY_PATH` docstring says why. Nearly all fifty are a `Call-ID`, a URI,
a `Via`, a SIP body or a STUN attribute, where rendering the octets is the entire purpose of the
log — which is exactly why the scope is not the workspace.

Four are not. They were found by reading the fifty rather than by the checker, which is the point:
a count that nobody reads back is a suppression list with a better name.

| Carrier | What its own documentation says the bytes are |
|---|---|
| `sipx_app_protocol::Source::Inline` — `crates/sipx-app-protocol/src/document.rs:27` | "PCM carried in the document itself, base64 (RFC 4648 §4) on the wire" |
| `sipx_testkit::Record::appended_audio` — `crates/sipx-testkit/src/realtime_peer.rs:184` | "Every appended audio byte … the uplink as the far end heard it" |
| `sipx_testkit::ClientEvent` — `crates/sipx-testkit/src/realtime_peer.rs:153` | "The decoded payload: one 20 ms G.711 frame, per §4.1" |
| `sipx_ua::Authenticator` — `crates/sipx-ua/src/challenge.rs:190` | `secret: [u8; 32]`, the key every self-describing nonce is MACed with |

The first three are `M-107`'s defect in three more places: raw call audio in whatever record a
`tracing` field, an `expect` message or a test failure carries, in a record whose length is the
audio's. The fourth is not audio at all and is the more serious of the two shapes — a derived
`Debug` on `Authenticator` prints the nonce-signing key, and an operator who pastes one into a
ticket has published the secret that makes every nonce forgeable.

**Whether the scope widens is part of the story rather than assumed.** `RELAY_PATH` means "the call
in bytes" and `sipx-ua` is not that, so a third crate in it would make the constant mean something
its name does not say. The alternatives are a second scope with its own name, `sipx-audio`-style
element-type coverage for `[u8; N]` keys, or four hand fixes and no rule at all — and the one thing
that must not happen is a fix without a decision, because that is how `M-107` left `M-110`.

## Acceptance

- [ ] A failing-first test renders each of the four types carrying a distinctive value and asserts
      the value does not appear, in decimal, in hex or as printable characters — the last of which
      is the spelling `M-110`'s first helper missed.
- [ ] Each of the four either redacts by hand with a length, or carries beside itself the reason
      its rendering is safe. Each hand-written `Debug` is bounded independently of what it describes.
- [ ] `Authenticator` is decided on security terms and not audio's: whatever holds it must also hold
      the next type that keeps a key in a `[u8; N]`, or the story says why nothing does.
- [ ] The scope question is answered in `check-audio-claims.py` either way — a named scope covering
      them, or a stated reason no rule reaches them and a count that keeps saying so.
- [ ] `./scripts/gate.py` green.

## Progress

- Filed by `M-110` on 2026-08-09. The four were found by reading the fifty carriers its
  `outstanding_byte_buffers` counts; the count is printed on every run of
  `./scripts/check-audio-claims.py --check`.

- 2026-08-09: **the first of the four is fixed, out of band, because it is a security defect rather
  than a privacy one.** `sipx_ua::Authenticator` derived `Debug` over `secret: [u8; 32]` — the key
  every self-describing nonce is MACed with. A record carrying it does not leak a credential; it
  lets whoever reads it *mint nonces this authenticator accepts as its own*, which is the entire
  replay protection. `M-110`'s implementor found it while counting byte buffers, named it as the
  one it would do first, and correctly left it outside its own story.

  Proved red by restoring the derive: `the nonce signing key survived as "167" in: Authenticator {
  realm: "example.net", secret: [167, 167, 167, …] }`. The hand-written `Debug` renders realm,
  algorithm and lifetime, `secret` as `<redacted>`, and the replay window as a count — its keys were
  on the wire, but a window printed in full is an unbounded diagnostic.
  `crates/sipx-ua/tests/authenticator_diagnostics.rs` checks all four spellings the key could
  survive as: decimal, lower and upper hex, and the escaped printable run.

  **Three carriers remain and this story keeps them**: `sipx_app_protocol::Source::Inline`,
  `sipx_testkit::Record` and `ClientEvent`. So does the question the story was really filed for —
  whether a second checker scope should exist, or whether these are a reviewer's to hold.

