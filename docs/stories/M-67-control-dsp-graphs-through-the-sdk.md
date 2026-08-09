---
id: M-67
title: Control call DSP graphs through the application SDK
pillar: Media
status: done
priority: 40
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [sipx-call, app-sdk, dsp, m18]
predicate:
announcement:
note: after M-64 · typed registry and sample-boundary parameters, never SDK callbacks on media work
---

# Control call DSP graphs through the application SDK

## Goal

Let applications compose and change registered DSP processors while preserving the media worker's
bounded, non-callback execution model.

## Acceptance

- [x] The SDK exposes processor discovery, graph generation, direction/order, closed parameter
      schemas and typed activation/bypass/failure/removal events.
- [x] Parameter updates are finite, validated off the media path and applied at a declared sample
      boundary with generation/correlation identity and exactly one terminal outcome.
- [x] SDK/JavaScript callbacks never execute on the media worker; applications select registered
      processor IDs and receive bounded events rather than borrowing audio-thread objects.
- [x] Unknown processors/parameters, stale generations, cross-call IDs and overlong graphs are
      refused without changing the active graph.
- [x] Event queues coalesce safe intermediate parameter state but preserve activation, failure,
      bypass and terminal transitions without blocking media.
- [x] Generated bindings, sequence tests, public reference docs and the full gate are green.

## Progress

- Backlog. Application surface after M-64.
- **2026-08-09 — the door is built, on `impl/M-67`.** Five of six rows ticked; the `gate` row is
  left for the coordinator's wave run.

  **What was added, in three layers.**

  *`sipx-media` (`dsp`)* — the registry is published: `BuiltIn::registered()`,
  `BuiltIn::from_id(id, shape)` and `BuiltIn::parameters()`, which is the list `builtin.rs` had
  said "`M-67` is where an SDK gets a reason to publish one" about. `GraphTransition::Configured`
  joins §5.3, and `DspGraph::configure(generation, processor, parameters)` moves one live stage's
  parameters — validated against the declared schema off the media path, applied under the same
  take a frame needs, and returning a `ParameterUpdate` that is the update's **one** terminal
  outcome. `DspGraph::next_transitions()` gives the queue a suspension point so a driver never
  polls, and `Journal::push` now coalesces a superseded `Configured` for the same stage of the same
  generation so a burst of parameter updates cannot push an activation out of a bounded queue.

  *`sipx-app-protocol`* — §6.2's `dsp`, `dsp_param` and `dsp_remove`, §5.3's `call.dsp.activated`,
  `.configured`, `.bypassed`, `.removed` and `.refused`, and the values they carry (`DspStage`,
  `DspParameter`, `DspValue`, `DspBypassCause`, `DspTeardownCause`, `DspRefusal`) in a new private
  `dsp` module. `docs/specs/app-contract.md` gains the §3, §5.3 and §6.2 rows plus a §6.6 stating
  the limit.

  *`sipx-app`* — `crates/sipx-app/src/dsp.rs` is the producer the §5.3 rule demands, and
  `spec_tables.rs` names it in `COMPOSED_BY_THE_DRIVER` for all five rows. `host.rs` holds a
  `CallGraphs` per call, performs the three effects and takes unsolicited transitions off a
  `select!` arm.

  **What an application can do to a graph:** name registered identifiers in order on one direction,
  give each a shape and a finite parameter set, move one live stage's parameters against a named
  generation, remove the chain, and read every transition.

  **What it cannot do:** supply a processor, a program, a callback, an execution profile, a
  deadline, a failure action or a graph bound — none of those has a shape in the vocabulary; claim
  containment (`contains_overrun` is derived from the stages and only reported); reach another
  call's graph (no verb carries a call identifier); or reach a supervised stage's parameters (§7.4
  frames `Hello`, `Frame` and `Result` and nothing else, so it is refused rather than dropped).

  **Provenance survives an app-assembled plan** because `crates/sipx-app/src/dsp.rs` reaches
  `GraphPlan::with_built_in` **once per stage** and never reaches `with_processor` — it cannot, the
  wire carries no processor. `dsp_sdk.rs`'s
  `naming_a_built_in_lends_an_application_s_processor_no_provenance` holds the other half: a plan
  mixing a registry stage with an application processor declaring `ProvenInline` is still refused
  `ProfileNotAdmissible` naming *that stage*.

  **Red before green** (`git merge-base main HEAD` = `413f19c`,
  `cargo test -p sipx-media --all-features --test dsp_sdk`): 25 compile errors, among them
  `no method named 'configure' found for struct 'DspGraph'`, `no variant named 'Configured' found
  for enum 'GraphTransition'`, `no function or associated item named 'registered' found for enum
  'BuiltIn'` and `no method named 'next_transitions'`. Green afterwards: 8 passed.

  **Owed to `CHANGELOG.md`** (fenced, so the coordinator writes it): *"Applications can compose and
  change a call's DSP chain over `sipx.app.v1`: three `dsp` verbs naming registered processors and
  closed parameter values, five `call.dsp.*` events, and sample-boundary parameter updates with one
  terminal outcome. An application can name a processor; it cannot supply one, declare a profile or
  claim containment."*

  **Left open.** `BypassCause::Requested` still has no producer — there is no verb that asks for a
  stage to be bypassed, and an application undoes a stage by replacing the chain. Filed as `M-115`.
  A supervised stage is unreachable from the wire at all (a program to spawn is host configuration,
  not something a document may name), so an application-assembled chain is always proven-inline
  today; that is stated in `call-dsp-graph.md` §10.1 rather than worked around.

- **2026-08-09 — merged `main` at `1a97409`, eight conflicts.** `M-68` rewrote the same file, and
  every conflict was a combination rather than a choice.

  **`Generation::run` carries both.** `M-68`'s signature won —
  `run(direction, samples, counters, journal)` — and `M-67`'s `report.bypassed` is gone, because
  `M-68`'s approach *subsumes* it: an `Option` on the frame's report holds one bypass, and a chain
  of three misbehaving stages spends three budgets on one frame, so the report would have silently
  kept the last. What `M-67` needed from that path was not the value but the **wake**, so the wake
  moved into `Journal::push`. Recording a transition and waking a reader is now one step, in one
  place, and the media worker gets it without knowing a reader exists. `Slot::record` was deleted:
  there is one journal door again, and all five writers — the activation, the in-`run` bypass, the
  teardown, the format-change teardown and §10.2's `Configured` — go through it.
  `dsp_sdk.rs::a_bypass_the_media_worker_journals_wakes_a_waiting_reader` is the new test for
  exactly this seam, and it hangs for the full bound and fails if the `stop()` is removed from
  `push`.

  **The other seven.** `Journal` keeps `M-67`'s coalescing paragraph and loses `#[derive(Debug)]`
  for `M-68`'s hand-written one, which now also renders `superseded` and carries an `#[expect]` for
  the omitted signal, worded as `Slot`'s is for `torn`. `Slot` takes `M-68`'s `sizing`/`counters`
  and `M-67`'s signal moved off it onto the journal. `Slot::retire` keeps `M-68`'s teardown and
  terminal-failure counters and pushes through the journal. `SlotRef` keeps both sides' new methods
  whole — `configure`/`next_transitions` beside `counters`/`re_anchor`/`reinstall`. `dsp/mod.rs` is
  the export union. `call-dsp-graph.md`'s header attributes both stories, and §10.3 now says the
  wake belongs to recording rather than to the callers that record.

  **`transitions_coalesced()` joins `GraphCounters`**, folded in from the journal exactly as
  `M-68` folds `transitions_dropped`, and documented as the figure read *beside* it and never added
  to it: coalesced entries are ones the application replaced itself.

  **Neither of `M-68`'s findings was reintroduced.** `check-audio-claims.py --check` and
  `check-dsp-heap.sh` are green; nothing added derives `Debug` through to a sample. `re_anchor` was
  not touched — it is on the `SlotRef` side of the union, taken whole from `main` — and
  `dsp_adversarial.rs`'s format-change and recovery tests pass unmodified.

  Verified after the merge: 54 test binaries green across the four crates, clippy `-D warnings`
  clean, `cargo fmt --all` clean, and the four checkers plus `sync-website --check`,
  `check-provenance.sh` and `check-story-closure.py`.

- 2026-08-10: closed at the `1.0.0-rc.18` boundary, against the wave gate run on this tree.
