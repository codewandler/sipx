---
id: M-97
title: Close and test the call-to-contract event bridge
pillar: Media
status: backlog
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

- [ ] A failing-first test proves an app-protocol client receives `call.signal.metrics` and
      `call.signal.silence` for a call with `Call::report_signal_metrics` running — they are
      specified, typed and round-tripped today, and `event_from_call` has no arm for them, so no
      host can emit one.
- [ ] `sipx_app_protocol::event_from_call` has tests. It has none, because the payload types on
      `sipx-call`'s side (`VoiceActivity`, `SignalMetrics`, `VoiceThresholds`) have private fields
      and no constructor, so nothing outside `sipx-call` can build the input. Whatever closes that
      is a decision about `sipx-call`'s test surface, not a `#[doc(hidden)]` reached for in passing.
- [ ] A check that a §5.3 row with no `event_from_call` arm is a red build, in the same derived
      style as `tests/spec_tables.rs` — the gap this story exists for went unnoticed because
      "the crate has the variant" and "a call can produce it" are different claims and only the
      first was checked.
- [ ] The full gate is green.

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
