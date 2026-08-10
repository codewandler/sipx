---
id: M-117
title: Redact the byte buffers outside the relay path
pillar: Quality
status: done
priority: 53
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

- [x] A failing-first test renders each of the four types carrying a distinctive value and asserts
      the value does not appear, in decimal, in hex or as printable characters — the last of which
      is the spelling `M-110`'s first helper missed.
- [x] Each of the four either redacts by hand with a length, or carries beside itself the reason
      its rendering is safe. Each hand-written `Debug` is bounded independently of what it describes.
- [x] `Authenticator` is decided on security terms and not audio's: whatever holds it must also hold
      the next type that keeps a key in a `[u8; N]`, or the story says why nothing does.
- [x] The scope question is answered in `check-audio-claims.py` either way — a named scope covering
      them, or a stated reason no rule reaches them and a count that keeps saying so.
- [x] `./scripts/gate.py` green.

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

- 2026-08-10: **the remaining three are redacted, and both scope questions are answered with rules
  rather than with notes.** Branch `impl/M-117`, base `c43ebde`.

  **The decision, first, because the story asked for one before a fix.** The four carriers turned
  out to be two questions and they are answered separately.

  *The three audio ones get a second named scope.* `AUDIO_AT_REST = ("sipx-app-protocol",
  "sipx-testkit")` in `check-audio-claims.py`, held to `byte_buffer_problems` — the same rule,
  unchanged, over a second crate list. A second constant rather than three more crates in
  `RELAY_PATH`, because that name means "the call in flight" and these are the opposite half of the
  same sentence: `Encoded` holds one payload for as long as it takes to send it, `Source::Inline`
  holds a prompt inside a document a host may have logged whole, and `Record` holds every uplink
  byte of a call for the life of the test. Same defect, opposite lifetime. `M-110`'s test for
  whether a scope is possible — can every carrier in these crates be decided by a person once —
  holds here and stays false for `sipx-sip`'s twenty-eight headers.

  *`Authenticator` gets a rule that is not a scope at all*, which is the acceptance row's "must also
  hold the next type that keeps a key in a `[u8; N]`". `key_problems` runs over **every** published
  crate on a new selector, `_KEY_ARRAY` = `[u8; N]`: a `Vec<u8>` is as long as whatever arrived and
  needs a crate list to decide, but a fixed-size array is a length the *type* chose, and in this
  workspace that has only ever meant a key, a tag or a fingerprint. Its escape is a sixth phrase,
  `/// Not a secret:`, for `NOT_THE_CALL_REASON`'s own argument one level up — "not the call" is
  *true* of a signing key, so a rule escapable by the byte-buffer phrase would have been answered
  truthfully by the one type it exists for.

  The population of that rule is **one type in the whole workspace** today, and that is stated
  rather than hidden: `_PLAUSIBLE_KEY_CARRIERS` is the only floor in the file set *at* its
  population instead of below it, and its comment says what that does and does not catch.

  **Failing-first, in this worktree at `c43ebde` with its own target directory.**

  `cargo test -q -p sipx-app-protocol --test document_diagnostics --all-features` — 4 of 5 red:

  ```
  an inline audio octet survived as `167` in: Inline([167, 167, 167, …])
  a printable inline audio octet survived as `126` in: Inline([126, 126, …])
  a record whose length is the audio's is an unbounded diagnostic: 500008 octets
  ```

  `cargo test -q -p sipx-testkit --test realtime_peer_diagnostics --all-features` — 6 of 7 red:

  ```
  an uplink audio octet survived as `167` in: Record { upgrades: [], client_events:
  [SessionUpdate(Object {…}), Append { audio: [167, 167, …
  a record whose length is the call's is an unbounded diagnostic: 16252123 octets
  an event record grew with the event: 50078 octets
  ```

  The checker's half was proved the same way: adding `AUDIO_AT_REST` before touching any Rust made
  `check-audio-claims.py --check` exit 1 on `Delivery`, `rfc4475::Case` and `rfc5118::Case` as well
  as the three the story named — the scope's remainder, found by the rule rather than by reading.

  **Two of the six went a way the story did not predict.** `Delivery` and the two corpus `Case`
  types are not audio, so the obvious answer was a `/// Not the call:` rationale. Both were written
  as hand-written `Debug`s instead, for reasons recorded beside them: a SIP datagram carries `From`,
  `To` and `Contact`, so claiming it is "neither the call **nor a participant**" is false in the
  second clause and this is a published crate a downstream stack points at its own users; and a
  corpus `Case` is RFC-published and fictional, so the redaction half does not apply but the
  *bounded* half does — a torture message is kilobytes and every diagnostic naming a case carried
  all of it. `Case::lossy()` is where a test gets the message, which is what the corpus tests
  already used.

  **What the redaction does not do**, said out loud at `Record`: the fields stay public, so a test
  that formats `record.appended_audio` itself still gets the bytes. ORB-3 asserts on those exact
  octets, so redacting the field would make the assertion vacuous — the same trade `Upgrade`
  documents for its verbatim `Authorization` header. What changed is that leaking the audio is now
  something a caller reaches for rather than what any record of the type does by default. Rendering
  `Record::upgrades` as a count is what keeps the bearer out of a failure message, one type up.

  **Counts after:** 8 carriers on the relay path, 6 where the call rests, 44 outside both scopes
  (was 50, and forty-four is not smaller in any way that means the remainder was checked), 1 key
  carrier.

  **CHANGELOG sentence owed** — the integrator writes it, this branch does not touch the file:

  > Byte buffers outside the relay path no longer print themselves: `sipx_app_protocol::Source`,
  > `sipx_testkit::Record`, `ClientEvent`, `Delivery` and both RFC corpus `Case` types render a
  > length rather than their octets, and `check-audio-claims.py` gains a second scope for the crates
  > where a call comes to rest plus a workspace-wide rule holding every fixed-size `[u8; N]` key.

  **Not run here:** `./scripts/gate.py`, by dispatch — the coordinator runs one gate for the wave,
  so the last acceptance row stays unticked. Run in this worktree: both new test binaries, the full
  `sipx-app-protocol`, `sipx-testkit`, `sipx-app` and RFC-corpus suites, `cargo clippy -q -p
  sipx-app-protocol -p sipx-testkit --all-features --all-targets`, `cargo fmt --all`, and
  `scripts/check-audio-claims.py --check`.

  **Adjacent, deliberately not fixed.** `sipx_app::WssMessage::Binary(Bytes)` derives its `Debug`
  and `sipx-app` is in neither scope. Its frames are not audio under the realtime contract — that
  audio is base64 inside *text* frames — so it is not one of the four, but whether `sipx-app` is a
  third crate where the call rests is the same question this story answered for two others, and it
  should be asked by a story rather than by an implementor widening a constant.

- 2026-08-10: closed at the wave gate — 51 steps, all green, on the merged tree.
