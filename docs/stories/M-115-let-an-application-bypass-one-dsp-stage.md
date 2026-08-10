---
id: M-115
title: Let an application bypass one DSP stage without replacing the chain
pillar: Media
status: in-progress
priority: 5
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [sipx-media, app-sdk, dsp, m18]
predicate:
announcement:
note: after M-67 · `BypassCause::Requested` is in the spec and nothing sets it
---

# Let an application bypass one DSP stage without replacing the chain

## Goal

Give `BypassCause::Requested` a producer, so that an application can take one stage out of a live
chain and put it back without replacing the whole chain and opening a new epoch.

## Why this is a story and not part of `M-67`

`docs/specs/call-dsp-graph.md` §5.3 has listed `BypassCause ::= Requested | …` since `M-64`, and
nothing in this workspace sets `Requested`: every bypass the runtime produces is a failure under
§6.1's miss budget. `M-67` built the application door — discovery, ordered chains, sample-boundary
parameters, typed events — and deliberately did **not** add a bypass verb, because its Acceptance
asks for bypass *events* rather than a bypass *request*, and a fourth verb would have widened a
surface that already carries three.

The gap is the same shape as `M-114`'s: a value the contract names with no producer behind it. It is
the defect `M-98`, `M-99` and `M-103` each closed once for the application contract, and it should
be closed here deliberately rather than left to be noticed.

The narrowing that makes this non-trivial: **a bypass is not currently reversible.** §6.1's bypass
is terminal for the stage — its input passes through for the rest of the generation, and an
application restores it by replacing the chain (`M-67` states this). A *requested* bypass an
application can undo is a second, reversible operation on the same word, and this story has to say
which one `Requested` means before it implements it.

## Acceptance

- [x] `docs/specs/call-dsp-graph.md` §6.1 says whether a requested bypass is reversible, and §10
      says how an application asks for one and what it costs the audio.
- [x] A live stage can be bypassed and reported `Bypassed { cause: Requested }` at a named position,
      under the same take a frame needs, with the frames after it carrying the discontinuity §6.2
      requires.
- [x] The request names a generation and a stage index and is refused — changing nothing — for a
      stale generation, an unknown index, or a stage already in that state.
- [x] `contains_overrun()` is unchanged by a bypass: containment is a property of which stages are
      installed, not of which are contributing.
- [ ] A verb and a completion event, if the application contract grows them, are held by
      `spec_tables.rs` to the same producer rule `M-67`'s five rows are.
- [ ] The full gate is green.

## Progress

- Backlog. Filed by `M-67` on 2026-08-09, which left the word without a producer on purpose.
- 2026-08-10 · **`Requested` is the reversible one, and it is the only reversible one.** The
  question this story said it had to answer first is answered in the spec before the code:
  `docs/specs/call-dsp-graph.md` §6.1 now carries a two-row table splitting `BypassCause` by *who
  imposed the bypass*. The four the runtime imposes stay terminal for the stage — the evidence
  behind them is a run of frames the stage could not keep up with, and nothing since is evidence to
  the contrary — and §5.4's replacement is still what lifts them. `Requested` is the application's
  own and is reversible through the same door. The asymmetry is the point: without it, an
  application could hold a failing stage on the media path indefinitely by restoring it after every
  failure, which is `BypassOpen` defeated by the control surface meant to complement it.
- **The door is `M-67`'s door, not a second one.** `DspGraph::set_bypassed(generation, processor,
  bypassed)` mirrors `configure` deliberately: same generation-and-index check, same single take a
  frame needs, one terminal outcome (`BypassUpdate`, on `ParameterUpdate`'s pattern), and every
  refusal leaving the live graph untouched. It takes the state asked for rather than toggling, so a
  request composed against a stage the application has since moved is refused instead of landing as
  its own opposite.
- **Failing-first.** Test written and run at the merge base `c43ebde`, in this worktree with its own
  `CARGO_TARGET_DIR`:

  ```
  $ cargo test -p sipx-media --test dsp_graph --all-features
  error[E0599]: no method named `set_bypassed` found for struct `DspGraph` in the current scope
  error[E0599]: no variant named `Restored` found for enum `GraphTransition`
  error[E0599]: no variant named `BypassUnchanged` found for enum `GraphError`
  error[E0599]: no variant named `NotBypassable` found for enum `GraphError`
  error[E0599]: no variant named `BypassNotReversible` found for enum `GraphError`
  error: could not compile `sipx-media` (test "dsp_graph") due to 15 previous errors
  ```

  Green after: `dsp_graph` 17 passed, `dsp_adversarial` 8 passed, `dsp_activity_wiring` 5 passed;
  `dsp_sdk`, `dsp_worker` and `dsp_noise_reduction` unchanged and passing beside them.
- **What a restore costs the audio**, and the part that was not free. A restored stage's first frame
  back carries §6.2's `Loss`, because the signal in front of it skipped the whole span it was out
  for. That is a per-stage debt (`Stage::resumed`) rather than a flag on the generation: a restore
  at index *k* changes the signal for *k* onward and for nothing before it, so putting it on
  `Generation::pending` would have flagged stages 0..*k* that saw no break at all. §6.2's table
  gained the row and says why.
- **Row 5 is deliberately not ticked, and the application contract deliberately did not grow.** The
  wire vocabulary already maps `BypassCause::Requested` to `DspBypassCause::Requested`
  (`crates/sipx-app/src/dsp.rs:379`), so the *event* half has had a home since `M-67`; what is
  missing is a fourth `dsp` verb, and this story's own rationale is that a fourth verb widens a
  surface already carrying three. Nothing in `spec_tables.rs` changed because nothing it holds
  changed. `GraphTransition::Restored` is unmapped in `sipx-app` on purpose — `unsolicited`'s
  catch-all drops it — so an application driving the graph over the wire sees a requested bypass and
  not yet its counterpart. **That is the follow-up this story leaves behind**, and it is a story
  rather than a loose end: the verb, the `call.dsp.restored` event and the `spec_tables.rs` producer
  row belong together in one diff, reviewed against §6.2 of `app-contract.md`.
- **Not counted, and that is a decision.** A requested bypass does not move `GraphCounters::bypasses`
  — §6.4 now says why. That counter is what an operator reads to decide whether a processor fits
  this machine, and an application's own control traffic rising in it would make a healthy call look
  like a failing one. The boundary is reported twice already: as the returned `BypassUpdate` and as
  a journalled transition.
- **CHANGELOG sentence owed to the coordinator** (not written here — the ledger is fenced):
  `An application can now take one DSP stage out of a live chain and put it back with`
  `DspGraph::set_bypassed, without replacing the chain or opening a new epoch; a bypass the runtime`
  `imposed under the miss budget stays terminal.`
