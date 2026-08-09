---
id: M-98
title: Close and test the call-to-contract event bridge
pillar: Media
status: done
priority: 27
design: docs/designs/app-sdk.md
epic: app-sdk
areas: [sipx-call, sipx-app-protocol, app-sdk, audio-analysis, m16]
predicate:
announcement:
note: filed by M-84 · `call.signal.metrics` is specified and typed but unreachable, and `event_from_call` has no tests at all
---

# Close and test the call-to-contract event bridge

## Goal

Make every event type `docs/specs/app-contract.md` §5.3 lists actually reachable from a call, and
make the one function that maps between the two vocabularies testable.

## Acceptance

- [x] A failing-first test proves an app-protocol client receives `call.signal.metrics` and
      `call.signal.silence` for a call with `Call::report_signal_metrics` running — they are
      specified, typed and round-tripped today, and `event_from_call` has no arm for them, so no
      host can emit one.
- [x] `sipx_app_protocol::event_from_call` has tests. It has none, because the payload types on
      `sipx-call`'s side (`VoiceActivity`, `SignalMetrics`, `VoiceThresholds`) have private fields
      and no constructor, so nothing outside `sipx-call` can build the input. Whatever closes that
      is a decision about `sipx-call`'s test surface, not a `#[doc(hidden)]` reached for in passing.
- [x] A check that a §5.3 row with no `event_from_call` arm is a red build, in the same derived
      style as `tests/spec_tables.rs` — the gap this story exists for went unnoticed because
      "the crate has the variant" and "a call can produce it" are different claims and only the
      first was checked.
- [x] The full gate is green.

## Progress

- Backlog. Filed by `M-84` on 2026-08-09, which added the `call.voice.thresholds` arm and found the
  `SignalMetrics` one missing next to it.

## Notes

- `M-59` shipped `EventKind::SignalMetrics` / `SignalSilence`, §5.3's rows for them, and their wire
  round trip. `crates/sipx-app-protocol/src/call.rs`'s `event_from_call` maps eleven `CallEvent`
  variants and neither of those two, so `CallEvent::SignalMetrics` falls into the `_ => return None`
  arm — which is documented as meaning "the contract has no spelling for this" and here means the
  opposite.
- `sipx-app` never asks a call for detection or metrics either
  (`Call::detect_voice_activity` and `Call::report_signal_metrics` have no caller in
  `crates/sipx-app`). That is a separate question — nothing in the contract's §6.2 turns analysis on
  — and is worth its own story rather than being smuggled into this one.

- 2026-08-09: implemented on `impl/M-98`. Three rows ticked; the `gate` row is the coordinator's.

  **Failing-first, at the merge base `6b6d32c`.** The derived check is
  `crates/sipx-app-protocol/tests/spec_tables.rs::section_5_3_s_rows_are_reachable_through_the_bridge`,
  which reads §5.3's rows and holds each against the bridge's own source. Red before the fix:

  ```
  $ cargo test -p sipx-app-protocol --test spec_tables \
        section_5_3_s_rows_are_reachable_through_the_bridge
  test section_5_3_s_rows_are_reachable_through_the_bridge ... FAILED
  §5.3 lists call.signal.metrics and the bridge has no arm producing `EventKind::SignalMetrics`,
  so no call can reach it — add the arm, or name the row in `COMPOSED_BY_THE_DRIVER` with the
  reason the driver composes it
  ```

  The client-level test the first Acceptance row asks for could not be written at the base at all,
  and that is the second half of the defect rather than a shortcut: nothing outside `sipx-call`
  could build a `CallEvent::SignalMetrics`, so the test does not compile there.

  **What was missing.** `CallEvent::SignalMetrics` now maps to `call.signal.metrics` for a
  completed report and to `call.signal.silence` for a silence transition; a reset and the analyser's
  coalesced-observation marker are named as the two the contract has no row for. `CallEvent`'s other
  wildcard-swallowed variants are named too, in `has_no_contract_event`: `Muted`/`Unmuted` (§5.2),
  `Bridged`/`Unbridged` (§5.3 names a leg the call event does not carry), `ApplicationRequest` (no
  row). The trailing `_ => return None` now means only "a variant `C-3` grew after this was
  written", which is what it always claimed to mean.

  **A second defect of the same class, found by testing the arms.** `EndCause::RemoteCancel` fell
  through to the contract's `error` — "the host could not go on" — for a far end that withdrew an
  invitation, while `sipx-call`'s own rustdoc says both far-end endings are §5.3's `remote`. Fixed
  to `remote`, asserted in `both_far_end_endings_are_the_contracts_remote_cause`.

  **Testability.** `VoiceActivity::new`, `VoiceThresholds::new` (from an `AnalysisProfile`, since
  `EffectiveThresholds` is `sipx-audio`'s to hand out), `SignalMetrics::new`,
  `sipx_call::signal_metrics::measure` — which runs the real analyser and reducer over stated
  samples, so no test types a measurement — and `sipx_media::PlaybackId::new`. Every `CallEvent`
  variant is now covered except `ApplicationRequest`, which owns a live server transaction's
  response capability and cannot be forged without lying about a transaction; its arm binds nothing.

  **CHANGELOG sentence owed** (this file may not write it): *Fixed — `call.signal.metrics` and
  `call.signal.silence` are reachable from a call at last, and a call the far end cancelled is
  reported to an application as `remote` rather than `error`.*

  **Board regeneration owed:** `M-99` was filed from this work (§5.3's `call.bridged` and
  `call.unbridged` have no producer anywhere), so `/track:board` has to run at integration.

- 2026-08-09: renumbered from `M-97` to `M-98`. The coordinator reserved the same `M-97`/`M-98` pair
  to two implementors running at once, so the scheme meant to stop id collisions produced one — the
  second time that has happened. Only the id moved. A reservation has to be disjoint per agent, not
  merely stated.

- 2026-08-09: closed at the `1.0.0-rc.15` boundary, against the wave gate run on this tree.
