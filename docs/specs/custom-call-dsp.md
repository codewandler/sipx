# The custom call-DSP contract

**Status:** normative · **Epic:** `custom-call-dsp` · **Contract story:** `M-63` ·
**Implementing stories:** `M-64` (graphs), `M-65` (effects), `M-66` (noise reduction),
`M-67` (SDK control), `M-68` (hardening), `X-109` (measurement), `A-34` (example) ·
**Design:** [custom-call-dsp](../designs/custom-call-dsp.md) ·
**Crates:** `sipx-audio` (`dsp`), `sipx-media`/`sipx-call` (attachment, per `M-54` and `M-64`)

Where this document and an implementation disagree, this document is right until it is changed
deliberately. It defines the one sans-I/O processor interface that built-in effects, noise
reduction and application-supplied DSP all implement, written before anything attaches DSP to a
live call. The point of writing it first is that a shared contract is only shared if nobody gets a
private version of it: an effect that reaches the media runtime through a door the application
cannot use is not the same contract, and the second time two processors disagree about what a
frame means is the first time nobody can say what a call sounded like.

It inherits rather than restates. Direction, discontinuity, the rate domain, the sample-position
timeline, the bounded-queue loss accounting and the reset-versus-refusal distinction are already
settled by [call-audio-processing.md](call-audio-processing.md) and
[call-audio-seam.md](call-audio-seam.md); this document reuses their types and their vocabulary and
mints nothing parallel. What is new here is what a processor may *change*, what it must *declare*
before it may run, and where its `process` call is allowed to execute.

## 1. Scope and shape

A processor is a synchronous frame transform and a pure state machine. Its complete input
vocabulary is: a validated capability declaration, a finite parameter set, a declared stream
format and direction, PCM frames with explicit metadata, and requested resets, flushes and
cancellations. Its complete output vocabulary is: samples written into a caller-owned sink, typed
observations enqueued into a caller-owned bounded queue, and typed errors.

It MUST NOT own or use a socket, a device, a device enumeration, a clock read, a random source, a
thread, or a background task, and it MUST NOT allocate the memory it works in — all of it is
supplied by the caller. Time exists only as the sample position and rate the caller states (§4).

This document defines the frame, the stream format and channel metadata, capability discovery, the
parameter vocabulary, the workspace and its bounds, the reset/flush/cancel/refusal taxonomy, the
three execution profiles and the minimum failure policy, and the conformance harness with its
sample vectors.

It deliberately does not define: any effect, filter or noise-reduction algorithm — the processors
this workspace ships under this contract are [call-dsp-effects.md](call-dsp-effects.md) (`M-65`) and
noise reduction is `M-66`;
the call-local graph, its ordering, its atomic replacement or its teardown barrier (`M-64`); the
SDK surface that controls a graph (`M-67`); measured CPU and quality thresholds (`M-68`, `X-109`);
or any new PCM representation or media tap — the boundary is
[linear-pcm.md](linear-pcm.md)'s and the tap is `M-54`'s (§10).

## 2. Normative references

- **RFC 3550** — RTP. The origin of the media timing this contract's sample positions descend from.
  A processor never sees a packet; RTP sequence numbers and timestamps stop at the seam.
- **RFC 3551** — the audio profile behind the linear PCM boundary this contract consumes.
- [linear-pcm.md](linear-pcm.md) — the owned linear PCM boundary: the supported rate domain
  1..=384,000 Hz and the typed `UnsupportedSampleRate` refusal, **reused and not re-minted**.
- [call-audio-seam.md](call-audio-seam.md) — `M-54`'s one bounded per-call PCM seam. Its §3
  direction vocabulary, §4 frame metadata and epoch rule, §6 bounded-queue policy and §7
  discontinuity kinds are inherited here.
- [call-audio-processing.md](call-audio-processing.md) — the deterministic analysis contract. Its
  §3.1 direction, §3.3 discontinuity vocabulary, §4 determinism obligation, §7 reset-versus-refusal
  distinction and §8.3 coalescing loss accounting are inherited here. A DSP processor and an
  analyser observing the same call agree about what a frame is because they are handed the same
  types.
- [media-runtime.md](media-runtime.md) §2.1 and §4 — worker ownership and shutdown. A processor
  owns no worker; `M-64` owns whatever the supervised profile spawns (§7.3), and that is where this
  contract's execution profiles are implemented.

No third-party implementation is referenced by this contract, its vectors or its rationale.

## 3. The input contract

### 3.1 Direction

```
AudioDirection ::= Inbound | Outbound
```

This is [call-audio-processing.md](call-audio-processing.md) §3.1's type, imported. A processor
instance is bound to exactly one direction at `prepare`, and a frame carrying the other direction
is refused (§8.3). Per-direction state never interleaves, which is what makes every vector in §12 a
single-stream replay and what lets `M-64` attach independent chains to the two paths.

### 3.2 Stream format and channels

```
StreamFormat ::= { sample_rate: 1..=384,000,
                   channels:    1..=8 }
```

The sample representation is **signed 16-bit linear PCM and nothing else**. The seam offers
unsigned 8-bit as well ([call-audio-seam.md](call-audio-seam.md) §5); a processor never sees it,
because a second depth in the sample path is a second arithmetic, a second saturation boundary and
a second set of vectors for every effect. Depth conversion happens at the attachment, before this
contract begins.

The rate domain and its refusal type are [linear-pcm.md](linear-pcm.md)'s, reused: rate 0 and rate
384,001 refuse with exactly the `UnsupportedSampleRate` that boundary's own PCM-4 vector names.

Multi-channel audio is **interleaved**, channel 0 first. The channel ceiling of 8 exists so that
one frame's interleaved sample count and one position's channel stride both stay inside the bounds
of §9 without a second cap; the call paths this epic serves are mono and stereo, and 8 is headroom
rather than a plan.

**Two units, named apart, because conflating them is how a stereo processor writes half a frame.**
A **position** is one sample per channel — the unit sample positions, latency, tail and length
policies are counted in. A **sample** is one `i16`. A frame of `n` positions on `c` channels
carries exactly `n · c` samples. A frame whose sample count is not a multiple of its channel count
is refused (§8.3).

### 3.3 The frame

```
DspFrame ::= { direction:     AudioDirection,
               format:        StreamFormat,
               position:      u64,                   -- first position, in this epoch
               discontinuity: Option<DiscontinuityKind>,
               samples:       &[i16] }                -- interleaved, borrowed

DiscontinuityKind ::= Loss | Overflow | Realign
```

`DiscontinuityKind` is [call-audio-processing.md](call-audio-processing.md) §3.3's type, imported,
and its three kinds mean exactly what [call-audio-seam.md](call-audio-seam.md) §7 says they mean.

Samples are **borrowed for the duration of the call** and MUST NOT be retained after `process`
returns. A processor that needs sample memory across frames — a delay line, a filter history — owns
that memory as declared state (§5) and copies into it; what it may not do is keep the caller's
buffer. Raw-audio non-retention is a design invariant, not an optimisation, and `M-68` proves it
against every profile.

### 3.4 Sample position is the only clock

`position` is the index of the frame's first position within the attachment's current **epoch**,
counted at the format's own rate. It is [call-audio-seam.md](call-audio-seam.md) §4's `sample_time`,
under the same epoch rule: the epoch opens at `prepare`, re-opens at every reset (§8.1) and at
every `Realign`, and advances across a gap by the span the discontinuity names, so the timeline
never compresses over loss.

A duration is converted to positions exactly once, by
[call-audio-processing.md](call-audio-processing.md) §4's formula:

```
positions(d_ms, rate) = ceil(d_ms · rate / 1000) = (d_ms · rate + 999) div 1000    -- in u64
```

There is no millisecond, no `Instant` and no wall clock anywhere inside a processor.

### 3.5 Position rules

For an accepted frame ending at position `e = position + positions`, the next frame is admitted by
this table and by nothing else:

| Arriving frame | Behaviour |
|---|---|
| the first of an epoch, `position = 0` | accepted; the epoch opens |
| the first of an epoch, `position ≠ 0` | refused `PositionNotContiguous` |
| unflagged, `position = e` | accepted; the stream continues |
| unflagged, `position ≠ e` | refused `PositionNotContiguous` — the seam always flags a gap, so an unflagged one is a broken caller and not a loss to smooth over |
| flagged `Loss` or `Overflow`, `position >= e` | accepted; the discontinuity reset (§8.1) runs before its samples are consumed, and the epoch continues at the stated position |
| flagged `Loss` or `Overflow`, `position < e` | refused `PositionNotContiguous`; positions are monotonic |
| flagged `Realign`, `position = 0` | accepted; the reset runs first and a new epoch opens |
| flagged `Realign`, `position ≠ 0` | refused `PositionNotContiguous` — a `Realign` restarts the epoch, which is what the seam already says it does |

The flag is authoritative: a discontinuity is what the seam says happened, never what a processor
infers from a position gap.

### 3.6 Parameters are finite by construction

```
ParameterValue  ::= Flag(bool) | Integer(i64) | Ratio(i32)      -- Ratio is thousandths
ParameterDomain ::= Flag | Integer { min, max } | Ratio { min, max }
ParameterSpec   ::= { id: &'static str, domain: ParameterDomain }
Parameter       ::= { id: &'static str, value: ParameterValue }
```

There is no floating-point parameter, and that is the whole point: "finite parameter state" is a
property this vocabulary cannot express the negation of. A NaN gain, an infinite cutoff and a
signalling payload smuggled through a `f64` bit pattern are not refused at a boundary here — they
are unrepresentable. `Ratio` is thousandths of a unit, which is 0.001 resolution on every ratio and
0.001 dB on every level `M-65` and `M-66` will declare.

`configure` validates the **whole** set against the declared specs before it assigns anything: an
unknown `id`, a value of the wrong kind, or a value outside its declared range refuses the set
entire and leaves the previous parameter state in force, untouched (§8.4). Ordering inside one set
is the caller's; a set is applied at one position boundary or not at all, which is the guarantee
`M-67` builds its generation and correlation identity on.

## 4. Determinism

Everything a processor writes is a pure function of (capability, parameters, declared format and
direction, ordered inputs). Normatively:

1. **No I/O.** No socket, no file, no device, no device *enumeration*. A processor that wants to
   know whether a machine has a particular instruction set asks its declared parameters, not the
   machine.
2. **No clock read.** Not `Instant::now`, not a monotonic counter, not a cycle counter. The only
   time is §3.4's sample position and the declared rate.
3. **No task spawn and no thread.** A processor is called and returns. Whatever the supervised
   profile runs (§7.3) is spawned and owned by `M-64`, outside this interface.
4. **No random source, no global mutable state, no interior mutability shared between instances.**
   Two calls sharing one adaptive state is the defect `M-66` is required to prove absent.
5. **Integer arithmetic is two's-complement at stated widths, and never wraps silently.**
   Saturation is a declared behaviour with an observation (§6), not an accident.
6. **Floating point, where a processor uses it at all, is restricted to the exactly-rounded IEEE
   754 operations**: addition, subtraction, multiplication, division, square root and fused
   multiply-add, plus comparisons and explicit conversions. A transcendental function — `sin`,
   `exp`, `pow`, `log`, or any approximation of one — MUST NOT appear in the sample path or in
   coefficient derivation at run time, because its result is not specified to the last bit and a
   fixture computed on one machine then stops being evidence on another. A processor needing such a
   coefficient derives it from its declared parameters through a table it ships.
7. **Scratch contents on entry are unspecified.** A processor MUST write a scratch region before it
   reads it. The conformance harness fills the workspace with a different pattern on each run
   (§11.2), so a processor that reads unwritten scratch is a function of the caller's leftovers and
   fails determinism rather than passing by luck.

Two processors built from the same capability, given the same parameters, format and direction, and
fed the same frames, MUST produce identical output samples and identical observation sequences on
every architecture and platform, **and the same vectors MUST hold under virtual and wall time
alike** — there is nothing in a processor for wall time to change, which is what makes that
sentence a consequence rather than a promise.

## 5. Capability discovery

A processor declares, before it may be prepared or attached:

| Field | Domain | What it promises |
|---|---|---|
| `id` | non-empty `&'static str` | the stable identifier `M-67` registers and the SDK selects |
| `rates` | `Any`, or an ascending non-empty list inside 1..=384,000 | which rates it accepts; anything else refuses at `prepare` |
| `channels` | ascending non-empty list inside 1..=8 | which channel counts it accepts |
| `max_frame_samples` | 1..=65,536 | the largest interleaved frame it will consume; the §9.2 CPU ceiling |
| `scratch_samples` | 0..=65,536 | the caller-owned scratch it needs, in `i16`s. The caller supplies exactly this much |
| `state_bytes` | any `u64` | an upper bound on the memory the processor itself owns (§9.1) |
| `latency_positions` | 0..=65,536 | its algorithmic delay: the position offset between an input and the output that carries it |
| `tail_positions` | 0..=65,536 | how many positions remain retained when input stops, and `flush` may produce |
| `length` | `Preserving`, or `Bounded { max_positions }` | whether one frame's output has exactly its input's position count |
| `reset` | `Stateless` or `ClearsState` | whether a reset is observable at all |
| `execution` | an `ExecutionPolicy` (§7) | where it may run and what happens when it is late |
| `parameters` | 0..=32 specs with unique ids | the closed schema `M-67` exposes |

A declaration that is not internally admissible — an empty rate or channel list, a channel above 8,
a frame above 65,536, a duplicate parameter id, a deadline of 0 frames — is refused by
`validate()` as a typed `CapabilityError` **before any caller sizes a buffer from it**. A capability
is read by the attachment (`M-64`) to allocate every queue, sink and scratch region a call will
ever use, so a capability that lies about its shape is a capability that mis-sizes memory, and it
has to be caught at the declaration and not at the first frame.

`max_output_positions(input_positions)` derives the sink size a caller must provide:
`input_positions` under `Preserving`, `max_positions` under `Bounded`. This is the *only* sanctioned
way to size a sink, so that a processor cannot get more output room by being asked politely.

**Latency and tail are declarations about content, not about count.** A `Preserving` processor with
one position of latency still writes exactly as many positions as it consumed; what is delayed is
which input each output carries. That is what makes latency composable in an ordered graph:
`M-64` sums the declared latency of a chain without simulating it.

## 6. Output shape and typed observations

`process` writes into a caller-owned **sink** and enqueues into a caller-owned bounded
**observation queue**.

```
DspObservation ::= Saturated { positions }
                 | PassedThrough
                 | ParameterApplied { parameter, at_position }
                 | Restarted { cause }
                 | Lost { count }
```

The sink refuses to grow: a write past its capacity is `OutputOverflow` and the already-written
prefix is untouched. The observation queue is [call-audio-processing.md](call-audio-processing.md)
§8.3's, applied unchanged — at capacity the newest retained entry is coalesced into `Lost { count }`
rather than blocking or growing, so an undersized queue is a visible counted fact rather than a
silent absence. `Lost` is deterministic like everything else: the same input against the same
capacity loses the same observations.

Observations are facts about audio, and the vocabulary above is deliberately small. Deadline
misses, bypasses, terminal failures and graph transitions are **not** here: they are things the
runtime observed about a processor, not things a processor observed about a frame, and they belong
to `M-64`'s events and `M-68`'s counters. This split is load-bearing for one of `M-65`'s
obligations — an intentional glitch effect emits its own observation, an overload defect emits a
runtime counter, and no door of this stack may present the second as the first.

## 7. Execution profiles

This is the part of the contract `M-64` implements and `M-68` proves, and the part whose wording is
a promise to an application about its call quality. Three profiles exist. **Only the first two may
claim that over-budget work cannot stall RTP**, and the third says so about itself in its own
documentation, its own events and here.

The claim decomposes into two independent facts, kept separate because they are separately true:

```
OverrunContainment ::= BoundedWork | BoundedWait | None
DeadlineAction     ::= AbandonResult | AccountAfterReturn
```

### 7.1 Proven inline — `BoundedWork`, `AccountAfterReturn`

Crate-owned processors that have passed §11's harness and whose per-frame work is bounded by
construction: no allocation after `prepare`, `O(positions)` with a fixed per-position step, no loop
whose trip count depends on a sample value, no blocking call, and a declared `max_frame_samples`
ceiling. They run **on the media worker**, synchronously.

- **What it claims.** Over-budget work cannot stall RTP, because there is no unbounded work to go
  over budget with. The containment is in the *shape of the work*, and §11's harness plus `M-68`'s
  budgets are what make that shape checkable rather than asserted.
- **What it does not claim.** Nothing preempts the call, because nothing can: a synchronous call on
  the media worker runs to completion. The deadline action is therefore `AccountAfterReturn` — a
  miss is measured once the call has returned, counted, and applied to the *next* frame's admission
  under §7.4. On a machine too slow to run the configured graph at all, every profile misses; the
  bound is on work, not on wall time, and no profile in this document promises otherwise.
- **Who may select it.** `M-64` admits this profile only for processors inside this workspace. An
  application-supplied processor may not declare itself proven, because "proven" names evidence in
  this repository's gate and an application cannot add to it.

### 7.2 Supervised isolated — `BoundedWait`, `AbandonResult`

Application-supplied processors running in a supervised OS process that `M-64` owns. `process` is
**never** called on the media worker. The worker offers a frame to a bounded request channel and
takes a result from a bounded result channel only if one is present by that frame's deadline.

- **What it claims.** Over-budget work cannot stall RTP, because the media worker never waits for
  application code: the wait is bounded by the deadline and the failure action applies at its
  expiry. A hung, looping, crashed or malformed worker costs exactly the declared action plus a
  termination and a reap. This is the profile an application selects when it requires the stack to
  contain a stall rather than to report one.
- **What it does not claim.** It does not make application code fast, and it does not make it
  correct. The abandoned result is *audio that did not get processed*, so a fail-open bypass under
  this profile is audible; the containment is of the stall, not of the artefact. Nor does it bound
  the isolated process's own memory or CPU beyond what the operating system is configured to bound
  — that is deployment configuration, and `M-68` states what it measured rather than what it
  assumed.
- The deadline action is `AbandonResult`: the frame's result is dropped unread, and §7.4's policy
  applies.

### 7.3 Trusted cooperative native — `None`, `AccountAfterReturn`

Application-supplied native code called **directly on the media worker**, by explicit selection.

- **What it claims.** Nothing about containment. It is fast, it is direct, and it is the
  application's own code running in the stack's own real-time thread.
- **What it does not claim, stated in full because this is the profile whose honest description is
  the feature.** sipx cannot preempt it, cannot cancel it, cannot reap it and cannot bound its
  memory. A callback that does not return stalls the media worker and therefore stalls RTP for that
  call, and no configured deadline, failure action or teardown barrier changes that, because every
  one of them is code that runs after the callback returns. The stack validates the declared shape
  and measures calls that *do* return; a call that does not return is outside sipx's containment and
  teardown guarantees. The profile may copy audio it was lent, spawn work the stack does not own,
  and outlive the call.
- **Conformance never upgrades it.** A cooperative-native processor that passes every check in §11
  is a processor that behaved on the harness's inputs. `contains_overrun()` is `false` for this
  profile unconditionally, and no harness result, measurement or configuration may set it true.
  Sandboxed execution is a different profile that does not exist yet and is not silently promised
  here.

### 7.4 The minimum failure policy

Every capability carries one, and `M-64` implements exactly this minimum:

```
ExecutionPolicy ::= { profile:                ExecutionProfile,
                      deadline_frames:        1..=8,
                      max_consecutive_misses: 1..=64,
                      on_failure:             BypassOpen | TerminateClosed }
```

`deadline_frames` is stated in **frame durations**, not milliseconds: a deadline of 1 means the
result must be available before the audio it belongs to must be sent. The runtime converts that to
whatever it measures against, using the declared rate and frame size. The processor never sees it —
a processor that could read its own deadline would have a clock (§4).

`max_consecutive_misses` is how many deadlines in a row a processor may miss before it has failed.
One miss is a hiccup; a run of them is a processor that does not fit its budget on this machine.

`on_failure` is the fail-open/fail-closed decision, and it is a decision only the application can
make:

- **`BypassOpen`** — the processor is bypassed and unmodified audio continues to flow. The call
  keeps working and sounds wrong. This is the right default for an effect.
- **`TerminateClosed`** — the graph is torn down and the audio it was protecting does not flow. The
  call loses media rather than carrying audio that was supposed to have been processed. This is the
  right choice for a processor whose absence is a policy breach rather than a quality regression —
  the redaction case, where unprocessed audio leaving the stack is worse than no audio leaving it.

Both are configured, both are reported as typed events by `M-64`, and neither is inferred. A
default that guessed would be guessing at whether an application's audio is allowed to leave
unprocessed.

## 8. Reset, flush, cancellation and refusal

Four distinct behaviours, never conflated, extending
[call-audio-processing.md](call-audio-processing.md) §7's two.

### 8.1 Reset

`reset(cause)` accepts that measurement restarts under a typed cause:

```
DspResetCause ::= Requested | FormatChange | Discontinuity { kind }
```

Every reset does the same thing: all sample memory is **discarded, not flushed** — a retained tail
belongs to an epoch that no longer exists, and emitting it into the new one would be old audio at a
new position. The declared parameters survive; so does the capability. After a reset,
`retained() = 0` and the next frame must open the new epoch at position 0, except after a `Loss` or
`Overflow` discontinuity, whose epoch continues at the position the seam stated (§3.5).

A discontinuity reset runs **before** the flagged frame's own samples are consumed, so those
samples open the new epoch. A processor declaring `reset: Stateless` has nothing to discard, and
§11's harness proves that by comparing a stream processed with a mid-stream reset against the same
stream processed without one.

### 8.2 Flush

`flush(sink)` writes the retained tail — at most `tail_positions` — and leaves `retained() = 0`.
It is how a graph drains at the end of input without losing the last positions of a delay line.
Flush is not a reset: it produces the tail rather than discarding it, and it is the caller's
decision which of the two an end-of-stream deserves.

### 8.3 Refusal

A refusal rejects the input and changes **nothing**: not the position expectation, not the epoch,
not one sample of state, and it writes nothing to the sink and enqueues no observation. A caller
that fixes its input and retries continues exactly where the stream stood. This is checkable, and
§11's `DSP-K3` checks it by replaying a reference stream with a refused frame spliced into the
middle and requiring byte-identical output.

| Input | Refusal |
|---|---|
| a frame before `prepare` | `NotPrepared` |
| a frame after `cancel` | `Cancelled` |
| empty `samples` | `MalformedFrame` — zero positions transform nothing, and a silent no-op would hide a broken caller |
| more than `max_frame_samples`, or more than 65,536 | `MalformedFrame` — §9.2's CPU ceiling is a contract, not a suggestion |
| a sample count that is not a multiple of the channel count | `MalformedFrame` |
| a direction other than the prepared one | `DirectionMismatch` |
| a format other than the prepared one | `FormatMismatch` — a rate change is a `prepare`, never a frame |
| a position violating §3.5 | `PositionNotContiguous` |
| a write past the sink | `OutputOverflow` |
| a scratch request past the declaration | `ScratchExhausted { requested, available }` |
| output whose position count violates the declared `length` | `LengthPolicyViolated { consumed, produced }` |
| the processor's own refusal of otherwise valid input | `Rejected { reason }` |

`prepare` refuses a rate or channel count outside the declaration with a typed `FormatError`,
leaving any previously prepared format in force — a malformed format change never half-applies.

### 8.4 Cancellation

`cancel()` is terminal and idempotent. After it: all sample memory is released, `retained() = 0`,
and every subsequent `process` and `flush` returns `Cancelled` without writing. This is the
processor's half of `M-64`'s teardown barrier: a graph proving it holds zero frames needs each
processor to be able to say it holds none, and a processor that keeps working after cancellation is
one that keeps a call alive after the call ended.

## 9. Bounds

### 9.1 Memory

A processor's memory is exactly two things, and both are declared: the state it owns
(`state_bytes`) and the scratch the caller lends it (`scratch_samples`). It performs **no
allocation after `prepare`** — not per frame, not per position, not per observation. Output goes to
the caller's sink; observations go to the caller's queue; working room comes from the caller's
scratch, whose length is fixed at exactly the declared figure so that a processor asking for more
is refused rather than served.

**What that bound is, precisely, and what it is not.** Making the workspace caller-owned is what
turns "does not allocate" from something to be observed into something the caller controls: a
processor cannot obtain more scratch or more output room than its declaration, whatever it does.
Heap growth *inside* a processor's own state is a different question, and it is measured in one
place and unproven in the other.

**Unproven under `cargo test`, and permanently so.** Counting allocations needs a global allocator,
a global allocator needs `unsafe impl GlobalAlloc`, and `unsafe_code = "forbid"` covers every target
of every crate in this workspace — an integration test as much as a library, with no `allow` able to
override a `forbid` (`E0453`). §11's harness therefore measures what it can (the inline size of the
processor against `state_bytes`), holds the scratch declaration exactly, and reports the heap
component as **unproven, by name**, rather than passing it. A check that quietly omitted it would be
the more comfortable and the less true design. What the unproven outcome must also do is say where
the proof lives; an unprovable check that names its prover is a different artifact from one that
says only "cannot".

**Measured by `heap-probe/`.** That package sits outside the workspace, as `wasm/` and `fuzz/` do
and for the same reason, installs the counting allocator, and hands §11's harness a `HeapMeter`
through `Conformance::run_with_heap_meter`. `./scripts/check-dsp-heap.sh` runs it. Two figures, one
per claim this section makes:

- **peak live bytes** across construction, `prepare`, `process`, `flush`, `reset` and `cancel`, held
  against `state_bytes` less the processor's inline size. Construction is inside the window
  deliberately: a processor that allocates its state in its constructor would otherwise measure
  zero.
- **bytes allocated after `prepare` returned**, which MUST be zero. This is the "no allocation after
  `prepare`" sentence above, and it is a separate claim from the size one — a processor that
  allocates and frees a buffer per frame owns nothing extra and is still in violation.

The comparison lives in the workspace and only the bytes come from outside, so what a figure *means*
is covered by `cargo test` even though what it *is* cannot be.

### 9.2 CPU per frame

`process` is `O(positions)` with a fixed per-position step and no hidden traversal; with the
`max_frame_samples` ceiling that is a hard per-frame bound. There is no path whose cost depends on
call length or on past input, which is precisely what §7.1's `BoundedWork` containment rests on.

## 10. The seam is `M-54`'s, the graph is `M-64`'s

Everything about reaching live call audio is `M-54`'s
([call-audio-seam.md](call-audio-seam.md)) and everything about composing processors is `M-64`'s.
Normatively:

- This epic MUST NOT introduce a second call-media tap. Both existing contracts forbid one by name
  and this one joins them: a private bypass is how two observers of one call start disagreeing
  about what happened.
- A processor MUST NOT mutate provider, playback, RTP, RTCP, negotiation or dialog state, directly
  or through the seam. It transforms the samples it is handed and reports what it did.
- A processor MUST NOT reach into a speech provider, a VAD or another processor. It may consume
  activity as a declared parameter (`M-66`'s optional VAD input), never as a call.
- Built-in and application-supplied processors use **this** interface. There is no crate-private
  door: `M-65`'s effects implement the same trait an application implements, and §11's harness runs
  against both, which is the only way the epic's central claim — that they behave identically — is
  a fact rather than an intention.
- Frame delivery order, loss under the seam's policy and the resulting discontinuity flags are the
  seam's facts. A processor's obligation is confined to §3.5's response to them.

## 11. Conformance

### 11.1 The harness is part of the contract

A capability declaration is a set of claims, and §11 is where they are checked. The harness accepts
**any** processor — built-in or external, from any crate — through a factory that produces fresh
instances, because half the checks need two instances that have seen nothing.

Every check below appears in every report. A check the harness could not prove is reported
`Unproven` with a stated reason and never omitted and never passed: a report whose length depends
on what it managed to run is a report that cannot be compared to yesterday's.

| ID | Name | What it proves |
|---|---|---|
| `DSP-K1` | capability | the declaration is internally admissible (§5) |
| `DSP-K2` | format refusal | a rate or channel count outside the declaration is refused typed, and the processor still prepares and runs on an accepted one afterwards |
| `DSP-K3` | frame refusal | every §8.3 refusal is typed and mutates nothing — proved by splicing each refusal into a reference stream and requiring byte-identical output |
| `DSP-K4` | parameter refusal | unknown, mistyped and out-of-range parameters refuse the whole set and leave the previous one in force (§3.6) |
| `DSP-K5` | determinism | two fresh instances fed the same frames write identical samples and identical observations, with the workspace poisoned differently on each run (§11.2) |
| `DSP-K6` | chunk boundary | one stream delivered as one frame and as many small frames produces the same samples |
| `DSP-K7` | reset | `retained() = 0` after a reset, and a stream processed after a reset equals the same stream on a fresh instance; a `Stateless` declaration is held to a reset being unobservable |
| `DSP-K8` | cancellation | after `cancel`: `retained() = 0`, `process` and `flush` return `Cancelled` and write nothing, and a second `cancel` is a no-op |
| `DSP-K9` | allocation | the scratch actually requested never exceeds the declaration, the sink is never overrun, and the processor's inline size is within `state_bytes` — plus, on a run given a `HeapMeter`, its peak live heap within the rest of `state_bytes` and nothing at all allocated after `prepare`; without a meter the heap component is reported `Unproven` per §9.1 |
| `DSP-K10` | discontinuity | a flagged frame is accepted, its reset runs before its own samples, and a `Realign` frame produces exactly what a fresh instance would |
| `DSP-K11` | length | a `Preserving` processor writes exactly the positions it consumed; a `Bounded` one never exceeds its declared maximum |
| `DSP-K12` | extremes | full scale, `i16::MIN`, alternating full scale, DC and long silence produce a typed result and never a panic |

A panic anywhere in a run is caught and reported as that check's failure rather than taking the
harness down: a processor that panics is a finding, and a harness that dies with it produces no
finding at all. (This requires the unwinding panic strategy; under `panic = "abort"` the harness
cannot make that promise and does not claim to.)

### 11.2 Poisoned workspace

Before each run the harness fills the sink and the scratch region with a **run-specific pattern**,
different between the two instances `DSP-K5` compares. §4.7 makes reading unwritten scratch a
contract violation; this is what makes it a *caught* one, rather than one that passes on a machine
where the leftovers happened to match.

### 11.3 The harness must be shown failing

A conformance harness that has only ever been run against well-behaved processors has proved that
it terminates. It is therefore **normative** that the harness's own test suite contains, for each
invariant, a fixture that deliberately violates exactly that invariant and an assertion that the
harness names it:

| Fixture | Violates | Caught by |
|---|---|---|
| `LeakyReset` | keeps sample memory across a reset | `DSP-K7` |
| `ZombieCancel` | keeps working after `cancel` and reports retained audio | `DSP-K8` |
| `ChunkDependent` | output depends on where the caller cut the stream | `DSP-K6` |
| `ScratchHog` | requests more scratch than it declared, and hides the refusal | `DSP-K9` |
| `HeapHog` | allocates once per frame and keeps it | `DSP-K9` (after-`prepare`, under a meter) |
| `HeapLiar` | owns a 64 KiB buffer behind a 4,096-byte declaration | `DSP-K9` (`state_bytes`, under a meter) |
| `LengthLiar` | produces one position for every two it consumed under `Preserving` | `DSP-K11` |
| `SloppyRefusal` | advances its state before refusing a frame | `DSP-K3` |
| `Panicky` | panics on `i16::MIN` | `DSP-K12` |
| `ScratchReader` | reads scratch it never wrote | `DSP-K5` |

Those fixtures are written against the crate's public API from outside the crate, which is the same
sentence as "the contract is implementable by an application" and is checked by being true. The
first eight live in `crates/sipx-audio/tests/dsp_conformance.rs`; `HeapHog` and `HeapLiar` live in
`heap-probe/` because they are the two whose invariant only has a check where a meter exists, and a
fixture kept somewhere its check cannot run is a fixture nobody notices going stale.

## 12. Vectors

### 12.1 Reference processors

Three, defined here and implemented from outside the crate by §11.3's suite. Unless a vector says
otherwise it runs on `D8`: `Inbound`, 8,000 Hz, one channel.

- **`IDENT`** — the identity. `Preserving`, `Stateless`, latency 0, tail 0, no parameters, no
  scratch. Emits `PassedThrough` per frame.
- **`DELAY1`** — a one-position delay line. `Preserving`, `ClearsState`, latency 1, tail 1.
- **`GAIN2`** — a saturating gain of `2^shift`, with one parameter `shift` of domain
  `Integer { min: 0, max: 2 }`. `Preserving`, `Stateless`. Emits `Saturated { positions }` when it
  clamps.

### 12.2 Transform vectors

| ID | Input | Expected |
|---|---|---|
| DSP-V1 | `IDENT` on `[1, −1, 32767, −32768]` | the same four samples, and one `PassedThrough` |
| DSP-V2 | `DELAY1` on `[1,2,3]` at position 0, then `[4,5,6]` at position 3 | `[0,1,2]` then `[3,4,5]`; `retained() = 1`; `flush` yields `[6]` and `retained() = 0` |
| DSP-V3 | `DELAY1` on `[1,2,3]`, then `reset(Requested)`, then `[4,5,6]` at position 0 | `retained() = 0` at the reset — the held `3` is discarded, not flushed — then `[0,4,5]` |
| DSP-V4 | `DELAY1` on `[1,2,3]`, then `[4,5,6]` at position 0 flagged `Realign` | `[0,4,5]`: the reset ran before the flagged frame's own samples |
| DSP-V9 | `GAIN2` with `shift = 1` on `[1, 2, 32767]`, then `shift = 9`, then a parameter named `gain`, then `shift = Flag(true)` | `[2, 4, 32767]` and `Saturated { positions: 1 }`; then `OutOfRange`, `Unknown` and `KindMismatch` — with `shift = 1` still in force after all three, proved by re-running the same three samples |

### 12.3 Refusal vectors

| ID | Input | Expected |
|---|---|---|
| DSP-V5 | `DELAY1`: a first frame at position 7; then position 0; then position 9 unflagged; then position 9 flagged `Loss` | refused `PositionNotContiguous`; accepted; refused `PositionNotContiguous`; accepted — and the accepted frame after the first refusal produces exactly what it would have without it |
| DSP-V6 | `IDENT`: an empty frame; an `Outbound` frame; a 16,000 Hz frame | `MalformedFrame`; `DirectionMismatch`; `FormatMismatch` — each writing nothing, and the next valid frame accepted unchanged |
| DSP-V11 | capabilities with no channels, channel 9, a 70,000-sample frame, a duplicate parameter id, `deadline_frames = 0` | each refused by `validate()` |

### 12.4 Bound vectors

| ID | Input | Expected |
|---|---|---|
| DSP-V7 | a 2-sample sink written twice; a 4-sample scratch asked for 5 | `OutputOverflow` with the written prefix intact; `ScratchExhausted { requested: 5, available: 4 }`, and the request recorded at 5 so the declaration is still what the processor is held to |
| DSP-V8 | a 2-slot observation queue given four observations | `[PassedThrough, Lost { count: 3 }]` — §6's coalescing, which is the analysis contract's |
| DSP-V12 | `max_output_positions(160)` under `Preserving`, and under `Bounded { max_positions: 4 }` | 160; 4 |

### 12.5 Profile vectors

| ID | Input | Expected |
|---|---|---|
| DSP-V10 | each `ExecutionProfile` | `ProvenInline` → `BoundedWork`, `AccountAfterReturn`, contains an overrun, runs on the media worker · `SupervisedIsolated` → `BoundedWait`, `AbandonResult`, contains an overrun, does **not** run on the media worker · `TrustedCooperativeNative` → `None`, `AccountAfterReturn`, does **not** contain an overrun, runs on the media worker |
