# Deterministic call-audio frame processing

**Status:** normative target · **Epic:** `call-audio-analysis` · **Contract story:** `M-57` ·
**Implementing stories:** `M-54` (attachment seam), `M-58`, `M-59`, `M-60`, `M-61` ·
**Design:** [call-audio-analysis](../designs/call-audio-analysis.md) ·
**Crates:** `sipx-audio` (processor), `sipx-media`/`sipx-call` (seam, per `M-54`)

Where this document and an implementation disagree, this document is right until it is changed
deliberately. It defines the sans-I/O contract under which live call audio becomes small, typed,
reproducible facts — voice activity, level, clipping, impulses, DC offset, silence — before any
algorithm is implemented and before any SDK event exists. The point of writing it first is the
central promise: **identical inputs produce identical observations on every machine**, so a
fixture is evidence and a regression is a diff, not a statistical argument.

## 1. Scope and shape

The processor is a pure state machine. Its complete input vocabulary is: a validated configuration,
PCM frames with explicit metadata, declared format changes, and requested resets. Its complete
output vocabulary is typed observations drained by the caller. It MUST NOT own or use a socket, a
device, a clock read, a random source, a thread, or a background task; time exists only as sample
counts derived from the declared rate. Delivery of frames from a live call is the attachment seam's
job and is assigned to `M-54` (§9); carrying observations into `CallEvent` and the application SDK
is `M-58`/`M-59`'s job (§10).

This document defines the frame and observation types, the exact integer arithmetic behind every
per-window fact, the activity/hangover/silence-timeout state machine, the reset and refusal
taxonomy, and the memory, CPU and queue bounds. §12 layers bounded threshold calibration on top of
all of it (`M-60`), under §4's determinism obligation and expressed only in sample counts. It
deliberately does not define any speech, tone or DTMF classification (§10).

## 2. Normative references

- **RFC 3550** — RTP. The upstream origin of timing and ordering; this contract never sees RTP
  packets, only decoded frames, so RTP sequence numbers and timestamps stop at the seam.
- **RFC 3551** — the audio profile behind the linear PCM boundary this processor consumes.
- **RFC 4733** — telephone events. DTMF already has a typed call event; this contract detects no
  tones (§10).
- [linear-pcm.md](linear-pcm.md) — the owned mono PCM application boundary: signed 16-bit
  representation, the supported rate domain 1..=384,000 Hz, and typed rate refusal. This contract
  consumes that boundary and adds no second PCM representation.
- [media-runtime.md](media-runtime.md) — worker ownership and shutdown for the media runtime the
  seam lives in. The processor itself owns no worker, so nothing here amends that document.
- **ITU-T Recommendation P.56** (informative) — defines *active speech level* as a measurement
  method. The facts in §5 are deliberately not P.56 levels: they are raw integer window facts with
  no gain assumption, and no door of this stack may describe them as P.56 measurements.

No third-party implementation is referenced by this contract, its vectors, or its rationale.

## 3. Input contract

### 3.1 Direction

```
Direction ::= Inbound | Outbound
```

`Inbound` is audio decoded from the remote peer; `Outbound` is audio produced locally for
transmission. A processor instance is bound to exactly one direction at construction. A frame
tagged with the other direction is refused (§7.3) — per-direction analysis state never interleaves,
which is what makes every vector in §11 a single-stream replay.

### 3.2 Sample rate and format

The sample format is mono signed 16-bit linear PCM at a declared rate in the
[linear-pcm.md](linear-pcm.md) domain: 1..=384,000 Hz. Rate 0 and rates above 384,000 are refused
with that boundary's existing `UnsupportedSampleRate` type — reused, not re-minted — exactly as in
its PCM-4 vector, at construction and at `declare_format` alike. The
seam owns conversion into this format (`M-54` reusing `M-43`); the processor never resamples,
never guesses a rate from buffer length, and carries no per-frame rate — the rate is declared
state, changed only by §7.2.

### 3.3 The frame

```
PcmFrame ::= { direction: Direction,
               sequence:  u64,
               discontinuity: Option<Discontinuity>,
               samples:   &[i16] }        -- 1..=65,536 samples, borrowed

Discontinuity ::= Loss      -- upstream frames were lost (network or decode)
                | Overflow  -- the seam dropped frames under its loss policy
                | Realign   -- the seam re-anchored the timeline
```

Samples are borrowed for the duration of the call and MUST NOT be retained after `process`
returns — raw audio non-retention is a design invariant (`M-61`), not an optimization. The
65,536-sample ceiling is the per-call CPU bound of §8.2; larger and empty frames are refused
without state change.

### 3.4 Sequence and discontinuity rules

`sequence` numbers delivered frames, assigned by the seam. After construction or any reset (§7),
the first accepted frame's sequence establishes the base with any value. Thereafter, for previous
accepted sequence `p`:

| Arriving frame | Behavior |
|---|---|
| `sequence = p + 1`, no flag | accepted; the continuous stream continues |
| `sequence > p`, flag set | accepted; discontinuity reset (§7.1) runs before its samples are consumed |
| `sequence > p + 1`, no flag | refused `MalformedSequence`; the seam always flags a gap, so an unflagged gap is a broken upstream, not a loss to smooth over |
| `sequence <= p` | refused `MalformedSequence`, flagged or not; sequence is strictly monotonic |

A flagged frame with `sequence = p + 1` is legal: `Overflow` and `Realign` breaks need not skip a
number. The flag is authoritative — a discontinuity is what the seam says happened, never what the
processor infers.

## 4. Determinism and arithmetic

Everything the processor emits is a pure function of (configuration, ordered inputs). Normatively:

- All decision arithmetic is two's-complement integer arithmetic at the widths stated in §5 and
  §6, with
  no floating point, no platform-width types in any comparison, and no wrapping: §5's width proofs
  make overflow unreachable, and an implementation MUST debug-assert that, not saturate silently.
- Divisions appear only in exactly-specified places, and never in the per-sample path: the
  duration derivation below, and §12.2's once-per-window deviation.
- Every configured duration is converted **once**, at construction and at each accepted format
  change, into a sample count by the exact formula

  ```
  samples(d_ms, rate) = ceil(d_ms · rate / 1000) = (d_ms · rate + 999) div 1000    -- in u64
  ```

  and only sample counts exist at runtime. There is no millisecond, no `Instant`, and no wall
  clock anywhere in the contract.

Two processors constructed from the same configuration and fed the same input sequence MUST
produce byte-identical drain sequences on every architecture and platform (CAP-D1).

## 5. Windows and per-window facts

### 5.1 Configuration

```
AnalysisProfile ::= { direction:            Direction,
                      rate:                 Hz          -- 1..=384,000
                      window_ms:            u32         -- >= 1; derived W in 1..=65,536
                      activation_amplitude: i32         -- 1..=32,767
                      silence_amplitude:    i32         -- 1..=32,768
                      impulse_amplitude:    i32         -- 1..=32,768
                      dc_amplitude:         i32         -- 1..=32,767
                      clip_samples:         u32         -- 1..=W
                      hangover_ms:          u32         -- derived count <= 2^32 - 1
                      silence_timeout_ms:   Option<u32> -- >= 1 when present; same cap
                      queue_capacity:       u32 }       -- 2..=4,096
```

Every violated domain is a typed `ProfileError` naming the field, before any allocation sized by
the bad value. The derived window length `W` is capped at 65,536 samples because that cap is what
makes §5.2's arithmetic provably fit `i64`; a 200 ms window at 384,000 Hz derives 76,800 and is
refused rather than silently clamped (CAP-C2).

### 5.2 Accumulators

Samples are consumed into consecutive, non-overlapping windows of exactly `W` samples, aligned to
the stream position (sample 0 is the first sample after the last reset; window `k` covers samples
`[k·W, (k+1)·W)`). Per window, the processor maintains exactly:

| Accumulator | Width | Content |
|---|---|---|
| `peak` | `i32` | max of \|s\| over the window (\|−32768\| = 32,768) |
| `sum` | `i64` | Σ s |
| `energy` | `i64` | Σ s² |
| `clipped` | `u32` | count of samples with s = 32,767 or s = −32,768 |

Width proof, with `W <= 2^16` and `|s| <= 2^15`: `energy <= W · 2^30 <= 2^46`;
`W · energy <= 2^62`; `|sum| <= W · 2^15 <= 2^31`, so `sum² <= 2^62`; and with
`A = activation_amplitude <= 2^15`, `A² · W² <= 2^62`. Every quantity below therefore fits `i64`
with headroom and no comparison can overflow.

### 5.3 Predicates

On window completion the five facts are computed, in this order, from the accumulators alone:

| Fact | Exact predicate | Reading |
|---|---|---|
| `clipping` | `clipped >= clip_samples` | the window touched full scale often enough to distort |
| `impulsive` | `peak >= impulse_amplitude` **and** `energy < 2 · peak²` | more than half the window's energy sits in its single largest sample: a click, not a signal |
| `active` | **not** `impulsive` **and** `W · energy − sum² >= A² · W²` | the DC-free variance meets the activation threshold: `(W·energy − sum²)/W²` is exactly the window's variance, so this is `variance >= A²` with no division performed |
| `dc_offset` | `\|sum\| >= dc_amplitude · W` | the mean magnitude meets the DC threshold, again division-free |
| `silent` | `peak < silence_amplitude` | nothing in the window rose above the silence floor |

The variance form is chosen over raw energy deliberately: a constant signal — a stuck DAC at
+32,767, a DC-biased capture — has variance exactly 0 and is *not* voice, while the same energy as
genuine modulation is (CAP-W3 versus CAP-W2). The impulse exclusion runs first so a single
full-scale click cannot masquerade as a voice onset (CAP-W4).

Every completed window enqueues one observation:

```
Observation::Window { index: u64,        -- k; first sample is k·W in the current epoch
                      peak: i32, sum: i64, energy: i64, clipped: u32,
                      clipping, impulsive, active, dc_offset, silent: bool }
```

A partial window at the moment of a reset, format change or discontinuity is discarded without
emission: a fact computed over fewer than `W` samples would compare against thresholds derived for
`W` and would be a different, undeclared measurement.

## 6. Activity, hangover and the silence timeout

Voice activity is edge-triggered over the per-window `active` fact. State: `Inactive` or `Active`,
an inactive-run sample counter (`u64`), and the end position of the last active window (`u64`).

| State | Completed window | Effect |
|---|---|---|
| `Inactive` | `active` | emit `VoiceStarted { at_sample: k·W }`; become `Active`; inactive-run := 0 |
| `Active` | `active` | inactive-run := 0 |
| `Active` | not `active` | inactive-run += W; when inactive-run >= hangover count: emit `VoiceEnded { at_sample: end of last active window, cause: Hangover }`; become `Inactive` |
| `Inactive` | not `active` | nothing |
| `Active` | reset of any cause (§7) | emit `VoiceEnded { at_sample: end of last active window, cause: Cut }` before the `Reset` observation; become `Inactive` |

The hangover is a sample count derived by §4's formula; a hangover of 0 ends voice at the first
inactive window. `at_sample` positions are in the current epoch's sample index (they restart at 0
after each reset, with the epoch identified by the `Reset` observation that opened it).

The silence timeout is independent of activity: a run counter (`u64`) accumulates `W` per consecutive
`silent` window and clears on any non-silent window. When the run first reaches the derived
timeout count, the processor emits `SilenceElapsed { at_sample: first sample of the run }` exactly
once, and re-arms only after a non-silent window or a reset. `silence_timeout_ms: None` disables
the timer entirely.

Ordering within one completed window is fixed: `Window` first, then `VoiceStarted` or
`VoiceEnded`, then `SilenceElapsed`, then §12.7's `ThresholdUpdated`. Within one `process` call,
windows complete in stream order.

## 7. Reset and refusal

Two distinct behaviors, never conflated: a **reset** accepts the input and restarts measurement
under a typed cause; a **refusal** rejects the input and changes nothing.

### 7.1 Reset causes

```
Observation::Reset { cause: Requested
                          | FormatChange { rate: Hz }
                          | Discontinuity { kind: Loss | Overflow | Realign } }
```

Every reset performs the same transition: `VoiceEnded { cause: Cut }` first if voice was active,
then the `Reset` observation, then all runtime state clears — partial-window accumulators
(discarded unemitted), activity state, both run counters, the stream position, and the sequence
base. The configuration survives; the observation queue and its contents survive (facts already
earned are not destroyed by a reset). A discontinuity reset runs before the flagged frame's own
samples are consumed, so those samples open the new epoch.

### 7.2 Format change

`declare_format(rate)` with a rate in 1..=384,000 re-derives every sample count from §4's formula
against the new rate (re-checking the §5.1 domains, `W <= 65,536` included) and performs a
`FormatChange` reset. A rate outside the domain — and a re-derivation that leaves the domain — is
a typed refusal (`UnsupportedSampleRate` / `ProfileError`) and the previous declared format remains in
force, untouched (CAP-F2). A malformed format change never half-applies.

### 7.3 Frame refusals

| Input | Refusal |
|---|---|
| empty `samples` | `MalformedFrame` — zero samples measure nothing and a silent no-op would hide a broken seam |
| more than 65,536 samples | `MalformedFrame` — the §8.2 CPU ceiling is a contract, not a suggestion |
| direction differs from the bound direction | `DirectionMismatch` |
| sequence violating §3.4 | `MalformedSequence` |

A refusal is a typed error returned from `process`; it enqueues no observation and mutates no
state — not the sequence expectation, not the position, not an accumulator. The caller that
retries after fixing its input continues exactly where the stream stood.

## 8. Bounds

### 8.1 Memory

Processor state is exactly: the configuration and its derived counts, the four §5.2 accumulators
plus the intra-window position, the stream position, the §6 activity and run state, the sequence
base, and the observation queue of `queue_capacity` preallocated slots. Its size is a constant of
the configuration — independent of call duration, frame count and frame size. After construction
the processor performs **no allocation**: not per frame, not per window, not per observation. Raw
samples are never copied into state (§3.3).

### 8.2 CPU per frame

The per-sample step is a fixed number of integer operations (peak compare, two accumulator adds,
one multiply, clip compare, position increment); window completion adds a fixed number of
comparisons and at most three enqueues. `process` is therefore O(samples in the frame) with
constant per-sample work and no hidden traversal; with the 65,536-sample frame ceiling this is a
hard per-call bound. There is no path whose cost depends on call length or on past input.

### 8.3 The observation queue

The queue is a caller-drained ring of `queue_capacity` slots (2..=4,096). `drain` returns the
queued observations in order and empties it; nothing else removes entries. When an enqueue would
exceed capacity, the newest retained entry is coalesced into loss accounting rather than blocking
or growing:

- if the newest entry is already `Observation::Lost { count }`, increment `count`;
- otherwise replace the newest entry with `Lost { count: 2 }` — one for the entry replaced, one
  for the observation that had no slot.

`Lost` is deterministic like everything else (CAP-Q1): the same input against the same capacity
loses the same observations. A caller that drains after every `process` call at the reference
profile's capacity never sees `Lost` — the marker exists so an undersized queue is a visible,
counted fact instead of silent absence or an unbounded buffer.

## 9. The attachment seam is `M-54`'s, and there is exactly one

Everything about reaching live call audio is assigned to `M-54`'s call-owned PCM processing seam
(design: [local-speech](../designs/local-speech.md)): attachment to received and transmitted
audio, per-call finite queues, the frame-loss policy for slow consumers, discontinuity flagging,
fan-out to simultaneous consumers, and format conversion via `M-43`'s resampling boundary. This
processor is one consumer of that seam. Normatively:

- This epic MUST NOT introduce a second call-media tap. If the seam cannot deliver what analysis
  needs, the seam's contract is amended under `M-54` — a private bypass is how two observers of
  one call start disagreeing about what happened.
- The processor MUST NOT mutate provider, playback, RTP, RTCP or negotiation state, directly or
  through the seam. It observes.
- The processor MUST NOT reach into a speech provider. Activity MAY inform synthesis ducking only
  as typed observations consumed by the application layers above (`M-58`'s carriage), never as a
  call from analysis into a provider.
- Frame delivery order, loss under the seam's policy, and the resulting `Discontinuity` flags are
  the seam's facts; the processor's obligation is confined to §3.4's response to them.

## 10. Boundaries and assignment of the rest

- **`M-10` stays the transport-quality surface.** Loss, jitter, round-trip and MOS belong to the
  RTCP snapshot; nothing in this contract measures packet delivery, and `M-59` MUST NOT duplicate
  those statistics as observations.
- **RFC 4733 DTMF keeps its one detector.** No tone, DTMF, echo or answering-machine
  classification exists in this contract; adding any is a new story with its own corpus, not an
  extension of §5.
- **`M-58`** carries `VoiceStarted`/`VoiceEnded` through `CallEvent` and the application SDK.
  **`M-59`** shapes level/clipping/silence reporting from `Window` facts. Neither may add an
  observation source outside this contract.
- **`M-60`** layers threshold adaptation, and §12 is that layer. The obligation it was written
  against still governs it: the result remains expressible as this contract's configuration over
  time, with bounded, observable, deterministic state — an adapted threshold is a new declared
  value, never a hidden multiplier.
- **`M-61`** proves the hostile-input obligations this contract already implies: no panic, no
  unbounded allocation and no starvation under extreme amplitudes, impulses, DC, alternating
  samples, format churn, discontinuity storms and long silence. §11's vectors are its floor, not
  its ceiling.

## 11. Vectors

### 11.1 Reference profile

Unless a vector says otherwise, it runs on `P8`: direction `Inbound`, rate 8,000 Hz,
`window_ms = 20` (derived `W = 160`), `activation_amplitude = 2,048`, `silence_amplitude = 64`,
`impulse_amplitude = 16,384`, `dc_amplitude = 512`, `clip_samples = 8`, `hangover_ms = 200`
(derived 1,600), `silence_timeout_ms = 2,000` (derived 16,000), `queue_capacity = 64`. Frames are
160 samples with consecutive sequences starting at 0 and no flag, and the caller drains after
every call. Sample patterns are given as exact signed decimal values.

### 11.2 Window facts

| ID | Input (one window) | Expected `Window` observation |
|---|---|---|
| CAP-W1 | 160 × `0` | `peak 0, sum 0, energy 0, clipped 0`; `silent`; no other fact |
| CAP-W2 | alternating `+8192, −8192` | `peak 8192, sum 0, energy 10,737,418,240, clipped 0`; `active` (variance term `1,717,986,918,400 >= 107,374,182,400`); then `VoiceStarted { at_sample: 0 }` |
| CAP-W3 | 160 × `+32767` | `peak 32767, sum 5,242,720, energy 171,788,206,240, clipped 160`; `clipping` and `dc_offset`; **not** `active` — `W·energy − sum² = 0` exactly |
| CAP-W4 | 159 × `0`, `+32767` at index 40 | `peak 32767, sum 32,767, energy 1,073,676,289, clipped 1`; `impulsive` (`energy < 2·peak²`); **not** `active`, `clipping`, `dc_offset`, `silent` |
| CAP-W5 | 160 × `+1000` | `peak 1000, sum 160,000, energy 160,000,000, clipped 0`; `dc_offset` (`160,000 >= 81,920`); **not** `active` (variance 0), **not** `silent` |

### 11.3 Activity, hangover, silence timeout

| ID | Input | Expected |
|---|---|---|
| CAP-A1 | frame 0 = CAP-W2 pattern, frames 1..=11 all zeros | `VoiceStarted { 0 }` after window 0; `VoiceEnded { at_sample: 160, cause: Hangover }` after window 10 (inactive run reaches 1,600); no `SilenceElapsed` (silent run 1,760 < 16,000) |
| CAP-A2 | 100 frames of 160 × `0` | `SilenceElapsed { at_sample: 0 }` exactly once, after window 99 (run reaches 16,000); no voice event; a 101st silent frame emits no second `SilenceElapsed` |
| CAP-A3 | frame 0 = CAP-W2 pattern, then `reset()` | after the frame: `Window`, `VoiceStarted { 0 }`; the reset yields `VoiceEnded { at_sample: 160, cause: Cut }`, then `Reset { Requested }` |
| CAP-D1 | two processors from `P8`, both fed CAP-A1's frames | byte-identical drain sequences |

### 11.4 Format changes

| ID | Input | Expected |
|---|---|---|
| CAP-F1 | one zero window at 8,000; `declare_format(16,000)`; 320 × `0` | `Reset { FormatChange { 16,000 } }`; derived `W = 320`; the 320 samples complete the new epoch's window 0 |
| CAP-F2 | `declare_format(0)`, then `declare_format(384,001)` | both refused `UnsupportedSampleRate`; no observation; 8,000 Hz remains in force and the next in-sequence frame is accepted |
| CAP-F3 | `declare_format(8,193)` | accepted; `W = ceil(20 · 8,193 / 1000) = 164` — the derivation rounds up, never truncates a window to fewer samples than the duration covers |

### 11.5 Sequence and discontinuity

| ID | Input | Expected |
|---|---|---|
| CAP-S1 | frame `seq 0` with 100 × `0`; frame `seq 5, Loss` with 160 × `0` | the 100-sample partial window is discarded unemitted; `Reset { Discontinuity { Loss } }`; the 160 samples complete new-epoch window 0; the next accepted frame is `seq 6` |
| CAP-S2 | frame `seq 0`; frame `seq 2` unflagged; frame `seq 1` | `seq 2` refused `MalformedSequence` with no observation and no state change; `seq 1` then accepted and completes window 1 |
| CAP-S3 | frame `seq 0`; frame `seq 0` again, with and without a flag | both repeats refused `MalformedSequence` |
| CAP-S4 | frame `seq 0`; frame `seq 1, Overflow` | accepted: `Reset { Discontinuity { Overflow } }` before its samples — a flagged frame need not skip a sequence number |

### 11.6 Queue bound

| ID | Input | Expected |
|---|---|---|
| CAP-Q1 | `P8` with `queue_capacity = 2`; one frame of 480 × `0`; drain once | exactly `[ Window { index 0, … }, Lost { count: 2 } ]` — window 1's entry was coalesced, window 2's had no slot |

### 11.7 Refused frames

| ID | Input | Expected |
|---|---|---|
| CAP-N1 | frame with 0 samples | `MalformedFrame`; sequence expectation and position unchanged |
| CAP-N2 | frame with 65,537 samples | `MalformedFrame` |
| CAP-N3 | `Outbound` frame to the `Inbound`-bound `P8` | `DirectionMismatch` |

### 11.8 Refused configurations

| ID | Input | Expected |
|---|---|---|
| CAP-C1 | `window_ms = 0` | `ProfileError` naming `window_ms` |
| CAP-C2 | `window_ms = 200` at rate 384,000 | derived 76,800 > 65,536; `ProfileError` — refused, never clamped |
| CAP-C3 | `queue_capacity = 1`; `queue_capacity = 4,097` | `ProfileError` naming `queue_capacity` |
| CAP-C4 | rate 0; rate 384,001 | `UnsupportedSampleRate`, the [linear-pcm.md](linear-pcm.md) PCM-4 type reused |
| CAP-C5 | `silence_timeout_ms = Some(0)` | `ProfileError` — a zero timeout would fire before any silence existed |

## 12. Threshold calibration (`M-60`)

A fixed activation amplitude is right for one room. §5.1's reference profile is a guess about a
quiet channel, and on a noisy one it opens voice and never closes it; on a very quiet one it misses
a talker. This section makes that one value move, and does so under the same discipline as every
other number here: **every limit is a sample count or an amplitude, the state is a constant, the
evolution is inspectable, and two runs of one input produce the same thresholds.** A calibrated
analyser is still a fixture-reproducible pure function of its inputs — the vectors in §12.12 pin
the exact threshold at the exact sample.

### 12.1 What adapts, and what does not

Exactly one configured value adapts: `activation_amplitude`. The window, the silence, impulse and
DC amplitudes, `clip_samples`, the hangover, the silence timeout and the queue capacity are fixed
for the life of the configuration.

§5.3's predicates are **unchanged**. `active` compares against the *effective activation
amplitude*, which is a declared value in that field's own domain (`1..=32,767`) — never a
multiplier, a scale factor or a correction applied to a fact. Everything §5.3 computes from the
accumulators means exactly what it meant before.

An analyser with no calibration profile behaves exactly as §5 and §6 already specify: the effective
activation amplitude is the configured one, forever, and no observation of this section is ever
emitted. Calibration is opt-in and adds nothing to the frame path when it is off (CAL-12).

### 12.2 The window deviation

For a completed window with the §5.2 accumulators `sum` and `energy` over `W` samples:

```
V         = W · energy − sum²          -- the DC-free variance numerator §5.3 already forms
deviation = isqrt(V) div W             -- in u64; isqrt is the floor of the real square root
```

`deviation` is exactly the quantity §5.3's activation predicate compares against. For any integer
`A >= 0` and `W >= 1`:

```
V >= A² · W²   ⟺   isqrt(V) >= A · W   ⟺   deviation >= A
```

so `active` is `not impulsive and deviation >= A_eff`, and calibration moves a value in the same
units as the fact it governs. This is the one division §4 admits outside the duration derivation:
it runs once per completed window, never per sample, and only when calibration is configured.

`0 <= deviation <= 32,768` for every supported input: §5.2's proof gives `V <= W² · 2^30`, so
`isqrt(V) <= W · 2^15`.

### 12.3 Configuration

```
CalibrationProfile ::= { calibration_ms:     u32         -- >= 1; derived C, the warm-up
                         update_ms:          u32         -- >= 1; derived U, the update period
                         margin_amplitude:   i32         -- 1..=32,767
                         floor_amplitude:    i32         -- 1..=32,767
                         ceiling_amplitude:  i32         -- floor_amplitude..=32,767
                         max_step_amplitude: i32         -- 1..=32,767
                         freeze_limit_ms:    Option<u32> -- >= 1 when present; derived F }
```

`AnalysisProfile` gains one optional field carrying this. `C`, `U` and `F` are derived by §4's
formula against the declared rate, at construction and again at each accepted format change,
exactly like `W`, the hangover and the silence timeout; none may exceed `2^32 − 1`.

Every violated domain is a typed `ProfileError` naming the field, before any allocation is sized by
it (CAL-C1). Two refusals are specific to this section and are refusals rather than corrections,
for §5.1's reason — a clamped configuration measures something other than what was asked for:

- `ceiling_amplitude < floor_amplitude` refuses naming `ceiling_amplitude`. An empty interval is
  not an interval, and swapping the two would silently run a configuration nobody wrote (CAL-C2).
- `activation_amplitude` outside `[floor_amplitude, ceiling_amplitude]` refuses naming
  `activation_amplitude` (CAL-C3). This is what makes §12.11's guarantee unconditional: the
  invariant `floor <= A_eff <= ceiling` holds from sample 0, with no clamping step anywhere and no
  epoch during which the analyser is more sensitive than its declared floor.

### 12.4 The update period and what it measures

**Period boundaries are a function of sample position alone, never of content.** A counter
accumulates `W` per completed window; when it reaches `U` the period ends and the counter restarts
at 0. A period therefore spans `ceil(U/W)` windows and `ceil(U/W)·W` samples, whatever the audio
did, which is what makes a threshold's evolution readable off a sample position rather than
reconstructed from a history of decisions.

Within a period the analyser holds exactly two measurement values: `eligible`, a count, and
`observed`, the least deviation among eligible windows.

A window is **eligible** when §5.3 labels it neither `clipping`, nor `impulsive`, nor `dc_offset`.
A window the contract has already called distortion, a click or a stuck bias is not a measurement
of background noise, and letting one define the quiet level is precisely how a single impulse or a
DC-biased capture would move a threshold (CAL-5a, CAL-5b, CAL-5c). A `silent` window *is* eligible:
it is the quietest thing there is and it is exactly what the floor should learn from.

`observed` is the minimum rather than a mean. The quietest eligible window in a period is the best
available estimate of what the channel sounds like with nothing on it, and a minimum cannot be
dragged upward by one loud outlier the way a mean can.

### 12.5 The update rule

At a period end, with `A` the effective threshold in force:

```
target = clamp(observed + margin_amplitude, floor_amplitude, ceiling_amplitude)
next   = A + clamp(target − A, −max_step_amplitude, +max_step_amplitude)
```

`next` replaces `A` and takes effect for the **next** window; the window that ended the period was
measured against `A`.

The margin is additive in deviation units rather than a ratio, deliberately: it is exact in
integers, it needs no second division, it is monotone in the observed floor, and its worst case is
the trivially bounded `32,768 + 32,767 = 65,535`. Because `A ∈ [floor, ceiling]` (§12.3) and
`target ∈ [floor, ceiling]` by construction, `next ∈ [floor, ceiling]`: a bounded move toward a
bounded target cannot leave the interval.

### 12.6 Freeze conditions

An update is **skipped** when any of the following holds at the period end. They are tested in this
order and the first that holds is the outcome; the period's measurement is discarded and the period
counter restarts exactly as it would otherwise, because §12.4's boundaries do not depend on
content.

| Outcome | Condition | Why |
|---|---|---|
| `Warmup` | the epoch's sample position is below `C` | Nothing has been measured for long enough to be a background level. `C` is the calibration period, and it is re-armed by every reset (§12.8). |
| `Voiced` | any window in the period was `active`, **or** voice is open at the period end | A period containing speech is not a measurement of background. Adapting through speech is how a threshold climbs into a talker and cuts them off, or falls into the room and never lets go. |
| `NoMeasurement` | `eligible = 0` | Every window was clipping, impulsive or DC. There is no observation, so there is no move. |

Freezing never changes the threshold and never emits an observation.

**The `Voiced` freeze is itself bounded, in samples.** A run counter accumulates the period length
for each consecutive period frozen by `Voiced`; when adding the ending period would reach `F`, that
period's update is applied anyway and the run counter clears. `freeze_limit_ms: None` disables the
bound, and an analyser configured that way holds its threshold for exactly as long as voice stays
open — which for continuously loud input is forever, the same behaviour a fixed profile has.

The bound is what turns §12.11's third clause from a hope into a sample count. Under continuous
noise at deviation `d`, the forced update sets the threshold toward `d + margin`, which is above
`d`, so windows go inactive and the hangover closes the interval (CAL-3). `Warmup` and an applied
update clear the run counter; `NoMeasurement` leaves it as it stands, because nothing was withheld
by voice and nothing was applied.

### 12.7 The observation

```
Observation::ThresholdUpdated { at_sample:            u64  -- first sample of the window the new
                                                           -- value takes effect for
                                activation_amplitude: i32  -- the new effective threshold
                                previous:             i32  -- the value it replaces
                                observed_floor:       i32 }-- the period's minimum eligible deviation
```

Emitted only when `next != A`. An update that lands on the value already in force is not a change
and says nothing, so a settled threshold is silent (CAL-1, CAL-4).

Ordering within one completed window extends §6's fixed order by one: `Window`, then a voice
transition, then `SilenceElapsed`, then `ThresholdUpdated`. Placing it last is what makes *"the new
value takes effect for the next window"* readable straight off the drain sequence.

### 12.8 Reset semantics

A reset of any cause (§7.1) — requested, format change, or a flagged discontinuity — clears exactly
these fields, and no others:

| Field | On reset |
|---|---|
| the period counter | 0 |
| `eligible` and `observed` | discarded; the period they measured no longer exists |
| the period's "contained an active window" flag | cleared |
| the `Voiced` freeze run counter | 0 |
| the last outcome and last observed floor an application can read (§12.9) | cleared to "nothing measured yet" |
| the warm-up | re-armed: no update may apply until `C` samples of the **new** epoch have completed windows |

and preserves exactly these:

| Field | On reset |
|---|---|
| the effective activation amplitude | unchanged |
| the calibration profile | unchanged; a `FormatChange` re-derives `C`, `U` and `F` against the new rate (§7.2) |
| the count of updates applied | unchanged |

The timeline broke; the room did not. Discarding a learned threshold at every `Realign` would make
a lossy network permanently re-calibrate, and would mean an application's inspected threshold
changes for a reason that has nothing to do with the audio. An amplitude is not a rate, so it
survives a format change too (CAL-6, CAL-6b, CAL-6c). A caller that genuinely wants the configured
threshold back constructs a new analyser — which is exactly what call teardown does, so a
cancelled call leaves no calibration behind by construction.

### 12.9 What an application may read, and what it may not

An analyser exposes one read-only snapshot, taken by shared reference. It mutates nothing, drains
nothing and is not a queue: taking it a thousand times between frames leaves the observation
sequence byte-identical to a run that never takes it (CAL-11). It is a value and not a view, so a
snapshot taken earlier keeps saying what it said.

It carries the configured `AnalysisProfile`, the effective activation amplitude, the derived `W`,
hangover, silence-timeout, `C`, `U` and `F` counts, the number of updates applied, the last
period's observed floor, and the last period's outcome.

It carries **no audio, and there is none to carry.** §3.3 forbids retaining samples past `process`
and §8.1 enumerates the whole of processor state, in which no audio buffer appears. Every field of
the snapshot is a count or an amplitude — the same class of scalar the contract already publishes
in `Observation::Window`. There is no accessor anywhere in this contract that returns samples, and
adding one would contradict §3.3.

**Carriage onto an application wire (`M-84`).** The application contract publishes a *subset* of this
snapshot as [`app-contract.md`](app-contract.md) §5.2.1's `voice` member and announces every move as
its `call.voice.thresholds` event. The subset is the effective activation amplitude and the derived
counts — everything invariant across a reset by §12.8 — so a delivered record cannot go stale except
by moving, and moving announces itself. The count of updates applied, the last observed floor and
the last period's outcome stay here, readable by a Rust caller and not published, because each is
cleared by a reset and none is a fact an application acts on; adding one later is a *field* addition
under that contract's §4 rather than a new wire line. Nothing in either direction widens what this
section permits: the wire carries strictly less than the snapshot, and the snapshot carries no
audio.

### 12.10 Bounds

Calibration adds to §8.1's state exactly: the effective threshold (`i32`), the period counter
(`u64`), `observed` (`i32`), `eligible` (`u32`), the period's active flag (`bool`), the freeze run
counter (`u64`), the update count (`u64`), the last observed floor and the last outcome. That is a
constant of the configuration, independent of call duration, frame count and frame size, and it
allocates nothing — §8.1 is otherwise unchanged.

Per completed window it adds one integer square root over a `u64` and a fixed number of
comparisons. **Per sample it adds nothing at all**, so §8.2's per-frame bound is unchanged.

Calibration adds at most one observation per update period to the queue of §8.3, which is subject
to that section's bound and its `Lost` accounting like every other observation.

### 12.11 What calibration cannot do

- **It cannot be more sensitive than `floor_amplitude`.** `A_eff ∈ [floor, ceiling]` from sample 0
  (§12.3) and after every update (§12.5), so every window adaptation makes `active` is a window the
  *fixed* profile at `activation_amplitude = floor_amplitude` also makes active. The set of voice
  intervals adaptation can open is a subset of one that does not adapt at all (CAL-9).
- **It cannot turn clipping, DC or a single impulse into a voice interval, at any threshold.**
  §5.3 excludes them before the activation comparison is reached: a constant or DC-biased window
  has variance exactly 0, and an impulsive window fails the `not impulsive` conjunct. No reachable
  threshold changes either. The interval those inputs can open is not merely bounded — it is empty
  (CAL-5a, CAL-5b, CAL-5c). They are additionally ineligible as calibration input (§12.4), so they
  cannot reach the threshold by the indirect route either.
- **It cannot move faster than `max_step_amplitude` per `ceil(U/W)·W` samples.** Crossing the whole
  amplitude domain therefore costs a declared number of samples, not an emergent one.
- **It cannot adapt through open voice for more than `F` samples** (§12.6), so neither can a
  talker's own voice raise the threshold above itself indefinitely, nor can a sudden step in noise
  hold an interval open forever.
- **It cannot overflow.** §12.5's arithmetic is `i32` over a domain whose largest intermediate is
  65,535; §12.2's is the `u64` square root of a quantity §5.2 already proves is at most `2^62`
  (CAL-10). §5.2's width proof is untouched, because `A_eff` never leaves the domain the proof
  assumed for `activation_amplitude`.

### 12.12 Vectors

Reference calibration profile `K8`, layered on §11.1's `P8` (so `W = 160`, `A_0 = 2,048`, hangover
1,600): `calibration_ms = 200` (C = 1,600), `update_ms = 100` (U = 800), `margin_amplitude = 512`,
`floor_amplitude = 256`, `ceiling_amplitude = 8,192`, `max_step_amplitude = 128`,
`freeze_limit_ms = Some(30,000)`. Periods span five windows and end at epoch samples 800, 1,600,
2,400, …, so the first update may apply at the period ending at 1,600.

The pattern every vector below is written in is **160 alternating `±v` samples**, whose deviation is
exactly `v`: 80 samples of each sign give `sum = 0` and `energy = W·v²`, hence `V = W²·v²` and
`isqrt(V) div W = v`.

| ID | Input | Expected |
|---|---|---|
| CAL-1 | `K8`; 70 all-zero frames | 12 updates, at epoch samples 1,600 + 800·k for k in 0..=11, stepping the threshold 2,048 → 1,920 → … → 512 with `observed_floor 0`; then silence on the queue — 512 is `0 + margin` and the target is reached. No voice event at any point |
| CAL-2 | `K8` with `activation_amplitude = 512`; frames of alternating `±64·p` held across the five frames of each period `p`, for `p` in 0..=124 | 120 updates: the period ending at `800·(p+1)` leaves the threshold at `512 + 64·p`, a 64-sample step that never reaches the 128 limit. The threshold tracks the ramp exactly one margin above it, saturates at 8,192 and never exceeds it. No voice event: a ramp the threshold stays ahead of is never called voice |
| CAL-3 | `K8` with `activation_amplitude = 512`, `max_step_amplitude = 1,024`, `freeze_limit_ms = Some(200)` (F = 1,600); 10 zero frames, then 60 frames of alternating `±4,000` | `VoiceStarted { 1,600 }`; then the `Voiced` freeze yields one forced update every 1,600 samples: 1,536 at 3,200, 2,560 at 4,800, 3,584 at 6,400, 4,512 at 8,000, each with `observed_floor 4,000`. The step is then below the threshold, and `VoiceEnded { at_sample: 8,000, cause: Hangover }` closes the interval. A sudden step does not open an unbounded one |
| CAL-4 | `K8` with `freeze_limit_ms = Some(200)`; 50 frames alternating between `±2,048` and `±2,047` — exactly at and just below `A_0` | `VoiceStarted { 0 }`; updates 2,176 at 2,400, 2,304 at 4,000, 2,432 at 4,800, 2,559 at 5,600, all with `observed_floor 2,047`; `VoiceEnded { at_sample: 2,400, cause: Hangover }`. The threshold settles one margin above the quietest eligible window and stops; flapping across a threshold does not flap the events |
| CAL-5a | `K8`; 40 frames of 160 × `+32,767` | no update ever — every window is `clipping` and ineligible, so the outcome is `NoMeasurement` and `observed_floor` is never set. No voice: variance is exactly 0 |
| CAL-5b | `K8`; 40 frames of 160 × `+1,000` | as CAL-5a, by `dc_offset` |
| CAL-5c | `K8`; 30 zero frames with frame 12 replaced by CAP-W4's impulse | the update sequence is identical to 30 zero frames — an impulsive window is excluded before it can define the quiet level |
| CAL-6 | `K8`; 25 zero frames (threshold 1,536, 4 updates), then `reset()` | the threshold stays 1,536 and the update count stays 4; the observed floor and outcome clear; the next epoch's first update is at *its own* sample 1,600, moving 1,536 → 1,408 |
| CAL-6b | CAL-6's prefix, then `declare_format(16,000)` | `Reset { FormatChange { 16,000 } }`; C = 3,200, U = 1,600, F = 480,000; the threshold is still 1,536 |
| CAL-6c | CAL-6's prefix, then a frame flagged `Loss` | as CAL-6: threshold preserved, warm-up re-armed |
| CAL-7 | two `K8` analysers, one `Inbound` fed zeros and one `Outbound` fed alternating `±1,000`, 30 frames each | five updates each, from one starting point to two different places: 1,408 with observed floor 0, and 1,512 — already `1,000 + margin` — with observed floor 1,000. Feeding either the other's direction is refused `DirectionMismatch`, so no floor can cross |
| CAL-8 | each of CAL-1's, CAL-3's, CAL-4's and CAL-2's inputs, replayed into two analysers built from one profile | byte-identical drain sequences, thresholds included — the §4 promise, extended to a value that moves |
| CAL-9 | `K8` and the fixed profile `activation_amplitude = 256`, both fed zeros, clipping, DC, impulses, a sweep and zeros again | for every window, `active` under `K8` implies `active` under the fixed floor profile |
| CAL-10 | `window_ms = 170` at 384,000 Hz (`W = 65,280`), `floor 1`, `ceiling 32,767`, `margin 32,767`, `max_step 32,767`, `freeze_limit_ms = Some(1)`; one frame alternating `i16::MIN`/`i16::MAX`, then one alternating `±32,766` | no overflow and no panic; deviation stays in 0..=32,768; the clipped frame is ineligible and moves nothing; the second saturates the threshold at the 32,767 ceiling rather than past it |
| CAL-11 | `K8`; 40 zero frames with the snapshot taken four times per frame | the drain sequence equals that of a run that never takes a snapshot; a snapshot taken at frame 12 still reads 1,920 afterwards |
| CAL-12 | `P8` with no calibration profile; 60 frames of a rising sweep | no `ThresholdUpdated` ever; the snapshot reports no calibration and the configured 2,048 |
| CAL-C1 | each of `calibration_ms = 0`, `update_ms = 0`, `margin_amplitude` 0 and 32,768, `floor_amplitude = 0`, `ceiling_amplitude = 32,768`, `max_step_amplitude = 0`, `freeze_limit_ms = Some(0)` | `ProfileError` naming that field |
| CAL-C2 | `floor_amplitude = 1,000`, `ceiling_amplitude = 999` | `ProfileError` naming `ceiling_amplitude` |
| CAL-C3 | `activation_amplitude` 100, then 9,000, against `K8`'s 256..=8,192 | `ProfileError` naming `activation_amplitude` — refused, never clamped |
| CAL-C4 | `calibration_ms = u32::MAX` at 8,000 Hz | `ProfileError::DerivedCount` naming `calibration_ms` |
