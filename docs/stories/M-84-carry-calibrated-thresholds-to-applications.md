---
id: M-84
title: Carry calibrated thresholds onto the application wire
pillar: Media
status: in-progress
priority: 26
design: docs/designs/call-audio-analysis.md
epic: call-audio-analysis
areas: [sipx-call, sipx-app, app-sdk, audio-analysis, vad, m16]
predicate:
announcement:
note: after M-60 · the effective thresholds are Rust-only; an app-protocol client cannot see them
---

# Carry calibrated thresholds onto the application wire

## Goal

Let an application on the app protocol see what a call's voice-activity detection is measuring
against, and when it changed, so a calibrated threshold is as observable to an SDK client as it is
to a Rust caller.

## Acceptance

- [x] A failing-first test proves an app-protocol client can read the effective thresholds of a call
      with detection running, and gets nothing for a call without it.
- [x] Threshold movement is observable without polling: either a typed event or a field on an
      existing one, specified in `docs/specs/app-contract.md` as a compatible `v1` addition and
      derived-tested against that spec the way `call.voice.started` is.
- [x] The wire surface carries no audio and no field an application could reconstruct audio from,
      and the check that would catch a regression there is named.
- [ ] Generated bindings and docs are updated and the full gate is green.

## Progress

- Backlog. Filed by `M-60`, which stopped at the Rust surface deliberately.

- 2026-08-09: implemented, except the gate row — the gate is run once per wave by the coordinator,
  not by this story.

- **The read is §5.2's snapshot, not a query verb.** `CallSnapshot` gains an optional `voice`
  member, present exactly when detection is running on the call and *absent* — not `null` — when it
  is not. This contract has no query shape and adding one would have been wrong three times over:
  §2 already makes every event carry a full snapshot, so "what is it measuring against now?" is
  answered by the last envelope an app received; document mode's alternation (§6.3) has no room for
  an unsolicited question; and §3 requires every verb to resolve to a call-framework operation,
  which a read is not. `M-60`'s call-side channel is latest-value and this is the same semantics on
  the wire.

- **The announcement is one new event type, `call.voice.thresholds`** (§5.3), carrying `sample_time`
  and the *same* `thresholds` object §5.2.1 defines — one spelling, so the read and the announcement
  cannot disagree. It is sent once when detection first has audio, so a call whose threshold never
  moves still says what it is measuring against, and thereafter only when calibration moved it:
  §12.7's "a settled threshold is silent" carried onto the wire unchanged. An analyser with no
  calibration profile costs exactly one event for the life of the call.

- **It carries no delta.** The value being replaced is deliberately not on the event, because §2's
  rule is that this contract never sends one — an app that missed a delivery is corrected by the
  snapshot riding the next envelope rather than left reassembling a history. That is also why a
  dropped announcement is survivable, and it is why the record is the *reset-invariant* subset of
  §12.9: the update count, the last observed floor and the last period's outcome all stay in
  process, because §12.8 clears each of them at a reset and a wire copy would go stale silently.
  Adding any of them later is a §4 *field* addition, which both sides already have to tolerate.

- **Everything is a sample count or an amplitude.** `direction`, `sample_rate`,
  `activation_amplitude`, `window_samples`, `hangover_samples`, `silence_timeout_samples`,
  `calibration_samples`, `update_samples`, `freeze_limit_samples`. No `*_ms` field anywhere, so
  `check-fixed-sleep.py` has nothing to say and a recorded call reproduces the same numbers on every
  host. `calibration_samples: null` is load-bearing: it tells an application this call will never
  send an announcement, so nothing waits on one.

- **The privacy boundary is enforced, not described.** Three things hold it, and none is prose:
  the record has no field that could hold a sample sequence; `the_voice_member_carries_only_counts_
  and_amplitudes` in `sipx-app-protocol`'s `tests/spec_tables.rs` serializes both shapes the record
  appears in and refuses any member that is not a number, a `null`, or one of the two direction
  words — so audio as an array or as base64 both fail the build; and
  `section_5_2_1_documents_exactly_the_voice_members_the_crate_writes` holds the member set against
  §5.2's own example *and* §5.2.1's table, so widening the surface is a reviewable diff against the
  specification. Upstream of all three, §3.3 and §8.1 mean there is no retained audio to send.

- **Failing-first, at merge base `2a91041`,**
  `cargo test -p sipx-app-protocol --all-features --test voice_thresholds`:

  ```
  running 2 tests
  test a_call_without_detection_carries_no_thresholds ... ok
  test an_app_protocol_client_reads_the_thresholds_of_a_call_with_detection_running ... FAILED

  ---- an_app_protocol_client_reads_the_thresholds_of_a_call_with_detection_running stdout ----
  thread '…' panicked at crates/sipx-app-protocol/tests/voice_thresholds.rs:80:10:
  a call with detection running says what it is measuring against

  test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
  ```

  The test is written in wire terms — JSON in, JSON out — precisely so that it *compiles* at the
  base and fails on the missing read: §4 already made the host tolerate an unknown event type, so
  the base delivers an envelope whose `event.type` is `call.voice.thresholds` and whose snapshot has
  nothing on it. That is the gap stated exactly.

- **Owed `CHANGELOG.md` sentence** (fenced for this story; the coordinator writes it):
  *Applications on the `sipx.app.v1` contract can now read what a call's voice-activity detection is
  measuring against, as the call snapshot's `voice` member, and are told when calibration moves it
  by the new `call.voice.thresholds` event. Every field is a sample count or an amplitude; the wire
  carries no audio.*

- **Filed `M-97`**: `call.signal.metrics` and `call.signal.silence` are specified, typed and
  round-tripped but have no `event_from_call` arm, so no host can emit one — found while adding the
  arm next to them — and `event_from_call` has no tests at all, because `sipx-call`'s payload types
  cannot be built from outside that crate.

- `sipx-app/src/host.rs` grew three `Box::pin(actor.run())`s. The contract's call snapshot got
  bigger, which pushed the call actor's future past `clippy::large_futures`' 16 KiB budget. It was
  already spawned inside a `Box::pin`, so the storage was on the heap either way; pinning the inner
  future makes that true for the reader as well as for the allocator.

## Notes

- `M-60` shipped calibration with two inspection points: `sipx_audio::analysis::AudioAnalyzer::
  thresholds()` and `sipx_call::Call::voice_thresholds()`, both `&self`, both returning a `Copy`
  `EffectiveThresholds` carrying no audio (`docs/specs/call-audio-processing.md` §12.9). Neither is
  reachable from the app protocol, so an SDK client can be told voice started but not what the
  decision was made against.
- `Observation::ThresholdUpdated` already exists and is deterministic, so the event half of this is
  carriage rather than new analysis — the shape `M-58` used for `call.voice.started` /
  `call.voice.ended` applies directly (`docs/specs/app-contract.md` §5.3, §4).
- Weigh the event against the read: a threshold that settles is silent, so an event stream costs
  nothing in steady state, but a client that only ever wants "what now?" is better served by a read.
  `M-60`'s call-side channel is already latest-value, which suits both.

- 2026-08-08: renumbered from `M-80` on filing. `M-80` had already been allocated in the same wave,
  to the `#[non_exhaustive]` decision for `Encoded` and `Packet`. Only the id moved.
