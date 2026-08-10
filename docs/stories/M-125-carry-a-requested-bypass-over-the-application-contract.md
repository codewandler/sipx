---
id: M-125
title: Carry a requested bypass over the application contract
pillar: Media
status: done
priority: 2
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [sipx-app, sipx-app-protocol, dsp, contract]
predicate:
announcement:
note: after M-115 · the graph door exists and an application driving it over the wire sees half of it
---

# Carry a requested bypass over the application contract

## Goal

Give an application driving a call's DSP graph **over the wire** the same bypass `M-115` gave a
Rust caller: a verb to ask with, an event when it takes effect, and the same when it is undone.

## Context

`M-115` added `DspGraph::set_bypassed`, so one stage can step out of a live chain and back in without
replacing the chain. It deliberately did not grow the application contract, and its reasoning was
sound at the time: a fourth `dsp` verb widens a surface already carrying three, and that is a
decision about the contract rather than about the graph.

What it leaves behind is a half. The event side already exists — `crates/sipx-app/src/dsp.rs` maps
`BypassCause::Requested` to the protocol's `DspBypassCause::Requested` — but nothing over the wire can
*ask*, and `GraphTransition::Restored` is unmapped, dropped by the driver's catch-all. So an
application watching a graph it drives sees a bypass it did not request and never sees the restore.

That asymmetry is the shape `M-103` and `M-108` both closed elsewhere: a row that is specified,
typed, and reachable from nothing. The contract's reachability table now checks composition rather
than mere absence, and a new event must point at the thing that produces it.

`M-115` also decided two refusals that this verb inherits and must not soften: a supervised stage
cannot be bypassed, because its output lags its input by the declared deadline and the gap would be
paid for in audio from the wrong part of the call; and a stage the *runtime* bypassed cannot be
restored by an application, because otherwise a failing stage can be pinned on the media path
forever by restoring it after every failure.

## Acceptance

- [x] A `dsp_bypass` verb (name to be settled with the existing three) carries stage and desired
      state, and refuses exactly what `set_bypassed` refuses, with the refusal typed on the wire.
- [x] `call.dsp.restored` exists, is produced, and the contract's reachability table holds it
      against the thing that composes it rather than against its absence from the bridge.
- [x] `spec_tables.rs` names the producer for both rows, so neither can become specified and
      unreachable.
- [x] An application cannot reach another call's graph through the new verb, and cannot bypass a
      supervised stage or restore a runtime-imposed bypass — each refused, not ignored.
- [x] A failing-first test at the interpreter level and one over a real socket.
- [x] The gate is green.

## Progress

- 2026-08-10: **done in 1.0.0-rc.22.** The complete local acceptance gate passed all 51 steps, exact
  main CI run 31412337169 passed, and protected release run 31413202714 attempt 2 published and
  verified the annotated `v1.0.0-rc.22` tag.

- 2026-08-10: selected in the five-story rc.22 wave.
- 2026-08-10: failing-first interpreter proof: `cargo test -p sipx-app-protocol --test vectors
  dsp_bypass_becomes_one_desired_state_effect_for_this_call -- --exact` produced no graph effect
  (`[ClearTimer(Callback)]`) because `dsp_bypass` was still an unknown verb.
- 2026-08-10: failing-first socket proof: `cargo test -p sipx-app --test host
  requested_bypass_and_restore_are_reported_over_the_callback_socket -- --exact` timed out waiting
  for the requested bypass boundary after the call answered and its graph activated.
- 2026-08-10: `cargo test -p sipx-app-protocol -p sipx-app` passes, as do
  `cargo clippy -p sipx-app-protocol --all-targets -- -D warnings` and
  `cargo clippy -p sipx-app --lib -- -D warnings`. The coordinator owns the full gate row.

- Filed 2026-08-10 by the `M-115` implementor, who named the gap rather than widening the contract
  inside another story's fence.
