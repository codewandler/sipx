# The call-local DSP graph

**Status:** normative · **Epic:** `custom-call-dsp` · **Stories:** `M-64`, `M-102` (§7's process),
`M-68` (§5.5, §6.4), `M-67` (§10's door) ·
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
Nothing here weakens either. Where this document narrows one — and it narrows exactly three things,
§4.3, §5.3 and §5.4 — it says so and says why. §10 narrows this document itself: it is the door an
**application** reaches a graph through, and everything an application may do is a subset of what a
Rust caller of this crate may.

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
`ProfileNotAdmissible` naming the processor and the profile. `M-65` filled the registry: the
built-ins reach the crate-internal builder through `GraphPlan::with_built_in`, and the same
processor handed to `with_processor` is still refused, because **provenance is a property of the
stage rather than of the plan**. A built-in beside an application's processor lends that processor
nothing.

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
GraphTransition ::= Activated  { generation, at_position, processors }
                  | Replaced   { generation, previous, at_position, processors }
                  | Configured { generation, at_position, processor }
                  | Bypassed   { generation, at_position, processor, cause }
                  | TornDown   { generation, at_position, cause }

BypassCause     ::= Requested | Refused | DeadlineMissed | MalformedResult | WorkerLost
TeardownCause   ::= Requested | Detached | SessionStopped | FailedClosed { processor }
```

`Configured` is §10.2's parameter update and is reported only for a set that applied **entire**: a
refused set changes nothing about what a frame will see, so there is nothing for it to report. It
carries the stage and the boundary and not the values — the values are the application's own, and
carrying them would make one entry's size depend on how many parameters a processor declares.

A transition queue is bounded by `observation_capacity` and drops its **oldest** entries at
capacity, counting them — a transition is a fact about the past, and the newest are the ones a
caller can still act on. This is the seam's own drop-oldest end
([call-audio-seam.md](call-audio-seam.md) §6.1) rather than a second policy.

**One narrowing of that policy, and only one: superseded parameter state coalesces.** When a
`Configured` is recorded for a stage of a generation, an earlier `Configured` for that same stage of
that same generation is removed and counted, because a stage has one current parameter state and the
earlier entry describes a value the application has itself replaced. Nothing else coalesces:
`Activated`, `Replaced`, `Bypassed` and `TornDown` are each a fact no later entry supersedes, and
each is the kind a caller cannot reconstruct from anything it holds. Without this narrowing an
application moving a parameter faster than it reads its events would push its own activation out of
a bounded queue with facts it had already overwritten — the queue's bound would be spent on the one
kind of entry that is safe to lose.

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

### 5.5 A renegotiation carries a graph over; a format change does not

A graph belongs to the **call**, not to a media worker generation, so an application does not
re-attach across a re-INVITE. A renegotiation re-anchors both directions: every stage is reset and
the epoch reopens at position 0, exactly as a `Realign` does (§6.2).

That holds only while the two things a chain was **validated against** are unchanged: the
`StreamFormat` every stage was prepared for, and the packetisation every buffer was sized from
(§4). A renegotiation that changes either **tears the graph down** and reports
`TornDown { cause: FormatChanged }`. Both halves of that are forced:

- Carrying the chain over would hand every stage frames at a rate it never accepted, under a
  declaration saying otherwise. [custom-call-dsp.md](custom-call-dsp.md) §8.3 makes a rate change a
  `prepare` and never a frame, and a stage prepared at 8,000 Hz cannot be told 8,000 Hz about
  16,000 Hz audio — it would refuse a truthful frame and silently mis-filter an untruthful one.
- Re-preparing the chain in place is not available either. A supervised stage's worker was sent a
  `Hello` naming the old format before its first frame (§7.4) and §7.2 forbids respawning it
  silently; an inline stage's audio state belongs to an epoch at the old rate. Re-preparing would
  be a reset the application did not ask for, dressed as continuity.

Nothing is re-attached on the application's behalf. A graph for the format the call is now carrying
is the application's to plan and attach, exactly as a replacement is (§5.4), and a handle that
survives the teardown validates its next `replace` against **the session's** current format rather
than the one the handle was created for.

`M-68` found this: before it, a renegotiation that changed the codec's audio rate or the
packetisation carried the chain over, and a frame longer than the sized buffer then passed through
untouched for the rest of the call under §4's pass-through rule — so a `TerminateClosed` stage whose
absence is a policy breach stopped running with nothing saying so.

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

### 6.4 Counters, and what is not one

§5.3's transitions are the detail — which stage, which position, which generation — and their queue
is bounded and drops its oldest at capacity. A **counter** is what survives that. Every direction
carries a cumulative tally, per call and across every generation of its graph, of what the runtime
observed: frames seen, deadline misses, refusals, malformed results, lost workers, bypasses, resets,
teardowns, terminal failures, frames passed through untouched for exceeding §4's sized frame, and
transitions the queue dropped. A replacement does not reset them and neither does a teardown — after
a fail-closed teardown the graph is gone and the counters are what is left to say why.

**Nothing a processor observed about a frame is a counter.** `DspObservation` is
[custom-call-dsp.md](custom-call-dsp.md) §6's vocabulary and belongs to the processor; a counter here
belongs to the runtime. That document's §6 states the reason and this is its other half: an
intentional glitch effect emits `Saturated` about audio it meant to clip, an overloaded processor
moves a counter, and **no door of this stack may present the second as the first**. There is
therefore no counter a processor can move by emitting an observation, and no observation the runtime
can synthesise from a counter.

The four counters that correspond to §6.1's *miss* — deadline misses, refusals, malformed results
and lost workers — are exactly the four things that increment a stage's consecutive-miss budget, and
nothing else. A frame passed through for exceeding §4's bound is **not** one of them: no stage was
late and no stage refused, the frame was never offered to the chain at all, and counting it as a
miss would fail a processor for a producer's mistake.

## 7. Supervised stages

A `SupervisedIsolated` stage's `process` is **never** called on the media worker. The stage owns:

- a **bounded request channel**, `worker_queue_capacity` deep, that the media worker offers to with
  a non-blocking send;
- a **bounded result channel**, the same depth, that the media worker takes from with a
  non-blocking receive;
- a **worker process the runtime spawns and owns**, running the processor in an operating-system
  process of its own, reached over §7.4's framed protocol.

The worker is a process and not a thread, and that is the whole of what
[custom-call-dsp.md](custom-call-dsp.md) §7.2's two operating-system sentences rest on: a worker's
memory and CPU are bounded by what the operating system is configured to bound, and a worker that
will not stop can be terminated regardless of what it is doing. A thread can be neither bounded nor
killed, so a thread-hosted worker cannot carry that profile's claim (`M-102`).

Nothing between the media worker and the process runs on the media worker. The runtime owns a
**pump**, off the media path, which is the only thing that touches the pipes: it takes a request
from the request channel, writes one `Frame`, reads one `Result`, and offers it to the result
channel with a non-blocking send. Spawning happens once, at validation, and never on the live path
(§3.1).

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
| answers with a message §7.4 refuses | a miss, `MalformedResult`; the pump stops and the worker is lost |
| exits, is killed, crashes, or closes its output | a miss, `WorkerLost`; the worker is terminated and reaped at once and never restarted silently |

All four then follow §6.1: the declared `on_failure` applies at `max_consecutive_misses`. A worker
is never restarted behind the application's back — a processor that crashed is a processor whose
state is gone, and a silent restart would be a silent reset of audio state the application believes
is continuous. **A worker process is never respawned**, for the same reason and one more: a
respawned process is a new process with none of the state the previous one had built, and an
application that wants one asks for one by replacing the graph (§5.4).

### 7.3 Termination and reaping

Cancellation of a supervised stage is: close the request channel, signal the worker to stop, drain
the result channel, and **reap** — wait for the worker to have finished, not for a duration. The
wait is an event with a suspension point, so a stopped session's teardown answers rather than
holding a runtime worker nothing can reclaim.

For a process that is four steps, and the order matters:

1. **Close the request channel.** The media worker's side is done; nothing further is offered.
2. **Signal the worker to stop.** The signal is *end of file on the worker's standard input*, which
   the pump produces by dropping its end of the pipe. It cannot be missed and it needs no
   cooperation to deliver.
3. **Terminate.** The runtime kills the process. This does not wait for the worker to agree and is
   not conditional on it having failed: a worker that is stuck inside one `process` call never sees
   step 2, and step 3 is what makes "a worker that will not stop can be terminated regardless" a
   fact rather than a hope. There is no grace period, because a grace period is a wall-clock
   duration standing in for a happens-before relation, and because a cancelled stage's retained
   audio is discarded rather than flushed ([custom-call-dsp.md](custom-call-dsp.md) §8.4) — a
   worker has nothing to finish that anyone will read.
4. **Reap.** `wait` for the process, so the operating system's entry for it is gone. §8's barrier
   reports `workers: 0` **only once `wait` has returned**, never merely once the kill was sent: a
   zombie is a worker that still exists.

**When the runtime is gone, the worker still ends.** If the media process dies without running any
of the four steps — `SIGKILL`, a crash, an operator's interrupt — the worker's standard input
closes because the only writer is gone, and step 2 is delivered by the operating system rather than
by us. A conforming worker MUST exit at that end of file **whatever its processing is doing** (§7.4),
which is why a worker keeps reading on a different thread from the one it processes on. A worker
that is stuck and orphaned is the one case the runtime cannot reach, and it is the worker's own
conformance requirement rather than a claim made on its behalf.

### 7.4 The worker protocol

A supervised worker is a **program**. The runtime spawns it, writes to its standard input, reads
from its standard output, and agrees nothing else about it: it may be the application's own
executable re-invoked with an argument, a separate program, or §7.5's reference worker. Standard
error is the worker's, untouched, so a worker can report to the operator without corrupting the
stream. The protocol is **lockstep** — one `Frame` written, one `Result` read — so neither pipe ever
holds more than one message and neither side can deadlock the other by filling one.

**Framing.** Every message is a fixed 8-octet header followed by exactly `length` octets of payload.
Integers are unsigned and big-endian; samples are signed 16-bit two's complement, big-endian, and
interleaved by channel exactly as [custom-call-dsp.md](custom-call-dsp.md) §3.2 defines them.

| Offset | Size | Field |
|---|---|---|
| 0 | 2 | magic, `0x5344` |
| 2 | 1 | type |
| 3 | 1 | reserved, MUST be zero |
| 4 | 4 | `length`, the payload's size in octets |

| Type | Direction | Fixed payload | Trailing |
|---|---|---|---|
| `0x01` `Hello` | runtime → worker | version `u16`, direction `u8`, sample rate `u32`, channels `u8`, `max_samples` `u32` — 12 octets | none |
| `0x02` `Frame` | runtime → worker | sequence `u64`, position `u64`, discontinuity `u8`, `samples` `u32` — 21 octets | `samples` × 2 octets |
| `0x81` `Result` | worker → runtime | sequence `u64`, outcome `u8`, `samples` `u32` — 13 octets | `samples` × 2 octets |

`direction` is `1` inbound and `2` outbound. `discontinuity` is `0` none, `1` loss, `2` overflow and
`3` realign — [call-audio-seam.md](call-audio-seam.md) §7's kinds, numbered here and not renamed.
`outcome` is `0` produced, `1` withheld and `2` failed, which are §7.2's three answers. `samples`
counts interleaved 16-bit samples and not positions; at `channels` channels a frame of *n* positions
carries *n · channels* of them.

`Hello` is sent once, before any frame, and carries the direction, the format and the ceiling every
later message is bounded by. There is no reply to it: a handshake the runtime waited for would be a
wait on application code before the call could proceed, which is the one thing this profile exists to
avoid. A worker that cannot accept the `Hello` exits, and the runtime learns that as §7.2's
`WorkerLost` like any other ending.

**A message is refused by its type, never by its length prefix.** The type determines the arithmetic
and the length prefix is checked against it — the reverse, sizing a read or an allocation from a
number the peer chose, is how a malformed message becomes a memory fault. In order:

| Refusal | When |
|---|---|
| `BadMagic` | the first two octets are not `0x5344` |
| `Reserved` | the reserved octet is not zero |
| `UnknownType` | the type octet is not one this version defines |
| `Undersized` | `length` is below the type's fixed payload |
| `Oversized` | `length` exceeds the type's fixed payload plus `2 × max_samples` |
| `LengthMismatch` | the payload's own `samples` field does not account for `length` exactly |
| `Truncated` | the stream ended inside a message |

`Oversized` is decided from the header alone, before a single octet of payload is read, so no
declared length ever sizes a buffer. `LengthMismatch` is decided after the fixed payload is read out
of a buffer already bounded by the previous rule. A refused message is terminal for the connection
in both directions: the runtime stops the pump and records `MalformedResult` (§7.2), and a worker
stops reading and exits.

**The ceiling is bounded too** (`M-122`). `max_samples` is the number every later length is checked
against, and on the worker's side it is declared by whatever wrote to its standard input — so a
`Hello` carrying a `max_samples` above 65,536, which is §4's own `max_frame_samples` ceiling and
[custom-call-dsp.md](custom-call-dsp.md) §5's, is refused as a `Value` this version does not define,
exactly as an undefined `direction` is. Without that bound the rule above
says nothing on that side: the peer picks the ceiling, so the peer picks the size of the next read,
and at the top of the range the arithmetic that compares a payload's own count against its declared
length saturates and stops deciding anything. A ceiling above that bound could never carry a frame a
stage would accept, because a frame larger than it is refused by the processor contract before any
stage is offered it.

The runtime's `max_samples` is the frame length its buffers were sized for (§4.3). A `Result` whose
`samples` is not the offered frame's own count is **not** a wire refusal — it is well-formed and
wrong, which is §7.2's `MalformedResult` at the contract level.

**What a conforming worker must do.**

1. Read `Hello` first and refuse to run at a `version` it does not implement.
2. Answer every `Frame` with exactly one `Result` carrying that frame's `sequence`, in order.
3. **Exit at end of file on its input, whatever its processing is doing.** This is the requirement
   §7.3 leans on for an orphaned worker, and it is why a worker reads on a different thread from the
   one it processes on: a worker that only notices end of file between frames does not notice it at
   all once a frame has hung.
4. Never write anything but `Result` messages to its standard output.

### 7.5 The reference worker

`sipx-media` ships one conforming worker, `sipx-dsp-worker`, so that §7.4 has a runnable peer rather
than only a written one — [vision.md](../vision.md) principle 6, applied to a protocol whose failure
modes are the point. It applies one declared transform, and its remaining modes exist to produce the
four endings §7.2 tabulates on demand: withhold every frame, hang inside one, answer with the wrong
sample count, and exit mid-call. It is what this document's §9 vectors drive, and an application can
drive it from a shell to check its own supervision before writing a worker of its own.

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
| GRAPH-14 | a supervised gain-2 worker process; drive frames and read the pid it reports | the pid is not the media process's own, and the process is alive while the stage is |
| GRAPH-15 | the same stage, detached | the barrier is clear and `wait` has returned: the pid is no longer a process this runtime owns, reaped rather than merely killed |
| GRAPH-16 | a worker process killed from outside mid-call, `BypassOpen` | `WorkerLost`, the declared action, RTP still flowing, and no respawn |
| GRAPH-17 | a worker process that hangs inside one frame, then detach | every frame after it misses, RTP never stalls, and the detach's barrier is still clear — the reap did not wait for the worker to agree |
| GRAPH-18 | a worker process answering with one sample fewer than the frame carried | `MalformedResult`, not audio — well formed on the wire and wrong at the contract |
| GRAPH-19 | a `Result` header declaring a length above `2 × max_samples` | refused `Oversized` from the header alone; no payload is read and nothing of that size is allocated |
| GRAPH-14 | attach to a direction that already has a graph | refused `DirectionInUse` |

## 10. The application door (`M-67`)

§3.2 settled *which profiles may be selected, and by whom*. This section settles the door an
**application** reaches them through: what it may name, what it may move while a call is running,
and what it is structurally unable to say. Everything here is a narrowing of what a Rust caller of
this crate can already do — an application gets strictly less, and the difference is the point.

### 10.1 An application names identifiers, never code

The registry of §3.2 is published: every entry's stable identifier and, per entry, the **closed
parameter schema** the processor itself declares. Discovery is that list, and it is read out of the
declarations rather than tabulated beside them, so what an application discovers and what validation
refuses it against cannot disagree.

An application composes a chain by naming those identifiers in order, on one direction, each with a
finite parameter set from [custom-call-dsp.md](custom-call-dsp.md) §3.6's vocabulary. Normatively:

| The application supplies | The stack derives |
|---|---|
| an ordered list of registered identifiers | each stage's processor, its capability and its profile |
| a finite parameter set per stage | validation against that stage's declared schema |
| a shape, where a processor declares one | the buffers the graph is sized by, at validation |
| a direction | which of §2's two tap points the chain runs at |

and nothing else. There is **no shape in this vocabulary** for supplying a processor, a program, a
callback, an execution profile, a deadline, a failure action or a graph bound. That is not a rule an
implementation applies; it is the absence of a way to say those things, in the same sense that
[app-contract.md](app-contract.md) §6.5's `play.source` has no URL variant.

Two consequences are normative and are why the vocabulary is this narrow.

**Provenance stays a property of the stage.** An application naming a built-in reaches the registry
door for *that stage*, and §3.2's asymmetry is untouched: a chain mixing a registry stage with a
processor offered at the public door still refuses the second one's `ProvenInline`, and the refusal
names the stage. A built-in beside another processor lends it nothing — through this door as through
any other.

**An application cannot assemble a claim.** §3.3's `contains_overrun()` is the conjunction over the
stages' profiles, derived from what the stages *are*. It is reported to an application and never
accepted from one: there is no field, parameter, bound or identifier through which an application
states it, and no combination of registered processors makes an uncontained chain claim
containment or the reverse. An application that requires the stack to contain a stall selects a
supervised stage, which is a **host** capability behind the host's own configuration — a program to
spawn is not something a wire vocabulary may name, for [app-contract.md](app-contract.md) §6.5's
reason.

### 10.2 Parameters move at a declared boundary, with one terminal outcome

A live stage's parameters are moved by naming three things: the **generation** the set was composed
against, the stage's **index** in that generation's plan order, and the set itself.

- **Validated whole, off the media path.** The set is checked against the stage's declared schema on
  the caller's own thread, before anything is assigned. The media worker validates nothing.
- **Applied at a position boundary.** The assignment happens under the same take a frame needs, so
  the values are in force from the first position of the next frame and §5.1 still holds: no frame
  sees one stage's old parameters beside another's new ones, and no frame sees half a set.
- **Exactly one terminal outcome.** Either a record of the update — its generation, its stage and
  the position it took effect at — or a refusal, and **every refusal leaves the live graph exactly
  as it was**: not one value, not the position expectation, not the generation. This is
  [custom-call-dsp.md](custom-call-dsp.md) §3.6's "a set is applied at one position boundary or not
  at all", read at the graph.

Four refusals, and each of them changes nothing:

| Refusal | When |
|---|---|
| `StaleGeneration { expected, live }` | the named generation is not the live one, `0` being a direction with no graph |
| `UnknownProcessor { index, processors }` | the live chain has no stage at that index |
| `Parameter { processor, source }` | an identifier outside the declared schema, a value of the wrong kind, or a value outside its declared range — the **whole** set refused, the previous one in force |
| `NotConfigurable { processor }` | a supervised stage: §7.4 frames `Hello`, `Frame` and `Result` and nothing else, so there is no message to carry a parameter set |

The generation is what makes an update unambiguous rather than merely correlated. Stage 2 of one
generation is a different processor from stage 2 of the next, so an update composed against the
chain an application last saw must not land on the chain that replaced it — and under §5.4 a
replacement is exactly the operation that can have happened in between.

**Adding, removing or reordering a stage is not a parameter update.** It is §5.4's replacement, it
has a generation of its own, and it opens a new epoch. A door that let a parameter set change the
shape of a chain would be a replacement without one.

### 10.3 Transitions reach an application without polling and without reaching the worker

§5.3's queue is what an application reads, and reading it must cost the call nothing. Normatively, a
driver turning transitions into application events:

- **never polls, and never times a look.** The drain has a suspension point: a waiting reader is
  woken where a transition is recorded, and the signal is installed under the same take that found
  the queue empty, so a transition recorded between the two cannot be missed. No fixed duration is
  part of this. The wake belongs to **recording**, not to the callers that record: §6.1's bypass is
  journalled by the media worker mid-frame and §5.5's format-change teardown by a renegotiation, and
  neither knows a reader exists. An implementation that made waking a caller's responsibility would
  be one story away from a transition that never wakes anybody.
- **never runs on the media worker.** Recording a transition on the live path costs a flag store and
  a waker wake, which is neither an await, a block nor an allocation (§6.3). Everything a reader
  then does with it happens on the reader's own thread.
- **never borrows an audio-thread object.** What crosses is §5.3's typed value: counts, positions,
  a stage identifier and a cause. No sample, no buffer, no processor and no handle to one.

### 10.4 Vectors

| ID | Input | Expected |
|---|---|---|
| GRAPH-20 | the published registry | every entry's identifier resolves back to that entry, an unregistered name resolves to nothing, and each entry's schema is the one validation holds a set against |
| GRAPH-21 | a live gain stage; a set inside its declared domain | applied at the next frame's first position; one `Configured` naming that generation, stage and position |
| GRAPH-22 | the same stage: a stage index the chain lacks, an identifier outside the schema, a value outside its range, and a generation that is not live | `UnknownProcessor`, `Parameter`, `Parameter`, `StaleGeneration` — each changing nothing and producing no `Configured` |
| GRAPH-23 | 32 parameter updates against a queue of 4 | the `Activated` survives and exactly one `Configured` remains — superseded parameter state gave way, not the activation |
| GRAPH-24 | a chain of registry stages, and the same chain with one cooperative-native stage | `contains_overrun()` true, then false; no parameter, identifier or built-in beside it changes either answer |
| GRAPH-25 | a plan mixing a registry stage with an application processor declaring `ProvenInline` | refused `ProfileNotAdmissible` naming *that stage*; the registry stage beside it lent it nothing |
