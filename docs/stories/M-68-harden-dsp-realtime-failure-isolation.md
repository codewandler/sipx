---
id: M-68
title: Harden DSP real-time and failure isolation
pillar: Media
status: in-progress
priority: 40
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [sipx-media, dsp, security, realtime, m18]
predicate:
announcement:
note: after M-63/M-64 · measured budgets and explicit fail-open/fail-closed policy
---

# Harden DSP real-time and failure isolation

## Goal

Prove and harden M-64's execution/failure policy so hostile audio and failed isolated processors
cannot panic, retain stack-owned audio or starve call media, while stating the trusted-native
boundary honestly.

## Acceptance

- [ ] The M-63/M-64 policy is hardened with measured frame CPU/allocation budgets, maximum
      consecutive misses, recovery/reset rules and stable fail-open/fail-closed events.
- [x] Impulses, full-scale alternating samples, long silence, DC, discontinuity, rate/channel changes,
      invalid output length and processor error cannot panic or corrupt another call/direction.
- [x] Deadline misses, bypasses, dropped/coalesced frames, resets and terminal failures have typed
      counters/events distinct from intentional glitch effects.
- [x] Proven inline and supervised-isolated processors cannot retain stack-owned borrowed frames,
      grow stack-owned state past declaration, spawn unowned stack work or keep a call/graph alive
      after cancellation; isolated processes are terminated and reaped.
- [x] Bounded stress tests prove RTP remains serviced through isolated worker hangs, crashes,
      malformed results and deadline misses, and all owned work drains through an event/barrier
      rather than a fixed sleep.
- [x] Cooperative-native APIs/docs state that arbitrary trusted code cannot be preempted and may copy
      audio or spawn work; conformance never upgrades that profile to the containment claim.
- [ ] Strict lint/feature, fixed-sleep, adversarial corpus and the full gate are green.

## Progress

- Backlog. Hardening gate after M-63 and M-64.

- 2026-08-09: implemented on `impl/M-68`.

  **What the hardening *found*, as opposed to what it now *guards*.** Three real defects, all in
  `M-64`/`M-102`'s shipped attachment and all reachable from public API.

  1. **`DspGraph`'s `Debug` rendered raw call audio.** `M-61`'s defect, again, in the graph. Every
     type between `DspGraph` and the samples derived `Debug`: `Buffers { front, back, scratch }`
     held the frame in flight, the staging buffer and the 4,096-sample scratch region, and a
     supervised stage's `Supervised { pool, held }` held up to nine more frames. One
     `tracing` field, panic message or failing assertion naming a graph put the call's audio into a
     record **whose length was the frame's** — measured at 10,020 octets for a two-stage graph on
     20 ms narrowband, and the contract's frame ceiling is 65,536 positions. Hand-written `Debug`s
     now report shape: what the graph is carrying, not what it is carrying.

     `scripts/check-audio-claims.py` passed the tree that contained this, and the reason is worth
     recording rather than working around: `M-107`'s carrier reader looks at a public type's **own**
     fields, and `DspGraph` has none — it holds a `SlotRef`. Filed as `M-121`.

  2. **A renegotiation carried a graph onto audio it never agreed to.** `MediaSession::reconfigure`
     re-anchored both directions and kept the chain, whatever the new `Config` said. Every stage
     was prepared for one `StreamFormat` and every buffer sized from one packetisation, and
     `docs/specs/custom-call-dsp.md` §8.3 makes a rate change a `prepare` and never a frame.
     Measured on a µ-law → G.722 re-INVITE (8 kHz/160 → 16 kHz/320): the graph stayed at generation
     1 and **every subsequent frame passed through untouched** under §4's oversize rule —
     `frames_passed_through: 3, stage_frames: 0` — for the rest of the call, with nothing saying so.
     For a `TerminateClosed` stage, whose absence §7.4 calls a policy breach rather than a quality
     regression, that is unprocessed audio leaving the stack while the graph reports itself healthy.
     Now torn down with `TeardownCause::FormatChanged`; `docs/specs/call-dsp-graph.md` §5.5 is the
     new normative paragraph, and a surviving handle's `replace` validates against the **session's**
     current sizing rather than the one the handle was built with.

  3. **Simultaneous bypasses lost all but the last transition.** `Live::run` carried one
     `Option<GraphTransition>` out per frame, so a chain whose stages spend their miss budgets on
     the same frame — which is what a chain of three misbehaving workers does — journalled one of
     them. Bypasses are now pushed where they happen.

  Two smaller gaps, both now closed: the transition journal counted what it dropped and **no caller
  could read the figure**, and §7.4's `Undersized` refusal was the one row of the refusal table with
  no test.

  **What is now guarded rather than fixed.** No panic, no unbounded growth, no cross-call leak and
  no retained borrowed frame was found in the shipped graph. Hostile audio — impulses, alternating
  full scale, DC at both rails, silence, a step, a full-domain ramp — reaches every stage of a live
  call and transforms cleanly; two concurrent calls with swapped sentinels never see each other's
  audio and neither direction sees the other's; a stage that reneges on `cancel` and lies about
  `retained()` cannot keep the graph alive, because §8's barrier is read off the graph's structure
  and not off the processor's word; three misbehaving worker processes cost their declared action
  and never a frame of RTP, and every one is reaped. Those are now tests rather than new behaviour.

  **The new surface.** `GraphCounters`, reached by `DspGraph::counters()`: frames, deadline misses,
  refusals, malformed results, workers lost, bypasses, resets, teardowns, terminal failures, frames
  passed through untouched, and transitions the bounded queue dropped. Cumulative for the life of
  the call and not reset by a replacement or a teardown, so they still answer after a fail-closed
  graph is gone — which is the point of a counter beside a queue that drops its oldest. Normative in
  `docs/specs/call-dsp-graph.md` §6.4, including the half that matters: **there is no counter a
  processor can move by emitting a `DspObservation`, and no observation the runtime synthesises from
  a counter.** An intentional glitch effect's `Saturated` is not a defect, and
  `an_intentional_glitch_effect_moves_no_runtime_counter` drives eight hostile programs through one
  to prove the two vocabularies stay apart.

  **No profile's claim changed, and one test exists to make sure of it.**
  `TrustedCooperativeNative.contains_overrun()` is still `false` unconditionally; a chain holding
  one such stage still claims nothing; `GraphPlan::with_processor`'s rustdoc now states the
  non-claim in full at the door an application actually calls — cannot be preempted, cancelled or
  reaped, may copy audio, may spawn work the stack does not own, may outlive the call, and no
  configured policy changes any of it.
  `nothing_upgrades_the_cooperative_native_profile_to_the_containment_claim` tries the two things
  that most look like they should buy something: a clean run in which no counter moves, and the
  strictest `ExecutionPolicy` the vocabulary can express. Neither does.

  **Failing-first.** `a_graph_diagnostic_carries_no_call_audio` in
  `crates/sipx-media/tests/dsp_adversarial.rs`, at merge base `413f19c`:

  ```
  $ cargo test -p sipx-media --test dsp_adversarial
  test a_graph_diagnostic_carries_no_call_audio ... FAILED
  the outbound graph's diagnostic rendering carries a call's sample value: DspGraph { slot:
  SlotRef { slot: Mutex { data: Slot { live: Some(Live { … buffers: Buffers { front: [30011,
  30011, 30011, … ×160 ], back: [], scratch: [0, 0, 0, … ×4096 ] … 
  ```

  The renegotiation finding was proved the same way, against the pre-fix `re_anchor`:

  ```
  test a_renegotiation_that_changes_the_format_tears_the_graph_down_rather_than_lying_to_it ... FAILED
  assertion `left == right` failed: the graph did not survive it
    left: 1
   right: 0
  ```

  **Coverage.** `crates/sipx-media/tests/dsp_adversarial.rs` (8): the diagnostics sweep, the
  hostile-audio and cross-call/cross-direction isolation sweep, refusal versus invalid output length
  counted apart, the format-change teardown and the recovery after it, the cancellation zombie, the
  three-worker RTP stress with the bounded queues checked per frame, the profile-claim sweep and the
  glitch-effect counter sweep. `crates/sipx-media/src/dsp/wire.rs` (+1): §7.4's `Undersized` row.

  **The gate row is left unticked deliberately**, and the first row with it.

  Row 1 needs *measured* frame CPU and allocation budgets, and those are `X-109`'s — being
  implemented concurrently, and its Acceptance is where "CPU, allocation/state high-water marks and
  deadline/glitch/drop counts" against "thresholds declared before the measured implementation run"
  live. What this story could do without those measurements it did: the maximum-consecutive-miss
  budget, the recovery and reset rules, and stable typed fail-open and fail-closed events are all
  implemented, counted and tested. The budgets are the part that cannot be honestly ticked from
  here, so the row is not.

  Row 7 names an **adversarial corpus** and there is no new fuzz target. The epic's one
  hostile-octet surface is `sipx_media::dsp::wire`, which is crate-private, and `fuzz/` depends on
  `sipx-sip` and `sipx-testkit` and reaches neither it nor `sipx-media`. Registering a target means
  editing a dependency list and a lockfile, which this story's fence forbids and which is a
  dispatch-level decision rather than something to work around. Filed as `M-122` with the two
  shapes that would work. What was done instead is the refusal table walked deliberately — that is
  a table, not a campaign, and it is not claimed as one. Everything else row 7 names is green and
  recorded under GATE below; the full `./scripts/gate.py` is the integrator's.

  **CHANGELOG sentence owed** (the integrator writes it; this story may not touch that file):
  *Hardened the call-DSP graph's real-time and failure isolation: typed per-direction runtime
  counters that survive the bounded transition queue and never carry a processor's own observations,
  a graph diagnostic that reports shape rather than the call's audio, a renegotiation that changes
  the audio format or packetisation now tearing the graph down instead of carrying it onto audio its
  stages never agreed to, and an adversarial suite over hostile audio, hostile processors and
  misbehaving worker processes (`M-68`).*

- 2026-08-10: shipped in `1.0.0-rc.18` and **left in-progress**. Three defects, the
  containment tests, the typed counters and the refusal-table row are all in the release.
  Row 1 stays unticked for the reason the implementation gave: it needs *measured* frame CPU
  and allocation budgets, and CPU is `X-109`'s figure, still untaken on a loaded machine.
