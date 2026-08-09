# The built-in call-DSP effects and filters

**Status:** normative · **Epic:** `custom-call-dsp` · **Story:** `M-65` ·
**Design:** [custom-call-dsp](../designs/custom-call-dsp.md) ·
**Crates:** `sipx-audio` (`dsp::effects`), `sipx-media` (`dsp::BuiltIn`)

[custom-call-dsp.md](custom-call-dsp.md) §1 says it "deliberately does not define: any effect,
filter or noise-reduction algorithm (`M-65`, `M-66`)". This is the first half of that: the nine
processors this workspace ships, what each one's arithmetic is to the last bit, and what each one
does **not** do.

Where this document and an implementation disagree, this document is right until it is changed
deliberately. It **inherits and does not restate**: the processor interface, the capability
declaration, the parameter vocabulary, the refusal taxonomy, the observation vocabulary, the
execution profiles and the bounds are [custom-call-dsp.md](custom-call-dsp.md)'s, and the graph that
attaches these to a call is [call-dsp-graph.md](call-dsp-graph.md)'s. Nothing here weakens either.
Where it narrows one it says so and says why — and it narrows exactly one thing, §2's channel
declaration.

A second obligation runs through the whole document and is the reason it is this long. Every one of
these processors is a *claim about audio*, and a claim nobody can check is worse than no claim: so
each effect is defined as arithmetic rather than as an adjective, and each one's "does not do"
paragraph is normative alongside its "does". "Smooth", "warm" and "musical" appear nowhere.

## 1. Normative references

- [custom-call-dsp.md](custom-call-dsp.md) — the processor contract. Its §3 inputs, §4 determinism
  obligation, §5 capability, §6 observations, §8 reset/flush/cancel/refusal, §9 bounds and §11
  conformance harness are inherited unchanged.
- [call-dsp-graph.md](call-dsp-graph.md) — the call-local graph. Its §3.2 profile admission and
  §4.3 length narrowing are what these processors are declared against.
- [linear-pcm.md](linear-pcm.md) — the rate domain `1..=384,000` Hz these accept in full.

No third-party implementation is referenced by this document, its vectors or its rationale.

## 2. What every built-in declares

| Field | Every processor here |
|---|---|
| `rates` | `Any` — the whole linear-PCM domain |
| `channels` | `[1, 2]` |
| `max_frame_samples` | 65,536, the contract's own ceiling |
| `scratch_samples` | 0 — none of them needs working memory beyond its own state |
| `length` | `Preserving` |
| `execution` | `ProvenInline`, one frame of deadline, three consecutive misses, `BypassOpen` |

**The channel declaration is the one narrowing.** [custom-call-dsp.md](custom-call-dsp.md) §3.2
admits eight channels; these declare two. Mono and stereo are the call paths this epic serves, eight
is that document's stated headroom, and a capability is what a caller sizes buffers from rather than
a place to be generous. A processor here declaring eight would be claiming a configuration nothing
in this workspace measures. This narrows nothing about the contract: an application-supplied
processor may still declare up to eight, and the harness still runs it there.

`latency_positions`, `tail_positions` and `reset` are per processor and are stated in §5 through §8.
Only [`Stutter`](#7-sipxstutter) declares a non-zero latency or tail.

### 2.1 Identifiers

The stable identifiers, in the order `BUILT_IN_IDS` lists them and `M-67` will register them:

```
sipx.gain · sipx.polarity · sipx.hard_clip · sipx.soft_clip · sipx.bit_crush ·
sipx.stutter · sipx.low_pass · sipx.high_pass · sipx.peaking
```

### 2.2 Smoothing is declared through the parameter schema

A processor whose parameter change is ramped declares `smoothing_positions` with its closed range.
A processor that applies a change whole declares no such parameter. Smoothing is therefore
discovered exactly the way every other property is — by reading the capability — and the absence of
the parameter is as discoverable as its presence. No capability field is added for it, because a
field would have to be answered by every processor including the ones for which interpolation is
meaningless.

Today `sipx.gain` is the only processor with a continuous magnitude for a ramp to mean anything on.
Each of the others states in its own section why its change is applied whole.

### 2.3 Registration and provenance

A stage reaches a graph under `ProvenInline` if and only if it was named through `sipx-media`'s
`BuiltIn` registry ([call-dsp-graph.md](call-dsp-graph.md) §3.2). The processors are ordinary public
types anybody may construct; what the registry grants is **provenance, not access**. The same
processor built by hand and offered at the public plan door is still refused
`ProfileNotAdmissible`, and provenance is recorded per stage, so a built-in beside an
application-supplied processor does not lend it the proven profile.

One thing a parameter cannot express is a processor's *shape*. `Stutter`'s line is its declared
latency and tail, which a graph sizes buffers from before the first frame, so it is a constructor
argument and a field of the registry variant rather than a parameter.

## 3. Arithmetic

Everything here is integer arithmetic at stated widths. There is no floating point in these
processors at all — not in the sample path and not in coefficient derivation — which is a stronger
position than [custom-call-dsp.md](custom-call-dsp.md) §4.6 requires and is taken for the same
reason: a fixture computed on one machine has to stay evidence on another, and integer arithmetic
carries that without anyone reasoning about which floating-point operations a compiler may contract.

Three operations are shared and are defined once:

```
scaled(v, n, d)  = round(v·n / d), rounded half AWAY FROM ZERO
narrow(v)        = v clamped into i32
clamp(v)         = v clamped into i16, and whether it had to be
```

Rounding away from zero rather than toward it makes the arithmetic symmetric about zero: a signal
and its negation produce exactly negated output. Intermediate values are `i64`; nothing wraps.

`clamp` is where [custom-call-dsp.md](custom-call-dsp.md) §6's `Saturated { positions }` comes
from, and its meaning here is narrow on purpose: **output clamped to full scale**, a bound the
arithmetic ran into. An effect limiting to a threshold of its own — a hard clipper at a declared
ceiling — is applying the transfer function the caller asked for and does **not** report
`Saturated`.

### 3.1 Observations, and their order

Every processor here emits at most this, in this order, per frame:

1. `Restarted { cause: Discontinuity { kind } }` — when the frame carried a declared break. The
   break preceded the frame, so its observation does. A processor with no sample memory emits
   nothing here, because it restarted nothing.
2. `ParameterApplied { parameter, at_position }` — one per parameter assigned by the most recently
   accepted set, at the first position of this frame. `configure` has no sink, so this is where a
   set becomes observable; that first position is
   [custom-call-dsp.md](custom-call-dsp.md) §3.6's "one position boundary", read as the boundary a
   caller can see. They are emitted in **declaration order**, not in the order the caller wrote
   them, because §3.6 leaves ordering inside a set to the caller and an observation sequence that
   inherited it would be one more thing two callers could disagree about.
3. `Saturated { positions }` — the number of positions in which any channel clamped to full scale.
4. `PassedThrough` — every output sample of the frame equalled its input sample.

3 and 4 are mutually exclusive: a frame that had to be clamped was not passed through, even where
the clamp happened to return the value it started at.

`reset(cause)` called by a caller has no sink and therefore emits nothing. Only the reset a *frame's
own declared discontinuity* causes is observable, which is the one a downstream consumer could not
otherwise account for.

## 4. Determinism, and what it rests on

Each processor is a pure function of (capability, parameters, format, direction, ordered frames).
None reads a clock, allocates after construction, spawns anything, or touches a random source.
Every duration is counted in **positions** — `smoothing_positions`, `hold_positions`, a delay line
— so an effect behaves identically on a 20 ms and a 60 ms packet, and re-cutting a stream into
different frames produces the same samples. `DSP-K6` is what proves that per processor and
EFFECT-V18 is what proves it across all nine at once.

## 5. Level

### 5.1 `sipx.gain`

```
y = clamp(scaled(x, gain, 1000))
```

| Parameter | Domain | Default |
|---|---|---|
| `gain` | `Ratio { 0 .. 8_000 }` | 1,000 |
| `smoothing_positions` | `Integer { 0 .. 4_096 }` | 0 |

`latency 0 · tail 0 · reset Stateless`.

A change to `gain` moves to its target over `smoothing_positions` positions as a linear ramp whose
value at step *k* is `start + scaled(target − start, k, length)`, so the last step lands on the
target exactly rather than accumulating toward it. At the default length the change applies whole at
the first position of the next frame.

**A transition survives a reset**, and that is why this processor declares `Stateless`: a ramp is
parameter state, [custom-call-dsp.md](custom-call-dsp.md) §8.1 keeps declared parameters across a
reset, and this processor holds no sample memory for a reset to discard. A reset of it is therefore
unobservable, which is what `Stateless` claims.

**Does not:** invert (§5.2), limit or compress (§6), follow the signal — there is no envelope, no
detector, no automatic level — or dither.

### 5.2 `sipx.polarity`

```
y = clamp(−x)   when `inverted`, else x
```

| Parameter | Domain | Default |
|---|---|---|
| `inverted` | `Flag` | false |

`latency 0 · tail 0 · reset Stateless`.

`i16::MIN` has no positive counterpart, so it clamps to `i16::MAX` and the frame reports
`Saturated { positions: 1 }` — the one place two's-complement asymmetry is audible, made visible
rather than wrapped around.

**Does not:** ramp or cross-fade. Interpolating between a signal and its negation is a cross-fade
through silence, which is a different effect with a different name, so a polarity change is applied
whole and this processor declares no `smoothing_positions`. It also does not act per channel: on a
stereo stream both channels invert together.

## 6. Waveshaping

### 6.1 `sipx.hard_clip`

```
y = x clamped to −ceiling ..= ceiling
```

| Parameter | Domain | Default |
|---|---|---|
| `ceiling` | `Integer { 0 .. 32_767 }` | 32,767 |

`latency 0 · tail 0 · reset Stateless`.

The clip is symmetric, so at the default ceiling `i16::MIN` becomes `−32_767`: a clipper that let
one value of one polarity through would not be symmetric. Clipping to a declared ceiling is **not**
`Saturated` (§3).

**Does not:** apply drive or make-up gain, look ahead, or soften the corner. Put a `sipx.gain` stage
before it for drive and after it for level; use §6.2 when the corner is what matters.

### 6.2 `sipx.soft_clip`

On the magnitude `d = |x|`, with `t = threshold` and `c = 32_767`:

```
y = d                                    for d ≤ t
y = d − (d − t)² / (4·(c − t))           for d > t      -- integer division, truncated
```

with the input's sign re-applied.

| Parameter | Domain | Default |
|---|---|---|
| `threshold` | `Integer { 0 .. 32_766 }` | 16,384 |

`latency 0 · tail 0 · reset Stateless`.

The curve meets the identity at `t` with a matching slope, so there is no corner where the knee
begins, and its maximum is `c − (c − t)/4` at full-scale input.

**Does not:** reach full scale, and therefore never reports `Saturated`. It *reduces* peak level
rather than holding it, which is the price of the soft corner. No drive, no make-up gain, no attack,
no release, no envelope, no look-ahead — it is a memoryless transfer function and not a compressor —
and its knee width is fixed by `threshold` rather than being a second parameter.

### 6.3 `sipx.bit_crush`

Two reductions, in this order:

```
held  = the input captured once every `hold_positions` positions, per channel
y     = (held >> (16 − bits)) << (16 − bits)
```

| Parameter | Domain | Default |
|---|---|---|
| `bits` | `Integer { 1 .. 16 }` | 16 |
| `hold_positions` | `Integer { 1 .. 256 }` | 1 |

`latency 0 · tail 0 · reset ClearsState`. `retained()` is 0: a held sample has already been
emitted, so nothing is owed.

The shift is arithmetic, which makes the quantiser mid-tread and biased toward negative infinity: a
quiet negative signal quantises to `−2^(16 − bits)` rather than to silence. At 16 bits the grid is
the sample grid and the reduction is the identity.

**Does not:** dither, noise-shape or filter. Quantisation noise is correlated with the signal and
aliases fold back unfiltered, which is what a bit crusher is for; if that is not wanted, this is the
wrong processor rather than the wrong settings. It does not resample — `hold_positions` repeats
samples at the stream's own rate and the frame's position count is unchanged. The hold counter runs
in positions, so it is independent of framing.

## 7. `sipx.stutter`

A circular line of exactly `d` positions, fixed at construction and declared as both
`latency_positions` and `tail_positions`. Per position, with `i` the read cursor:

```
y      = line[i]
line[i] = x                    unless `repeat`
i      = (i + 1) mod d
```

`d = 0` is a pass-through. `flush` writes the `retained()` positions the line still owes, oldest
first, and leaves `retained() = 0`. A reset discards them instead, because retained audio belongs to
an epoch that no longer exists.

| Parameter | Domain | Default |
|---|---|---|
| `repeat` | `Flag` | false |

`latency d · tail d · reset ClearsState`, `d` in `0..=4_096`. The maximum is
[call-dsp-graph.md](call-dsp-graph.md) §4's own default `retained_tail_positions`, reused, so a line
at its maximum is exactly what a default-bounded graph still attaches.

### 7.1 An intentional glitch is not a defect, and cannot be mistaken for one

This is the obligation the effect exists to keep, and it is met by three facts living in three
types across two streams:

| The fact | How it appears | Who emits it |
|---|---|---|
| the application asked for a repeat | `ParameterApplied { parameter: "repeat", at_position }` | the processor, in the observation queue |
| the call's timeline broke | `Restarted { cause: Discontinuity { kind } }` | the processor, in the observation queue |
| a stage was late, refused or was bypassed | `GraphTransition::Bypassed { cause }` | the graph, in the transition queue |

No processor can emit the third — [custom-call-dsp.md](custom-call-dsp.md) §6 keeps deadline misses
and bypasses out of the observation vocabulary deliberately — and this processor never emits the
second for a repeat. **No document, event, metric or API in this stack presents an overload defect
as an effect**, and none counts this effect as an overload defect.

**Does not:** fade, cross-fade or window the loop point, so a repeat whose line does not hold a whole
number of periods clicks at the seam. Does not vary its line while running: latency and tail are
declarations a graph has already sized buffers from. Does not feed back — the line is written from
the input and never from its own output, so there is no repeat count, no decay and no unbounded
growth.

## 8. Filters

All three are built from one first-order section and nothing else. That is a stability decision: a
one-pole with a coefficient in `0..1` has its pole strictly inside the unit circle for **every**
admissible parameter value, so stability is a property of the shape rather than a range somebody has
to keep checking. A resonant second-order section would put stability at the mercy of how its
coefficients round in fixed point, and this story does not measure that.

### 8.1 The section

```
v   = scaled(x·32768 − s, G, 32768)
y   = v + s
s   = y + v
out = scaled(y, 1, 32768)
```

`H(z) = G(1 + z⁻¹) / (1 + (2G − 1)z⁻¹)`: unity at DC for every `G`, pole at `1 − 2G`. At `G = 0.5`
— a cutoff of exactly one quarter of the rate — the pole is at the origin and the section is the
two-tap average `(x[n] + x[n−1])/2`, which is why every filter vector in §9 runs there and can be
checked on paper.

**The state is carried 15 bits wider than the samples, and that is load-bearing.** A section whose
state were kept at sample resolution would stop moving as soon as the rounded update fell below one
sample — a dead band of `16384/G` samples, which at a 300 Hz cutoff on a narrowband call is four LSB
of permanent DC offset in a filter whose job is removing DC. At Q15 that dead band is below the
resolution of the output, and DC settles exactly.

### 8.2 The coefficient is a shipped table

`G` for a cutoff `f` at a rate `r` is read from a 65-entry table whose entry *k* is
`round(32768 · g/(1+g))` for `g = tan(π·k/128)`, with linear interpolation in integer arithmetic
between neighbours at index `f·128/r`. [custom-call-dsp.md](custom-call-dsp.md) §4.6 forbids a
transcendental function in coefficient derivation *at run time*; the table is how a processor
"derives it from its declared parameters through a table it ships", and the coefficient a rate and a
cutoff produce is exactly this table and this interpolation and nothing else.

A cutoff at or above **0.45 of the sample rate** is clamped to it rather than refused. The parameter
domain is closed independently of the rate — it has to be, because a capability is declared before a
format is — so a rate-dependent refusal would make one configuration valid on an 8,000 Hz call and
invalid on the 16,000 Hz one beside it.

### 8.3 `sipx.low_pass` and `sipx.high_pass`

```
low_pass  : y = section(x)
high_pass : y = x − section(x)
```

| Parameter | Domain | Default |
|---|---|---|
| `cutoff_hz` | `Integer { 1 .. 192_000 }` | 3,400 (low-pass) · 300 (high-pass) |

`latency 0 · tail 0 · reset ClearsState`. Latency is 0 because a one-pole's group delay is not a
whole-position offset a graph could sum; `tail` is 0 because the section retains history rather than
undelivered audio, so there is nothing for `flush` to owe.

**Do not:** resonate, offer a `Q`, an order or a steeper slope. They are not anti-aliasing or
reconstruction filters — nothing here resamples — and they are not linear phase. The high-pass is
not a noise gate and not a rumble *detector*: it removes low frequencies whether or not anything
down there is a problem.

### 8.4 `sipx.peaking`

```
band = section(2·centre_hz) − section(centre_hz/2)
y    = clamp(x + scaled(band, band_gain − 1000, 1000))
```

| Parameter | Domain | Default |
|---|---|---|
| `centre_hz` | `Integer { 1 .. 192_000 }` | 1,000 |
| `band_gain` | `Ratio { 0 .. 4_000 }` | 1,000 |

`latency 0 · tail 0 · reset ClearsState`.

A `band_gain` of 1.0 is the exact identity. DC and the folding frequency are untouched at every
setting, because both sections agree there and the difference is zero — which is the arithmetic
statement of "a peaking filter changes a band and not a level".

**Does not:** make `band_gain` the magnitude response at the centre frequency. It is the scale
applied to the extracted band, and a first-order pair's band does not reach unity, so the resulting
peak is smaller than the number suggests. The parameter is named after what it multiplies rather
than after what a sweep would measure; `X-109` owns the measurement that would let anyone claim
otherwise. There is no `Q` and no bandwidth parameter — the width is two octaves and fixed — no
shelving mode and no cascade. A centre frequency whose upper edge would pass 0.45 of the rate is
clamped there, so a band pushed past the folding frequency narrows and then vanishes rather than
folding back.

## 9. Vectors

Unless a vector says otherwise it runs on `D8`: `Inbound`, 8,000 Hz, one channel, and the parameter
set named in the row applied before the stream.

| ID | Input | Expected |
|---|---|---|
| EFFECT-V1 | `gain = 2.0` on `[1, −1, 16384, −16384, 32767, −32768]` | `[2, −2, 32767, −32768, 32767, −32768]`, `ParameterApplied { "gain", 0 }` then `Saturated { positions: 3 }` |
| EFFECT-V2 | `gain = 1.5` on `[1, 2, 3, −1, −2, −3]` | `[2, 3, 5, −2, −3, −5]` — rounding is symmetric about zero |
| EFFECT-V3 | default gain on `[0, 1, −1, 32767, −32768]` | the same five samples and one `PassedThrough` |
| EFFECT-V4 | `gain = 2.0`, `smoothing_positions = 4` on `[1000]×6` | `[1250, 1500, 1750, 2000, 2000, 2000]`, and the same however the stream is cut |
| EFFECT-V5 | `inverted` on `[0, 1, −1, 32767, −32768]` | `[0, −1, 1, −32767, 32767]` and `Saturated { positions: 1 }` |
| EFFECT-V6 | `ceiling = 1000` on `[0, 500, 1000, 1001, −1000, −1001, 32767, −32768]` | `[0, 500, 1000, 1000, −1000, −1000, 1000, −1000]` and **no** `Saturated` |
| EFFECT-V7 | `threshold = 16384` on `[0, 16384, 20000, 32767, −32768]` | `[0, 16384, 19801, 28672, −28672]` and no `Saturated` |
| EFFECT-V8 | `bits = 8` on `[0, 255, 256, 32767, −1, −32768]`; `hold_positions = 2` on `[100, 200, 300, 400]` | `[0, 0, 256, 32512, −256, −32768]`; `[100, 100, 300, 300]`, and the same across any cutting |
| EFFECT-V9 | `d = 2` on `[1,2,3]` then `[4,5,6]`, then `flush` | `[0,0,1]`, `[2,3,4]`, `retained() = 2`, flush `[5,6]` and `retained() = 0` |
| EFFECT-V10 | `d = 3` fed `[1..6]`, then `repeat` on for two frames, then off | `[0,0,0,1,2,3]`; `[4,5,6]` twice with one `ParameterApplied { "repeat", 6 }`; then `[4,5,6]` again as the delay resumes |
| EFFECT-V11 | `d = 0` with `repeat` set; `d = 4096`; `d = 4097` | pass-through; a full line of silence while it fills; the constructor refuses |
| EFFECT-V12 | `cutoff_hz = 2000` on `[32767, 0, 0, 0, 0]` and on `[1000]×4` | `[16384, 16384, 0, 0, 0]`; `[500, 1000, 1000, 1000]` |
| EFFECT-V13 | `cutoff_hz = 2000` on `[10000, −10000, …]` and on `[10000]×5` | `[5000, 0, 0, 0, 0]`; `[5000, 10000, 10000, 10000, 10000]` — an exact zero at the folding frequency and exact unity at DC |
| EFFECT-V14 | the same, high-pass | `[16383, −16384, 0, 0, 0]`; `[5000, 0, 0, 0, 0]` |
| EFFECT-V15 | `cutoff_hz = 192000` at 8,000 Hz on 512 positions of full-scale alternation | clamped to the guard; the tail is within one LSB of silence |
| EFFECT-V16 | `centre_hz = 1000`, `band_gain = 4.0` on settled DC and on settled alternation; default `band_gain` on any input | the input, unchanged, in all three |
| EFFECT-V17 | `centre_hz = 1000` on `[10000, 0×7]` at `band_gain = 2.0` and at `band_gain = 0` | `[13341, 2232, −1849, −1236, −826, −552, −369, −246]` and `[6659, −2232, 1849, 1236, 826, 552, 369, 246]`; the two sum to `2x` position for position |
| EFFECT-V18 | every built-in, one stream cut at 7, 1, 13, 5 and 22 positions against one frame | identical samples and identical flushed tails |
| EFFECT-V19 | every built-in, against `DSP-K1`..`DSP-K12` | no failure; `DSP-K9` `Unproven` and nothing else |

### 9.1 What `DSP-K9` being unproven means here

[custom-call-dsp.md](custom-call-dsp.md) §9.1 reports the heap component of a processor's own state
as `Unproven` for **every** processor, because `unsafe_code` is forbidden workspace-wide and no
counting allocator can be installed. That is the only unproven check these processors produce, and
it is unproven by construction rather than by anything they do: the scratch declaration is held
exactly, the sink is never overrun, and each one's inline size is inside its declared
`state_bytes`. EFFECT-V19 asserts the unproven set is *exactly* `DSP-K9`, so a check quietly
slipping from passed to unproven is a test failure rather than a footnote.
