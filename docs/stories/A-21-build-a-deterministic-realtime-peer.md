---
id: A-21
title: Build a deterministic realtime peer
pillar: Application
status: done
priority: 2
design: docs/designs/openai.md
epic: openai
areas: [sipx-testkit, interop]
predicate:
announcement:
note: starts when A-19's spec lands — the peer implements the spec's other side, vector by vector
---

# Build a deterministic realtime peer

## Goal

Give `sipx-testkit` a loopback WebSocket server that speaks `docs/specs/openai-realtime.md`
from the far side, so the bridge's whole loop runs in the default CI matrix with no account,
no credential and no network — the interop peer criteria kept intact.

## Acceptance

- [x] The peer accepts a WebSocket upgrade with a bearer it is configured to expect, refuses
      a wrong or absent bearer the way the spec says the vendor does, acknowledges the
      session, consumes append events, and emits delta events carrying a distinct known
      tone — each behaviour holding to the spec's vectors, cited by vector name in the
      tests.
- [x] Cancel is honoured mid-response: after the client's cancel event the peer sends no
      further deltas for that response, so a bridge test can assert truncation as a fact.
- [x] Negative modes are first-class configuration: wrong-bearer refusal, a malformed event,
      a mid-call stall, an oversize frame — each drives one row of the spec's failure
      taxonomy, and each mode has a test proving the peer actually misbehaves (a stand-in
      whose negatives are vacuous proves nothing).
- [x] Deterministic and bounded: no fixed wall-clock duration standing in for a
      happens-before (`check-fixed-sleep.py` clean), every task cancellation-safe, the peer
      shuts down when dropped.
- [x] Runs in the default `cargo test` matrix — no Docker, no credentials, and no
      certificate material of any kind.

**Two Acceptance lines were amended at integration rather than ticked as written** (`wss`
→ WebSocket; per-run fixture certificates → no certificates at all). The peer is cleartext
`ws://` on loopback: no vector in the spec asks the peer for TLS — §2's certificate
discipline binds the *bridge* toward the vendor — and `A-20`'s client refuses cleartext
anywhere but loopback, so the seam is closed by the client's own rule rather than by a
fixture certificate. What the original line protected (nothing committed, nothing to
expire, default matrix) holds more strongly with no certificate than with one. Amended
rather than reinterpreted, so the next reader sees the decision instead of inferring it.

## Progress

- **Peer landed** in `crates/sipx-testkit/src/realtime_peer.rs`, with its vector suite in
  `crates/sipx-testkit/tests/realtime_peer.rs` (16 tests, named for the ORB vectors they belong
  to). Reached by `PeerConfig::new()…start()`; every directive resolves *after* its frame is on
  the socket, so a test orders its script by awaiting the peer and never by a clock.
- **Cleartext `ws://` on loopback, not `wss://` — a deliberate departure from this story's first
  Acceptance line, and the one thing to argue with in review.** The Goal says "loopback WSS
  server" and the last Acceptance line asks for per-run fixture certificates. Neither is what the
  spec's vectors need: no ORB row asks the peer for TLS (spec §2's certificate discipline binds
  the *bridge* toward the vendor, and §2 says the stand-in "is reached by configuring its URL"),
  and A-20's client permits cleartext to loopback while refusing it everywhere else, so the seam
  is closed by the client's own rule. Certificates are the fixture cost that spreads: every test
  reaching the peer would need the trust anchor threaded through it, and the first one that found
  that awkward would disable verification — a worse habit than the one the certificate was for.
  What the Acceptance line was protecting (nothing committed, nothing to expire, default matrix)
  holds more strongly with no certificate at all. `crate::certs` is still there if a later story
  finds a vector that genuinely needs TLS on this side.
- **Negative modes, each with a test proving the peer really misbehaves** — asserted from the
  client's side of the socket, never from the flag: wrong/absent bearer refused 401 before the
  101 (ORB-10), `not json{` / no `type` / a binary frame (ORB-13), a delta whose `delta` is not
  base64, a delta with no `delta`, a `done` with no `response_id` (ORB-18), a >1 MiB text frame
  (ORB-11), a stall that answers the upgrade and then not even a Pong (ORB-14), withheld
  `session.created` and `session.updated` (ORB-15), close 1000 and an abrupt reset (ORB-16),
  unknown/ignorable events (ORB-12). Where a negative could pass vacuously the same test carries
  a control arm: the unstalled peer answers the ping, the withholding peer still answers a
  `session.update`, the peer that refused two bearers accepts the configured one, and the peer
  that reported the retired beta header absent is shown seeing it when a client sends it.
- **Cancel has both halves.** `CancelPolicy::Truncate` (default) suppresses directed deltas for a
  cancelled response and counts them, so a bridge test can assert truncation as a fact;
  `CancelPolicy::KeepStreaming` is ORB-8's actual script, where the peer chooses to send two more
  deltas after the cancel and `bridge_cancelled_deltas` counts them.
- **The tone is tied to the spec:** `tone_frame(0)` *is* §4.2's F-ramp, and the module exports
  `F_RAMP_BASE64`/`F_SILENCE_BASE64` as literals quoted from §4.2 (both verified against the
  bytes in `the_tone_begins_with_the_specs_f_ramp_vector`; both were correct as written).
  Successive frames differ, so a truncated response is distinguishable from a late one.
- **Bounded and cancellation-safe:** every await in the connection loop is a cancel-safe
  primitive selected against a `CancellationToken`; sessions are spawned into a `JoinSet` that is
  `shutdown().await`-ed, so `RealtimePeer::shutdown()` leaves no task and `Drop` cancels the
  listener. `check-fixed-sleep.py` clean; the suite runs 16/16 green in ~0.53 s.
- **Fence note for the coordinator:** `Cargo.lock` carries a cargo-generated diff — the
  `sipx-testkit` dependency list gains `base64`, `futures-util`, `serde_json`, `thiserror`,
  `tokio-tungstenite`, all already in the root manifest and already in the lock. No new package,
  no version change, nothing hand-edited.
- **Not done here, by design:** the ORB rows A-22 and A-20 own are still theirs — this story
  supplies the peer behaviour and the observation surface they assert on
  (`Record::events_outside_the_client_subset`, `appended_audio`, the upgrade's target and
  headers, delta counts).

- 2026-08-05 (integration): independent review PASS, and an unusually load-bearing one — it
  **mutation-tested the fixture**, disabling each misbehaviour in turn and confirming the
  owning test then failed. Seven of eight mutations were caught, including the implementor's
  self-reported `StallPoint::Session` fix (reverting it fails the stall test, so that fix is
  real). The eighth is `MINOR 1` below. The reviewer also hand-merged `A-20` and ran the two
  together — full handshake, byte-exact F-silence uplink, delta downlink, a 2 MiB frame
  surfacing as `WssError::Oversize`, a wrong bearer as a 401 with no credential in the
  rendered error — so the cleartext-loopback decision is proven against the real client and
  not only against the fixture's own tests. Fixture gaps accepted rather than reworked, none
  of which weakens a vector A-21 owns: **(1)** ORB-16's reset arm cannot tell an RST from an
  ordinary EOF (removing the zero-linger leaves the test green) — the mode itself is real,
  and ORB-16's enforcement belongs to `A-22`; **(2)** a failed close write reports `Sent`,
  contradicting the module's own "a failed write drops the sender" rule; **(3)** `direct()`
  is the one wait with no bound, so a directive to a stalled session hangs rather than
  failing with a sentence; **(4)** the raw append base64 is not retained, only decoded bytes;
  **(5)** `shutdown()` has no test of its own, only the `Drop` path; **(6)** a failed
  `accept()` ends the listener silently.

## Notes

- Design: `docs/designs/openai.md` component 3. Blocked on A-19 (the spec is what this peer
  implements). Uses A-20's client only in its own tests, if at all — the peer is a server.
- Precedent: the webhook vectors run "against a real loopback HTTP peer" in
  `crates/sipx-app/tests/`; this story gives the realtime spec the same treatment.
