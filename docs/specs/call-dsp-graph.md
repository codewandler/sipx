# The call-local DSP graph

**Status:** normative · **Epic:** `custom-call-dsp` · **Story:** `M-64` ·
**Design:** [custom-call-dsp](../designs/custom-call-dsp.md) · **Crate:** `sipx-media` (`dsp`)

[custom-call-dsp.md](custom-call-dsp.md) defines what one processor is and says, in its §1, that it
"deliberately does not define … the call-local graph, its ordering, its atomic replacement or its
teardown barrier (`M-64`)". This is that document. It defines how an ordered chain of processors is
attached to a running call, what a frame is guaranteed to see, what a late or failing processor
costs, and what teardown proves.

Where this document and an implementation disagree, this document is right until it is changed
deliberately. It **inherits and does not restate**: the processor interface, the capability
declaration, the parameter vocabulary, the refusal taxonomy, the three execution profiles and the
minimum failure policy are [custom-call-dsp.md](custom-call-dsp.md)'s, and the direction, frame
metadata, discontinuity vocabulary and tap points are [call-audio-seam.md](call-audio-seam.md)'s.
Nothing here weakens either. Where this document narrows one — and it narrows exactly two things,
§4.3 and §5.4 — it says so and says why.

## 1. Normative references

- [custom-call-dsp.md](custom-call-dsp.md) — the processor contract. Its §3.5 position table, §5
  capability, §6 sink and observation queue, §7 execution profiles and minimum failure policy, §8
  reset/flush/cancel/refusal and §9 bounds are inherited unchanged.
- [call-audio-seam.md](call-audio-seam.md) — `M-54`'s one call-media tap. Its §3 tap points and §7
  discontinuity kinds are inherited. **This document adds no second tap** (§2).
- [call-audio-processing.md](call-audio-processing.md) — the direction and discontinuity
  vocabulary both of the above already share.
- [media-runtime.md](media-runtime.md) §2.1 and §4 — worker ownership and shutdown. A graph's
  supervised workers are owned here and joined at §6's barrier.

No third-party implementation is referenced by this contract, its vectors or its rationale.

## 2. Where a graph runs, and why that is not a second tap

A graph runs **at `M-54`'s two tap points and nowhere else**:

- **Outbound** — in the send loop, after the mute gate and before encoding.
- **Inbound** — at the jitter buffer's output, after decode and before the frame reaches the
  application receive queue.

Those are the same two places [call-audio-seam.md](call-audio-seam.md) §3 already names, which is
what makes this an in-path transform at the existing tap rather than a second tap. The ordering at
each point is normative and is the whole of the difference:

```
outbound:  produce → mute gate → GRAPH → seam offer → encode → RTP
inbound:   RTP → decode → jitter → GRAPH → seam offer → application receive queue
```

The graph runs **before** the seam offer. The seam's own sentences are therefore still true rather
than newly false: the outbound tap keeps reporting "the samples that actually become RTP" and the
inbound tap keeps reporting "the same samples `MediaSession::recv` and `PcmCapture` see". A seam
consumer observes the call's audio as the call carries it, processed, and no consumer has to know
whether a graph is attached to be right about what it heard.

Two consequences follow and are normative. A relaying leg decodes nothing, so it has no `Inbound`
frames for a graph to transform. Telephone events are not audio and never reach a graph.

## 3. Attachment

One graph per direction per call. Two directions are two graphs: separate ordering, separate
generation, separate bounds, separate state. A second attachment to a direction that already has
one is refused `DirectionInUse` rather than silently replacing it — replacement is §5's operation
and has a generation, and an attach that quietly became one would produce a transition nobody
asked for.

Attaching to a stopped session is refused `SessionStopped`, matching
[call-audio-seam.md](call-audio-seam.md) §5 rather than minting a second answer for the same
situation.

### 3.1 Validation is whole, and precedes activation

A plan is an ordered list of stages and a `GraphBounds`. `validate` checks, in order: the bounds
themselves (§4), then every stage against the bounds and against
[custom-call-dsp.md](custom-call-dsp.md) §5's `validate()`, then the format against every stage's
declaration. **The first refusal refuses the whole plan**, nothing is prepared, nothing is
allocated, and the direction keeps running exactly as it was — with the previous graph in force if
there was one. A partially applied graph is the failure mode this ordering exists to make
unreachable.

Every buffer a graph will ever use is allocated at validation, from the declarations: the two
sample buffers it alternates between, the scratch region, the observation queue, and each
supervised stage's request and result channels. **The live path allocates nothing.**

### 3.2 Which profiles may be selected, and by whom

[custom-call-dsp.md](custom-call-dsp.md) §7.1: "`M-64` admits this profile only for processors
inside this workspace. An application-supplied processor may not declare itself proven." That is
implemented and not merely repeated:

| Door | Admits |
|---|---|
| the public plan builder | `SupervisedIsolated`, `TrustedCooperativeNative` |
| the crate-internal builder, reached only from a workspace processor registry | all three |

A stage reaching the public door with `ExecutionProfile::ProvenInline` is refused
`ProfileNotAdmissible` naming the processor and the profile. The registry is empty until `M-65`
ships processors to put in it, so today the refusal is total, which is the correct state of a
workspace that has proven nothing yet.

### 3.3 What attaching a graph promises, per profile

A graph is **only as contained as its least contained stage**. `contains_overrun()` on a graph is
the conjunction of `ExecutionProfile::contains_overrun()` over its stages, and it is not
configurable:

| Every stage is | The graph claims |
|---|---|
| `ProvenInline` and/or `SupervisedIsolated` | over-budget work in this graph cannot stall RTP |
| any stage `TrustedCooperativeNative` | nothing about containment |

One cooperative-native stage in an otherwise supervised chain makes the whole chain uncontained,
because it runs on the media worker and a callback that does not return stalls the media worker.
No configuration, measurement or conformance result changes this, exactly as
[custom-call-dsp.md](custom-call-dsp.md) §7.3 requires.

## 4. Bounds

Every bound is **explicit and non-zero**, and every one is a ceiling a configuration is refused for
exceeding rather than a hint.

| Bound | Domain | What it stops |
|---|---|---|
| `max_processors` | 1..=8 | an unbounded chain |
| `max_frame_samples` | 1..=65,536 | a frame larger than the contract's own §9.2 ceiling |
| `scratch_samples` | 1..=65,536 | a stage lending itself unbounded working memory |
| `retained_tail_positions` | 1..=65,536 | an unbounded delay line |
| `observation_capacity` | 1..=4,096 | an unbounded observation queue |
| `worker_queue_capacity` | 1..=64 | an unbounded request or result channel |

A bound outside its domain is refused `Bound { field, value }` before anything is sized by it. A
stage declaring more than a bound admits — a longer tail, more scratch, a larger frame — is refused
naming the stage, what it declared and what the bound was. `retained_tail_positions` bounds each
stage's declared `tail_positions` **and** its declared `latency_positions`, because both are audio
the graph holds and neither may be unbounded.

The upper ends are [custom-call-dsp.md](custom-call-dsp.md) §5's and
[call-audio-seam.md](call-audio-seam.md) §5's own, reused: 65,536 is the contract's frame, scratch
and latency ceiling, 4,096 is the seam's queue ceiling, and 8 is the seam's per-session attachment
ceiling. No number here is invented.

The graph's own two sample buffers are sized at validation from **the session's packetisation**,
which is what a frame on this call actually carries. A frame longer than that is passed through
untouched and the next frame carries a `Loss` (§6.2), because growing a buffer to fit it would make
the frame bound a suggestion. That is also what makes §6.3's "never allocates" exact rather than
nearly true: the graph keeps both of its buffers for its whole life and copies in and out of them,
rather than trading one with the caller's and inheriting whatever capacity that had.

### 4.3 The one narrowing: length

A live call's packetisation is fixed by its `Config`, so a stage that may change a frame's position
count cannot be attached frame-for-frame to a live call. A stage declaring
`LengthPolicy::Bounded` is therefore refused `LengthNotPreserving` naming it.

This narrows [custom-call-dsp.md](custom-call-dsp.md) §5 for the live path only. It does not change
the contract: a `Bounded` processor is still a valid processor, still runs under §11's harness, and
is still what an offline caller sizes a sink for with `max_output_positions`. What is not yet
defined is what a shortened frame means to the send loop's clock, and inventing an answer here
would be this epic quietly acquiring a re-framing stage nobody specified.

## 5. Generation, ordering and transitions

### 5.1 One frame, one generation

A frame is processed by one generation of one graph, entire. The graph's state is taken once per
frame and released once per frame; a replacement, bypass, removal or teardown is applied **between**
frames and never inside one. There is therefore no frame that saw stage 1 of generation *n* and
stage 2 of generation *n+1*, and no frame that saw one stage's old parameters and another's new
ones.

Generations are per direction and strictly increasing from 1. `0` is "no graph".

### 5.2 Ordering

Stages run in the order the plan declares them, index 0 first, on both directions. Stage *k*'s
output samples are stage *k+1*'s input samples. Position, direction and format are the same for
every stage of a frame, which is what §4.3's length narrowing buys.

### 5.3 Transitions are typed and name a position

Every change to what a frame will see is reported as a typed transition carrying the generation and
the **position** at which it took effect — the first position of the first frame the change applies
to, in the graph's current epoch:

```
GraphTransition ::= Activated { generation, at_position, processors }
                  | Replaced  { generation, previous, at_position, processors }
                  | Bypassed  { generation, at_position, processor, cause }
                  | TornDown  { generation, at_position, cause }

BypassCause     ::= Requested | Refused | DeadlineMissed | MalformedResult | WorkerLost
TeardownCause   ::= Requested | Detached | SessionStopped | FailedClosed { processor }
```

A transition queue is bounded by `observation_capacity` and drops its **oldest** entries at
capacity, counting them — a transition is a fact about the past, and the newest are the ones a
caller can still act on. This is the seam's own drop-oldest end
([call-audio-seam.md](call-audio-seam.md) §6.1) rather than a second policy.

### 5.4 Replacement, bypass and removal

`replace` validates the new plan entire (§3.1) and then, under the same take that a frame takes,
cancels every stage of the outgoing generation and installs the incoming one. Cancellation is
[custom-call-dsp.md](custom-call-dsp.md) §8.4's: terminal, idempotent, `retained() = 0`. A refused
replacement leaves the graph in force untouched and is the caller's error, not the call's.

A replacement opens a **new epoch**: the incoming stages are prepared, and the first frame they see
is at position 0. The outgoing generation's retained audio is **discarded, not flushed** — it
belongs to an epoch that no longer exists, which is §8.1's rule applied to a graph rather than to a
processor.

This is the second narrowing, and it is a narrowing of convenience rather than of contract: a
replacement could in principle flush the outgoing chain's tail into the incoming one's first frame.
It does not, because the outgoing tail is audio at old positions and there is no defined way to
place it in a new epoch. `flush` remains what an end of input deserves, and a graph replacement is
not an end of input.

## 6. Failure, and what the media worker never does

### 6.1 The minimum policy, implemented

[custom-call-dsp.md](custom-call-dsp.md) §7.4 is implemented exactly, and nothing is inferred:

- A stage that refuses a frame, misses its deadline, returns a malformed result or whose worker is
  lost has **missed**. Its consecutive-miss counter increments; any success resets it to zero.
- At `max_consecutive_misses` the stage has **failed**, and `on_failure` decides:
  - `BypassOpen` — the stage stops contributing. Its input passes through unchanged, the call keeps
    working and sounds wrong, and a `Bypassed` transition says so.
  - `TerminateClosed` — the whole graph is torn down under §7's barrier and audio for that
    direction does not flow. A `TornDown { FailedClosed }` transition says so.
- Below the threshold a miss costs one frame: that frame's stage contributes nothing and its input
  passes through, which is `BypassOpen`'s behaviour applied to one frame rather than to the stage.

`TerminateClosed` stopping the audio is the point of it: unprocessed audio leaving the stack is
worse than no audio leaving it, for the processor whose absence is a policy breach.

### 6.2 A miss reaches every downstream processor

When a stage contributes nothing to a frame — refused, missed, bypassed — the signal reaching
stage *k+1* is not the signal it has been filtering, so **the frame handed downstream carries a
discontinuity** and every stage after the affected one sees it. The kind is
[call-audio-seam.md](call-audio-seam.md) §7's, reused:

| Situation | Kind | Why that one |
|---|---|---|
| a supervised stage's result was abandoned at its deadline, or its channel dropped it | `Overflow` | §7: "the bounded-queue policy dropped queued frames". A bounded result channel is that queue |
| any other stage contributed nothing — a refusal, a bypass, a lost worker | `Loss` | §7: upstream audio never became a frame. The processed audio that stage owed never arrived |

Under [custom-call-dsp.md](custom-call-dsp.md) §3.5 a flagged frame at the expected position is
accepted and its reset runs before its own samples, so a downstream delay line discards a history
that is no longer about the signal in front of it. That is the whole reason the flag is carried
rather than the frame being passed on silently.

The session's own breaks reach a graph the same way: a decoded packet the seam counts as `Loss`
advances the graph's epoch by the lost span and flags the next frame, and a `Realign` restarts the
graph's epoch at position 0 with every stage reset.

### 6.3 What the media worker never does

Normatively, on the live path the media worker:

- never awaits, never blocks and never sleeps for a graph;
- never allocates for a graph (§3.1);
- never calls `process` for a `SupervisedIsolated` stage (§7);
- never reads a clock on a processor's behalf — a deadline is counted in frames, and a processor
  that could read its own deadline would have a clock
  ([custom-call-dsp.md](custom-call-dsp.md) §4.2, §7.4);
- never spawns a task or a thread; whatever a supervised stage owns is spawned at validation and
  joined at §7's barrier.

A `TrustedCooperativeNative` stage runs on the media worker and every one of those sentences is
still about the *runtime*: the callback itself is outside them, which is precisely why that profile
claims nothing (§3.3).

## 7. Supervised stages

A `SupervisedIsolated` stage's `process` is **never** called on the media worker. The stage owns:

- a **bounded request channel**, `worker_queue_capacity` deep, that the media worker offers to with
  a non-blocking send;
- a **bounded result channel**, the same depth, that the media worker takes from with a
  non-blocking receive;
- a **worker the runtime owns**, running the processor off the media path.

### 7.1 The deadline is a pipeline depth

`deadline_frames` is `d`. The media worker offers frame *n* and takes the result belonging to frame
*n − d*, so the worker has exactly `d` frame durations to answer and the media worker never waits
for it. The stage's output therefore lags its input by `d` frames, which is real latency and is
reported as such: a graph's `pipeline_frames()` is the sum of `d` over its supervised stages,
separate from `latency_positions()`, which is the sum of the stages' own declared
`latency_positions`. [custom-call-dsp.md](custom-call-dsp.md) §5's "`M-64` sums the declared
latency of a chain without simulating it" is that second sum.

While the pipeline fills — the first `d` frames after activation — there is no result to take. That
is a miss and is counted as one (§6.1); it is not a special case, because "no result by the
deadline" is the same fact whatever caused it.

### 7.2 Deadline, crash and malformed result

| The worker | The stage records |
|---|---|
| does not answer by the deadline | a miss; the result, if it ever arrives, is abandoned unread |
| answers with a position count other than the frame's | a miss, `MalformedResult` |
| fails terminally, panics, or its channel closes | a miss, `WorkerLost`; the worker is terminated and reaped at once and never restarted silently |

All three then follow §6.1: the declared `on_failure` applies at `max_consecutive_misses`. A worker
is never restarted behind the application's back — a processor that crashed is a processor whose
state is gone, and a silent restart would be a silent reset of audio state the application believes
is continuous.

### 7.3 Termination and reaping

Cancellation of a supervised stage is: close the request channel, signal the worker to stop, drain
the result channel, and **reap** — wait for the worker to have finished, not for a duration. The
wait is an event with a suspension point, so a stopped session's teardown answers rather than
holding a runtime worker nothing can reclaim.

## 8. Teardown and the barrier

Detach, cancellation, a fail-closed teardown, `stop()`, `shutdown()` and drop all reach the same
place, and it is **observable**:

```
GraphBarrier ::= { workers, frames_in_flight, retained_positions, processors }
```

The barrier is **clear** when all four are zero, and clear is what teardown waits for:

- `workers` — supervised workers still running. Zero means every one was terminated and reaped.
- `frames_in_flight` — frames offered to a request channel whose result has not been accounted for.
  Zero means no audio of this call is anywhere in the graph.
- `retained_positions` — the sum of `retained()` over the stages. Zero is
  [custom-call-dsp.md](custom-call-dsp.md) §8.4's guarantee, read back rather than assumed.
- `processors` — stages still installed. Zero means the generation is gone.

Nothing here may be observed by waiting a fixed duration. Teardown completes by reaching the
barrier and reports the barrier it reached, so a caller that saw a clear barrier knows the graph
holds nothing rather than believing it.

**Concurrent calls share no mutable DSP state.** A graph belongs to one direction of one call, its
processors are owned by it, its buffers are owned by it, and its workers are owned by it. Two calls
running the same processor kind run two instances. That is the property `M-66`'s adaptive state
depends on, and it is a consequence of ownership rather than a rule anyone has to observe.

## 9. Vectors

Unless a vector says otherwise it runs on `G8`: one `MediaSession` at G.711 µ-law, media clock
8,000 Hz, 20 ms packets (160 positions), one `Outbound` graph with default bounds, and the stages
named in [custom-call-dsp.md](custom-call-dsp.md) §12.1 reachable through the public plan builder.

| ID | Input | Expected |
|---|---|---|
| GRAPH-1 | an `Outbound` chain of gain-2 then bias-1, and an `Inbound` chain of bias-3 | the outbound tap reports `(n·2)+1` and never `(n+1)·2`; the application's received audio carries the inbound chain's output |
| GRAPH-2 | a plan whose second stage declares `ProvenInline` through the public door | refused `ProfileNotAdmissible`; nothing activates and outbound audio is unmodified |
| GRAPH-3 | `max_processors = 0`; `retained_tail_positions = 0` | each refused `Bound { field, value: 0 }` |
| GRAPH-4 | three stages against `max_processors = 2` | refused `TooManyProcessors { limit: 2 }` |
| GRAPH-5 | a stage declaring `tail_positions = 64` against `retained_tail_positions = 4` | refused `TailExceedsBound { declared: 64, bound: 4 }` |
| GRAPH-6 | activate bias-1, send a frame, `replace` with gain-2, send a frame | the first frame is `n+1`, the second is exactly `n·2`; transitions are `Activated { generation: 1 }` then `Replaced { generation: 2, previous: 1 }` |
| GRAPH-7 | a stage refusing every frame, `BypassOpen`, `max_consecutive_misses = 1`, followed by a second stage | RTP keeps flowing carrying unmodified audio; the second stage is handed a discontinuity; a `Bypassed { cause: Refused }` transition is reported |
| GRAPH-8 | the same stage with `TerminateClosed` | the graph tears down; the barrier is clear; a `TornDown { cause: FailedClosed }` transition is reported |
| GRAPH-9 | a supervised gain-2 with `deadline_frames = 1`; send two frames | the first output is the unprocessed input and a miss; the second is `n·2` |
| GRAPH-10 | a supervised worker that never answers, `max_consecutive_misses = 2`; send four frames | every output is the unprocessed input, RTP never stalls, and a `Bypassed { cause: DeadlineMissed }` transition is reported |
| GRAPH-11 | detach a graph holding one inline and one supervised stage | the barrier is clear: zero workers, zero frames in flight, zero retained positions, zero processors |
| GRAPH-12 | `shutdown()` a session with a graph attached, then attach again | the graph settles clear; the second attach is refused `SessionStopped` |
| GRAPH-13 | two sessions, each with a counting stage; drive only the first | the first stage saw every frame and the second saw none |
| GRAPH-14 | attach to a direction that already has a graph | refused `DirectionInUse` |
