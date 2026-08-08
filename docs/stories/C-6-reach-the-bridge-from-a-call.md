---
id: C-6
title: Reach the bridge and the conference from a call
pillar: Signalling
status: done
priority: 18
design: docs/designs/app-sdk.md
epic: app-sdk
areas: [sipx-call, sipx-media]
note: app-sdk · last; not v1-blocking · C-1 (M9) later upgrades the signalling half · size M
---

# Reach the bridge and the conference from a call

## Goal
Two calls a host owns can be bridged — and several joined to a conference — through the public
API, without `Arc<Mutex<Call>>` and without constructing media sessions from raw ports.

## Acceptance
- [x] Two `Call`s can be connected so audio passes between them, using `M-11`'s bridge
      underneath; the connection is made through the calls' public API, and `Call`'s ownership
      story survives it — media sharing stays channel-backed, no shared mutable session
      (the vision's principle 3).
- [x] DTMF while bridged has a declared, tested behaviour: pass through or deliver to the host,
      selectable when the bridge is made.
- [x] Unbridging returns both calls to independent operation; ending either call ends the bridge
      and is observable on the other's event stream (`C-3`).
- [x] A `Call` can join and leave `M-12`'s conference through the same public path.
- [x] Failing-first test: `two_calls_bridge_and_pass_audio`.

## Progress
- 2026-08-08: delivered. `crates/sipx-call/src/bridge.rs` and `crates/sipx-call/src/conference.rs`
  are the new `Call`-facing path to `M-11`'s bridge and `M-12`'s mixer; seven tests in
  `crates/sipx-call/tests/bridge.rs`.

  **Failing-first, at `ae65b5b`:** `cargo test -p sipx-call --all-features --test bridge
  two_calls_bridge_and_pass_audio` →
  ``error[E0432]: unresolved imports `sipx_call::CallBridge`, `sipx_call::CallConference`,
  `sipx_call::DtmfBridging`, `sipx_call::UnbridgeCause` … no `CallBridge` in the root``,
  ``error[E0433]: cannot find `BridgeOptions` in `sipx_call` ``, ``error[E0599]: no method named
  `is_bridged` found for struct `Call` ``, ``error[E0599]: no variant named `Unbridged` found for
  enum `CallEvent` `` — 11 errors, the whole public path absent.

  **New public surface.** `CallBridge::{connect, connect_with, is_connected, is_transcoding,
  dtmf, unbridge}`, `BridgeOptions::{new, with_dtmf, dtmf}`, `DtmfBridging::{Deliver,
  PassThrough}`, `UnbridgeCause::{Released, PeerEnded}`, `CallConference::{new, narrowband, join,
  leave, len, is_empty, samples_per_frame, close}`, `Participant`, `Call::is_bridged`, and
  `CallEvent::{Bridged, Unbridged{cause}}`. Both new enums are `#[non_exhaustive]`.

  **Ownership.** `connect` takes `&mut Call` for each call and keeps neither borrow — so a call
  cannot be bridged to itself (the compiler says so), and the host goes on owning both calls and
  driving their signalling. What the bridge holds is two `Arc<MediaSession>` handles; audio moves
  over the sessions' own channels. No `Arc<Mutex<Call>>`, no shared mutable session, no raw port.

  **DTMF, declared.** `DtmfBridging::Deliver` is the default and is today's behaviour: the
  keypress stays on the call it arrived on. `DtmfBridging::PassThrough`, selected through
  `BridgeOptions` when the bridge is made, has the bridge consume the keypress and *regenerate* it
  on the other leg — not relay the payload, because the two legs negotiate `telephone-event`
  payload types independently. The two are exclusive by construction: a session's keypress queue
  has one consumer.

  **Ordering.** `Unbridged` is emitted while the bridge's own lock is held, and a call takes that
  same lock before emitting `Ended`, so `Ended` stays last on every stream even if both calls end
  at once. The ending call gets no `Unbridged` at all; only its peer does.

  **A trap worth recording:** `sipx_media::Bridge` sets the encoded-relay flag on both sessions and
  deliberately never clears it, so unbridging without clearing it leaves both calls deaf.
  `CallBridge` clears it on every teardown path, and `Bridge::close`'s documentation now says so.

  **Not done here, filed as `C-9`:** a bridge or a conference participation does not survive its
  call renegotiating media (a re-INVITE replaces the `MediaSession` and the coupling keeps
  forwarding the stopped one). Documented at both modules and reported by `is_connected`.

  **CHANGELOG sentence the coordinator needs** (this story did not touch that file):
  `Two calls a host owns can now be bridged, and several joined to a conference, through
  \`sipx_call::CallBridge\` and \`sipx_call::CallConference\` — the media coupling only, with each
  \`Call\` still owned and driven by the host. DTMF while bridged is selectable when the bridge is
  made (\`DtmfBridging\`), and \`CallEvent::Bridged\`/\`CallEvent::Unbridged\` report both ends of
  it on the call event stream.`

  Verified with `cargo test -p sipx-call --all-features` (all suites green), `cargo test -p
  sipx-media --all-features`, `cargo clippy --workspace --all-targets --all-features --no-deps -D
  warnings`, `cargo fmt --all`, `cargo doc -p sipx-call -p sipx-media --no-deps --all-features`,
  `./scripts/check-fixed-sleep.py --check`, `./scripts/check-app-surface.py --check`,
  `./scripts/check-provenance.sh`, `./scripts/check-docs-links.py`. The full `./scripts/gate.py`
  was deliberately not run here; it is run once for the wave.

## Notes
- Was: `Bridge::connect(Arc<MediaSession>, Arc<MediaSession>)` and `Conference::join` existed
  only at the media layer and were unreachable from `Call`, which lends `&MediaSession`
  (`crates/sipx-media/src/bridge.rs`, `crates/sipx-call/src/call/mod.rs`). Only tests constructed
  bridges, from raw ports.
- Scope: the **media** coupling of two host-owned calls. The signalling coupling — offer relay
  on every axis, glare, CANCEL/BYE mapping — is `C-1`, deliberately later (M9, after `S-19` and
  `C-2`), and upgrades the contract's `bridge` verb without changing it.
- Needed by the host (`crates/sipx-app`): `bridge` and `dial`-then-connect are contract
  verbs.
