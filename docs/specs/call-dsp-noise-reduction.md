# Interchangeable call-DSP noise reduction

**Status:** normative · **Epic:** `custom-call-dsp` · **Story:** `M-66` ·
**Design:** [custom-call-dsp](../designs/custom-call-dsp.md) ·
**Crates:** `sipx-audio` (`dsp::noise`), `sipx-media` (`dsp::BuiltIn`)

[custom-call-dsp.md](custom-call-dsp.md) §1 says it "deliberately does not define: any effect,
filter or noise-reduction algorithm — the processors this workspace ships under this contract are
[call-dsp-effects.md](call-dsp-effects.md) (`M-65`) and noise reduction is `M-66`". This is the
second half of that.

Where this document and an implementation disagree, this document is right until it is changed
deliberately. It **inherits and does not restate**: the processor interface, the capability
declaration, the parameter vocabulary, the refusal taxonomy, the observation vocabulary, the
execution profiles, the bounds and the conformance harness are
[custom-call-dsp.md](custom-call-dsp.md)'s, and the graph that attaches a reducer to a call is
[call-dsp-graph.md](call-dsp-graph.md)'s. Nothing here weakens either.

Two obligations shape the whole document, and they are why it is this long.

**The interface is the deliverable and the algorithm is one instance of it.** What ships is a
*declaration* a noise reducer makes and a trait it implements, plus one implementation behind them.
A second implementation with different arithmetic, a different warm-up and a different host
requirement must be substitutable without the call layer being changed or even being told, and §4
states what that has to mean before it can be claimed.

**A suppressor is described by what it damages, not by an adjective.** "Clean", "clear" and
"studio" appear nowhere. §5.6 states what the shipped baseline removes, what it damages while
removing it, and the four conditions under which it makes speech *worse* than leaving it alone. That
paragraph is normative alongside the arithmetic, and it is the part a caller has to read.

## 1. Normative references

- [custom-call-dsp.md](custom-call-dsp.md) — the processor contract. Its §3 inputs, §3.6 parameter
  vocabulary, §4 determinism obligation, §5 capability, §6 observations, §8
  reset/flush/cancel/refusal, §9 bounds, §10 boundaries and §11 conformance harness are inherited
  unchanged.
- [call-dsp-effects.md](call-dsp-effects.md) — `M-65`'s built-ins. Its §3 shared integer arithmetic
  and its §8.1 one-pole section are reused rather than re-derived, and its §2 channel narrowing is
  taken for the same reason.
- [call-dsp-graph.md](call-dsp-graph.md) — the call-local graph. Its §3.2 profile admission, §4.3
  length narrowing and §8 per-call ownership are what a reducer is declared against.
- [call-audio-processing.md](call-audio-processing.md) — the deterministic analysis contract, which
  owns voice activity detection. §2 states the boundary between the two.
- [linear-pcm.md](linear-pcm.md) — the rate domain and its typed `UnsupportedSampleRate` refusal.

No third-party implementation is referenced by this document, its vectors or its rationale.

## 2. What "noise reduction" names here, and what it does not

A **noise reducer** under this document is a [custom-call-dsp.md](custom-call-dsp.md) processor
that attenuates part of its input on an estimate it derives from that same input, and declares §3's
extra facts about how it does so. It is the same trait, the same frames, the same refusals and the
same harness as every other processor; what it adds is a declaration, not a door.

Four neighbours are named apart because conflating any of them with this one would be a claim about
a call that nobody could check:

- **Voice activity detection** is [call-audio-processing.md](call-audio-processing.md)'s and stays
  there. A reducer never emits, redefines, delays or suppresses a VAD observation, and a VAD's
  events are unchanged by whether a reducer is attached. A reducer may *consume* activity — as a
  declared parameter the caller sets, per §3.3, never as a call into an analyser — and
  [custom-call-dsp.md](custom-call-dsp.md) §10 already forbids the other direction by name.
- **Echo cancellation** is not here and is not implied. An echo canceller needs the far-end
  reference signal, which is the *other* direction of the call; a processor under this contract is
  bound to one direction at `prepare` and cannot see the other. Nothing in this document reduces
  echo, and a reducer attenuating a returning talker is attenuating it as noise, badly.
- **Speech recognition and synthesis** are the provider contracts in `docs/specs/speech-providers.md`
  and are not here. A reducer produces samples, never text, never a confidence and never a
  transcript.
- **Automatic gain control, dereverberation and source separation** are not here either. A reducer
  never raises a level: every gain it applies is at most unity (§5.3), so nothing it does can make a
  quiet talker louder.

## 3. The declaration

A processor already declares its rates, channels, frame ceiling, scratch, latency, tail, length
policy, reset behaviour, execution policy and parameter schema — all of it
[custom-call-dsp.md](custom-call-dsp.md) §5's `DspCapability`, and none of it restated here. A noise
reducer declares that **and three more facts**, and the three exist because a caller choosing
between two reducers cannot answer these from `DspCapability` alone:

```
NoiseReduction ::= { processor:         DspCapability,
                     warm_up_positions: 0..=65,536,
                     activity:          ActivityInput,
                     host:              HostRequirement }

ActivityInput   ::= Ignored | Optional { parameter }
HostRequirement ::= PortableInteger | Device { id }
```

`warm_up_positions` is the ceiling on `latency_positions` and `tail_positions`,
[custom-call-dsp.md](custom-call-dsp.md) §5's own `MAX_LATENCY_POSITIONS`, reused. A warm-up is not
a buffer, but it is a position count a caller reasons about beside latency and tail, and a second
ceiling would be a second number to keep consistent for no gain.

### 3.1 Warm-up is a position count and never a duration

`warm_up_positions` is how many positions of an epoch a reducer consumes before its estimate is
usable, and during which it is obliged to be the exact identity (§5.4). It opens at `prepare`,
re-opens at every reset and at every discontinuity — that is [custom-call-dsp.md](custom-call-dsp.md)
§8.1's epoch and not a second notion of one.

It is a **fixed position count and not a duration**, which has a consequence this document states
rather than hides: the same reducer on a 16,000 Hz call warms up in half the wall-clock time it
takes on an 8,000 Hz call. That follows from §3.4 of the processor contract — sample position is the
only clock a processor has — and the remedy, where a caller wants equal wall-clock behaviour across
rates, is to configure the rate-dependent parameters per rate, which is §4.1's "asks its declared
parameters, not the machine" used for what it is for.

### 3.2 `HostRequirement` is a declaration, never a probe

[custom-call-dsp.md](custom-call-dsp.md) §4.1 forbids device enumeration outright: "a processor that
wants to know whether a machine has a particular instruction set asks its declared parameters, not
the machine". So a reducer states what it *needs* and the caller states what it *has*; nothing here
asks the machine anything.

- `PortableInteger` — integer arithmetic and nothing else. No instruction-set extension, no
  accelerator, no coprocessor, no device, and no model of any kind. Every machine this crate
  compiles for runs it identically, which is the same sentence as "its vectors are portable".
- `Device { id }` — the implementation needs a named host facility. `admits_host(devices)` refuses
  with `DeviceUnavailable { required }` when the caller's list does not carry that name, and that
  refusal is §6's typed refusal for an unsupported device.

A `Device` requirement is *the caller's* to satisfy or to refuse. Nothing in this workspace declares
one today; the variant exists so that an implementation which does can say so and be refused
cleanly, rather than discovering it at the first frame on the media worker.

### 3.3 `ActivityInput` names a parameter or names nothing

`Optional { parameter }` claims that the reducer consumes a voice-activity hint, and the claim is
checkable rather than prose: `validate()` requires that `parameter` appear in the processor's own
declared schema with domain `Flag`. A reducer whose declaration says it consumes activity, and whose
schema has no such flag, is refused at the declaration — before a caller wires a VAD to a parameter
that is not there.

`Optional` also means *optional*: a reducer under it MUST be deterministic and useful with the flag
never set, and MUST NOT refuse a frame for want of one. `Ignored` claims the reducer never consumes
activity, and a caller wiring a VAD to it is wiring it to nothing.

The hint arrives as a parameter set between frames, so its granularity is one frame and its
observability is [call-dsp-effects.md](call-dsp-effects.md) §3.1's `ParameterApplied`. A caller that
sets it every frame gets one observation every frame — that is the caller's choice made visible,
not a defect.

### 3.4 What `validate()` checks

`NoiseReduction::validate()` is [custom-call-dsp.md](custom-call-dsp.md) §5's `validate()` plus this
document's three fields, and it refuses before a caller sizes anything:

| Declared | Refusal |
|---|---|
| a processor declaration §5 does not admit | `Capability(CapabilityError)`, that document's own error, carried rather than re-minted |
| `warm_up_positions` above 65,536 | `WarmUp { positions }` |
| `Optional { parameter }` naming a parameter the schema does not declare | `ActivityParameterUndeclared { parameter }` |
| `Optional { parameter }` naming a parameter whose domain is not `Flag` | `ActivityParameterNotAFlag { parameter }` |
| `Device { id }` with an empty `id` | `EmptyDeviceIdentifier` |

## 4. Interchangeability, and what proves it

"Interchangeable" is the word this story turns on, so it is defined as an obligation on the code
rather than as an adjective on the design:

1. **One trait.** `NoiseReducer: FrameProcessor` adds exactly one method,
   `noise_reduction() -> NoiseReduction`. There is no second lifecycle, no second parameter
   language and no second refusal type: everything a reducer does to a frame is the processor
   contract's, unchanged.
2. **No crate-private door.** The trait, the declaration and every type they name are public, and
   the workspace's own baseline implements them through the same public surface an application
   reaches. This is [custom-call-dsp.md](custom-call-dsp.md) §10's rule applied one level down.
3. **The call layer names the trait and not the implementation.** Attaching a reducer to a call is
   [call-dsp-graph.md](call-dsp-graph.md)'s `GraphPlan`, which takes a `FrameProcessor`. A reducer
   reaches it as one, so the graph, the seam, the session and the application all remain unable to
   tell which implementation is installed.
4. **Substitution is proved by running two.** It is normative that this workspace's test suite
   contains a second reducer, written **from outside `sipx-audio`**, whose arithmetic, warm-up,
   reset behaviour and host requirement all differ from the baseline's, and that both are driven
   through one generic harness and one `dyn NoiseReducer` list which the driving code cannot
   distinguish between. A single implementation behind an interface proves that the interface
   compiles.
5. **Both pass the same conformance run.** `DSP-K1`..`DSP-K12` are run against every reducer, and
   the unproven set is asserted to be *exactly* `DSP-K9`, for the reason
   [call-dsp-effects.md](call-dsp-effects.md) §9.1 gives: the heap component of a processor's own
   state cannot be counted in a workspace that forbids `unsafe_code`, so it is reported by name
   rather than passed. A reducer whose unproven set grows is a test failure and not a footnote.

Provenance does not travel with the trait. [call-dsp-graph.md](call-dsp-graph.md) §3.2 grants
`ProvenInline` only through `sipx-media`'s `BuiltIn` registry, so the workspace baseline reaches a
graph under that profile and an application-supplied reducer — this document's fixture included —
does not, however identical its behaviour. That is the intended asymmetry: `ProvenInline` names
evidence in this repository's gate, and implementing a trait adds none.

## 5. The baseline: `sipx.subband_suppressor`

One implementation ships. It is named after its mechanism rather than after its category, because
the category is the trait and a second implementation gets a second name.

| Field | Declared |
|---|---|
| `id` | `sipx.subband_suppressor` |
| `rates` | exactly `8,000 · 16,000 · 32,000 · 48,000` Hz |
| `channels` | `[1, 2]` |
| `max_frame_samples` | 65,536, the contract's own ceiling |
| `scratch_samples` | 0 |
| `latency_positions` | 0 |
| `tail_positions` | 0 |
| `length` | `Preserving` |
| `reset` | `ClearsState` |
| `execution` | `ProvenInline`, one frame of deadline, three consecutive misses, `BypassOpen` |
| `warm_up_positions` | 1,024 |
| `activity` | `Optional { parameter: "voice_active" }` |
| `host` | `PortableInteger` |

**The rate list is closed, and that is the point.** The band edges below are fixed in hertz and the
estimator's constants are counted in positions; the four rates listed are where the two agree with
the speech this is shaped for. A 44,100 Hz stream is **refused** with
[custom-call-dsp.md](custom-call-dsp.md) §8.3's `RateNotAccepted` rather than resampled or quietly
accepted: §10 forbids a processor changing the rate, and accepting a rate this was never measured at
would be a quality change nobody asked for (§6).

**Zero declared latency is a real zero and it has a price.** There is no lookahead, no analysis
block and no overlap-add, so the output position carrying an input position is that same position.
That is why §5.6's transient row reads as it does: a suppressor that cannot see forward cannot know
a transient is coming, and every transient therefore arrives before any decision about it.

`ExecutionPolicy`'s `BypassOpen` is the honest default for this processor and is stated as a
decision: a reducer's absence is a quality regression and not a policy breach, so a call that cannot
afford the reducer's budget should keep carrying audio that sounds worse rather than stop carrying
audio. A caller whose policy is the other way sets `TerminateClosed`, and
[custom-call-dsp.md](custom-call-dsp.md) §7.4 is where that decision is described.

### 5.1 The band split

Two [call-dsp-effects.md](call-dsp-effects.md) §8.1 one-pole sections per channel, at **500 Hz** and
**2,000 Hz**, read from that document's shipped coefficient table with its interpolation and its
0.45-of-the-rate guard. Nothing new is derived and no transcendental is evaluated at run time.

```
s₁ = section(x, 500 Hz)          s₂ = section(x, 2,000 Hz)
b₀ = s₁                          -- low:  hum, rumble, the bottom of the first formant
b₁ = s₂ − s₁                     -- mid:  most of the vowel energy
b₂ = x − s₂                      -- high: fricatives, and most hiss
```

`b₀ + b₁ + b₂ = x` **exactly**, by construction and not by luck: the upper two bands are defined by
subtraction, so whatever the sections rounded to cancels. That identity is what makes §5.4's
warm-up bit-exact and what makes an all-unity gain the exact identity rather than approximately one.

The split is **first order**, which is 6 dB per octave and a wide overlap. Three bands with skirts
that broad cannot separate a fricative from hiss, and §5.6 says so rather than this document
implying otherwise by calling them bands.

### 5.2 The estimator

Per band and per channel, two integers at Q8 — the sample magnitude times 256 — and nothing else.

```
m     = |bᵢ|
env   = env + step(m·256 − env, 8)                        -- a magnitude follower
creep = max(1, floor / adaptation_positions)
floor = min(floor + creep, env)
```

`step(d, n)` is `d / n` when that is non-zero, and otherwise the sign of `d`: an integer follower
whose rounded update fell below one unit would stop short of its target forever, and the sign
carries it the rest of the way in bounded time.

Both numbers are **seeded from the first position of every epoch** — `env = m·256` with no
smoothing, and `floor = env` — so the estimator does not spend its warm-up climbing out of a zero it
was never at. The seeding is unconditional, including under an activity hint (§5.5): the hint
governs *adaptation*, and there is nothing to adapt from before the epoch has a first position.

`floor` is a **recursive minimum with an upward creep**: it descends to any new minimum in one
position and rises by a fixed fraction of itself otherwise, capped at `env` so it can never exceed
the signal it is a floor for. `adaptation_positions` is that fraction's denominator, so an
under-estimate recovers over a stated number of positions rather than at a wall-clock rate.

**At `adaptation_positions = 1` the floor reaches the envelope within a handful of positions**, at
which point every signal is its own noise floor, every band sits at `min_band_gain` and nothing ever
recovers from anything. That is a real property of the recurrence rather than a defect, it is what
§9's sample-exact vectors use to reach a settled state in a few positions, and it is not a setting
for a call.

This is the whole estimator. It is a *statistical* floor tracker and not a model of anything: it has
no notion of speech, no notion of a speaker, no spectrum finer than the three bands above, and no
memory beyond the two numbers per band. What it can tell apart is "louder than it has recently been"
from "as quiet as it has recently been", and every claim in §5.6 follows from that being all it can
do.

### 5.3 The gain

Per band, in thousandths, computed from the estimate and then slewed:

```
target = 1000                                             when env ≤ 0
target = clamp(1000 − over_subtraction·floor / env,  min_band_gain, 1000)
gain   = gain moved toward target by at most ceil(1000 / gain_slew_positions) per position
y      = clamp(Σᵢ scaled(bᵢ, gainᵢ, 1000))
```

`scaled` and `clamp` are [call-dsp-effects.md](call-dsp-effects.md) §3's, unchanged, so the rounding
is symmetric about zero and full scale saturates rather than wrapping. Every gain is in `0..=1000`:
a reducer never amplifies (§2).

With `gain_slew_positions = 0` a gain change applies whole at the position the estimate demanded it.
The parameter is **not** named `smoothing_positions`: [call-dsp-effects.md](call-dsp-effects.md) §2.2
gives that name to a ramp on a *parameter* change, and this is a slew on a derived gain. One name
for two mechanisms is how a caller ends up configuring the wrong one.

| Parameter | Domain | Default |
|---|---|---|
| `min_band_gain` | `Ratio { 0 .. 1_000 }` | 250 |
| `over_subtraction` | `Ratio { 1_000 .. 4_000 }` | 1,500 |
| `adaptation_positions` | `Integer { 1 .. 65_536 }` | 4,000 |
| `gain_slew_positions` | `Integer { 0 .. 4_096 }` | 64 |
| `voice_active` | `Flag` | false |

`min_band_gain` is the **most** the suppressor may attenuate a band, and the default of 250 — a
quarter of the amplitude, 12.04 dB — is a deliberate floor rather than a limit of the arithmetic.
Attenuating further makes the residual pump audibly against the speech and makes every pause a hole;
`0` is available and is that hole, on request.

`over_subtraction` above 1.0 subtracts more than the estimate, which removes more of the noise and
takes more of the speech with it. At the default of 1.5, any band whose floor has reached half its
envelope is already at `min_band_gain`.

### 5.4 Warm-up is the exact identity

For the first `warm_up_positions` positions of an epoch, every band gain is held at 1,000 and the
output is **bit-exact the input** — §5.1's identity, so the frame also earns
[custom-call-dsp.md](custom-call-dsp.md) §6's `PassedThrough`. The estimator runs throughout.

The warm-up exists because an epoch may open in the middle of a syllable. A floor seeded from speech
is a floor that would attenuate speech, and 1,024 positions is how long this implementation gives it
to find a minimum before it is allowed to act on one. It is not a fade-in, and it is not a
convergence guarantee: 1,024 positions of continuous speech leave the floor exactly as wrong as they
found it, and the suppressor then acts on it.

A reset, a `FormatChange` and every `Loss`, `Overflow` or `Realign` discontinuity restart the
warm-up along with the rest of the state. That is §5.6's fifth row, and it is a real cost on a lossy
call.

### 5.5 The optional activity input

While `voice_active` is set, `floor` does not update; `env` does, and the gain is derived from both
as usual. The reducer is fully deterministic and fully functional with the flag never set — that is
what `Optional` obliges (§3.3) — and what the flag buys is the one thing the estimator cannot do for
itself: refusing to adapt its floor upward into speech it cannot recognise.

It buys nothing else, and in particular it is not a gate: a frame is neither passed nor silenced on
the strength of the flag, and a reducer under this document never emits a voice-activity event of
its own (§2).

A wrong hint is worse than no hint, both ways, and §5.6's last row says how.

### 5.6 What it removes, what it damages, and when it makes speech worse

**What it removes.** Additive noise that is stationary over `adaptation_positions` and quieter than
the speech above it, in whichever of the three bands it occupies: mains hum and rumble in the low
band, fan, road and cabin noise across the low and mid bands, tape-like hiss in the high band. In a
band that is *only* noise, the gain settles at `min_band_gain` and the attenuation is exactly that
ratio — 12.04 dB at the default, not more.

**What it damages, whenever it is removing anything.** The gain is applied to a whole band and not
to the noise inside it, so everything sharing that band is attenuated with it:

- the **low** band carries the first formant of a low-pitched voice, so a talker over hum loses
  chest and body and sounds thin and distant;
- the **high** band carries fricatives, and /s/, /f/ and /ʃ/ are attenuated together with the hiss
  they physically resemble — the speech sounds lisped, and consonant intelligibility falls exactly
  where a listener needs it most;
- the **overlap** between first-order bands is wide (§5.1), so a gain change in one band is audible
  in its neighbours as a broad tilt rather than a narrow cut;
- every **onset** is smeared: after a pause the gain has fallen to `min_band_gain`, and the first
  `gain_slew_positions`-worth of the returning syllable is attenuated by however far it had fallen;
- **stereo channels are estimated independently**, so a noise source that is not centred can be
  attenuated by different amounts on the two channels and its image moves as the gains diverge.

**When it makes speech worse than doing nothing at all.** Five conditions, none of them exotic:

1. **Speech quieter than the tracked floor.** The estimator's whole vocabulary is "louder than it
   has recently been". A talker who drops to a whisper, or who is quieter than the room, is
   indistinguishable from the room and is attenuated as the room. Turning `over_subtraction` up
   makes this worse and nothing about it better.
2. **Non-stationary noise.** A door, a keyboard, a cough and a plosive are faster than `env` follows
   and far faster than `floor` creeps, so they pass through essentially unattenuated — and then they
   raise the envelope, which raises the floor, which attenuates the *speech that follows them*. The
   audible result is that the interfering sound survives and the sentence after it does not. Zero
   declared latency (§5) is precisely why: nothing here can see a transient coming.
3. **Overlapping speech.** Background babble that is stationary over `adaptation_positions` is a
   floor as far as this estimator is concerned, so a second talker or a room of them pulls the floor
   up and the near talker down with it. Nothing here separates sources, and a reducer is the wrong
   tool for a room with two people talking in it.
4. **A restarting epoch.** Every reset and every discontinuity discards the estimate and re-opens
   the warm-up (§5.4), during which the reducer is the exact identity. A call losing packets
   frequently enough spends its time warming up and carrying unmodified noise, and no configuration
   changes that — the alternative would be applying an estimate that belongs to an epoch that no
   longer exists.
5. **A wrong `voice_active` hint.** Pinned true, the floor never adapts and the suppressor freezes
   on whatever it had at the moment it stopped, including a floor seeded from noise that has since
   gone. Pinned false during continuous speech, the floor climbs into the speech and the speech is
   then subtracted from itself. A hint from a detector that is wrong half the time is worse than the
   flag never being set (§5.5).

**What it never claims.** Not intelligibility, not "studio quality", not a dB figure other than the
one its own `min_band_gain` bounds it to, and not any measurement `X-109` has not made. It removes
some stationary noise and damages some speech, and the trade between those two is
`over_subtraction`'s and `min_band_gain`'s and the caller's.

## 6. Refusal, bypass, and the four things that never happen

Row by row, so that "no silent X" is a checked sentence and not a reassurance:

| The caller offers | What happens |
|---|---|
| a rate outside the declaration | `prepare` refuses `RateNotAccepted`. **Nothing is resampled** — a processor cannot change a rate ([custom-call-dsp.md](custom-call-dsp.md) §10) and this one does not silently accept one it was not measured at |
| a channel count outside the declaration | `prepare` refuses `ChannelsNotAccepted` |
| a rate outside `1..=384,000` | the linear-PCM boundary's own `UnsupportedSampleRate`, carried and not re-minted |
| a frame whose format is not the prepared one | `FormatMismatch`; a rate change is a `prepare`, never a frame |
| a parameter outside its declared range | the **whole set** is refused and the previous set stays in force ([custom-call-dsp.md](custom-call-dsp.md) §3.6) |
| a host that lacks a declared `Device` | `admits_host` refuses `DeviceUnavailable { required }` before anything is attached |
| a graph whose real-time profile is not met | [call-dsp-graph.md](call-dsp-graph.md)'s bypass or teardown, per the declared `on_failure`. The reducer is not consulted and emits nothing: a deadline miss is the runtime's fact, and [custom-call-dsp.md](custom-call-dsp.md) §6 keeps it out of the observation vocabulary on purpose |

And four things that cannot happen, each because of a rule rather than because of care:

- **No remote processing.** [custom-call-dsp.md](custom-call-dsp.md) §4.1 gives a processor no
  socket, no file and no device. A reducer that wanted to send a call's audio somewhere has nothing
  to send it with.
- **No model download, and no model.** The same rule, plus `PortableInteger` (§3.2) as the positive
  declaration. The baseline's entire state is the integers §5.2 names.
- **No resampling.** `Preserving` length and a format fixed at `prepare`; the rate a reducer was
  prepared at is the rate every one of its frames carries or is refused.
- **No silent quality change.** Every parameter is in the closed schema, every change is
  `ParameterApplied` at a stated position, and a rate the baseline was not shaped for is a refusal
  rather than a degraded acceptance.

## 7. Adaptive state is bounded, and is per instance

The baseline's state is a fixed array: two one-pole sections, and three `(env, floor, gain)` triples,
per channel, for at most two channels, plus the position counter the warm-up is measured with. There
is no growth with call length, with frame size or with anything else, no allocation after
construction, and `state_bytes` is the declaration that says so.

**Isolation is a consequence of ownership rather than a rule anyone observes.**
[call-dsp-graph.md](call-dsp-graph.md) §8 already establishes that a graph belongs to one direction
of one call and owns its processors, so two calls running this reducer run two instances with two
sets of the integers above. Nothing here is `static`, nothing is shared behind a lock, and the type
holds no interior mutability — which is
[custom-call-dsp.md](custom-call-dsp.md) §4.4's obligation, and the one this story is required to
prove absent rather than assert.

**No raw audio survives teardown**, and there is none to survive: `retained()` is 0 at every point
in the lifecycle because latency and tail are 0, the frame's samples are borrowed and never copied
into state, and what the estimator keeps is a magnitude and not a sample. `cancel()` is terminal and
idempotent, and the graph's barrier reads `retained()` back rather than assuming it
([call-dsp-graph.md](call-dsp-graph.md) §6).

## 8. The predeclared corpus

Four conditions, generated by the stated integer recurrences below and by nothing else, so that the
corpus is a fixed sequence of samples anyone can regenerate rather than a recording. All run on
`D8`: `Inbound`, 8,000 Hz, one channel, default parameters unless a row says otherwise.

```
lcg(n+1) = (1,664,525·lcg(n) + 1,013,904,223) mod 2³²,          lcg(0) = 1
noise(n) = ((lcg(n+1) >> 16) as i16) · amplitude / 32,768
voice(n) = a 100-position sawtooth of the stated amplitude, gated on for 800
           positions and off for 800
```

The recurrence is a **stated sequence and not a random source**: it lives in the corpus, never in a
processor, and [custom-call-dsp.md](custom-call-dsp.md) §4.4's prohibition is on the processor.

| Condition | Signal |
|---|---|
| `silence` | 4,096 positions of 0 |
| `stationary` | 12,288 positions of `noise` at amplitude 2,000 |
| `transient` | `stationary`, with 32 positions of full-scale alternation spliced in at position 8,192 |
| `overlapping` | `voice` at 8,000 over a second `voice` at 2,000 with a 137-position period, plus `noise` at 800, for 12,288 positions |

Four measures are derived from a run, each an exact integer so that a change moves a number rather
than a judgement:

- **attenuation** — the output's summed magnitude over the input's, in thousandths, over the
  positions after the warm-up.
- **speech distortion** — the summed magnitude of `output − input` over the input's summed
  magnitude, in thousandths, over the positions the gate is on.
- **onset recovery** — the positions between a step up in level and the first position at which the
  output is within one sixteenth of the input.
- **latency** — the declared `latency_positions`, checked against the identity of §5.4's warm-up
  rather than taken on trust.

The recorded figures, at the §5.3 defaults, in thousandths. Each is asserted **exactly**, so that a
change to the arithmetic moves a number rather than a judgement, and each is where it is for a
stated reason:

| Condition | Attenuation | Distortion | Why |
|---|---|---|---|
| `silence` | 1,000 | 0 | `env = 0` takes §5.3's guard; there is nothing to attenuate and nothing to damage |
| `stationary` | 298 | 707 | the whole signal is noise, so every band settles at the floor it declared — and here the "distortion" *is* the noise removal |
| `transient` | 377 | 643 | a full-scale burst is faster than the floor creeps, so it survives where the noise around it does not (§5.6, second row) |
| `overlapping` | 859 | 177 | the near talker rides above the floor the background pulled up, so most of the speech survives — and 177 thousandths of it does not |

Onset recovery is **42 positions** and is hand-derivable rather than observed: the gain sits at
`min_band_gain` when the onset arrives, one sixteenth of the input needs 938 thousandths, and the
slew moves `ceil(1000/64) = 16` per position, so `250 + 16·43 = 938` lands at the position 42 after
the step. Onset recovery under this design *is* the gain slew's length, counted in positions.

**CPU and memory were not measured here.** `X-109` owns the measurement corpus for this epic, and a
figure this story produced on the machine that happened to run it is not a measurement — §9's
vectors therefore assert the bounded *shape* of the state (§7) and leave the cost to the story that
owns it. §8.1 is what that story brought back.

### 8.1 Memory, measured; cost, refused (`X-109`)

**Memory.** `./scripts/check-dsp-heap.sh` — `X-128`'s counting allocator, outside the workspace
because `unsafe_code` is forbidden inside it — now covers this reducer, and the run measures

| Processor | Declared `state_bytes` | Inline | Peak live heap | Allocated after `prepare` |
|---|---|---|---|---|
| `sipx.subband_suppressor` | 256 | 256 | **0** | **0** |

which is §7's bound turned from a construction argument into a figure: three bands of fixed-width
followers and a recursive minimum, and not one byte of heap behind them, across construction,
`prepare`, `process`, `flush`, `reset` and `cancel`. It went unmeasured for two stories because
that probe's processor list was written by hand; the list is now held to `BUILT_IN_IDS` and
`NOISE_REDUCTION_IDS`, so the next processor that ships without a measurement is a red run.

**Cost is not recorded, and this is the row deliberately left empty.** `crates/sipx-audio/examples/dsp_cost.rs`
takes it — per processor, per condition, over these four signals at their §8 lengths, reported as
nanoseconds per thousand positions and as parts per million of one core at 8,000 Hz. It refuses to
report when the one-minute load average is above a tenth of the machine's cores, and refuses again
when its own control workload drifts by more than 10% between the start of the run and the end.
**On the machine available to `X-109` both guards fired**, so there is no figure here rather than a
figure with a caveat. The command is in that story's `## Progress`.

The two generators of this corpus — the vectors' and the cost harness's — are held to one set of
FNV-1a checksums by `the_corpus_is_the_same_corpus_the_cost_harness_measures`, so a cost figure and
a quality figure are always about the same samples. That test found a real divergence the day it
was written.

**Response.** The reducer also runs through `sipx_audio::dsp::response`'s packaged sweep, which is
integer arithmetic and needs no quiet box. What it shows is worth stating and easy to misread: after
the 1,024-position warm-up a **settled tone is essentially untouched** — 4 thousandths off at worst
after 2,048 positions, 17 after 8,192 — because a tone raises the envelope as fast as it raises the
floor and §5.3's ratio barely moves. That is **not** a noise-reduction figure; this section's four
conditions are. What it rules out is the opposite failure: a reducer that gated a steady talker would
show there as a collapse toward `min_band_gain`, and this one does not.

## 9. Vectors

Unless a row says otherwise it runs on `D8` with the §5.3 defaults.

| ID | Input | Expected |
|---|---|---|
| NR-V1 | any 1,024 positions | bit-exact the input, and `PassedThrough` on every frame — §5.4's warm-up identity |
| NR-V2 | 4,096 positions of 0 | 4,096 zeros and `PassedThrough` throughout: `env = 0` takes §5.3's guard, so silence is never attenuated into something else |
| NR-V3 | full-scale alternation at amplitude 16,384, `adaptation_positions = 1` | after the warm-up and at most 47 slew positions, every output is exactly `input · 250 / 1000`: at the folding frequency both sections are zero, so all the energy is in `b₂` and its gain is at `min_band_gain` |
| NR-V4 | NR-V3 with `min_band_gain = 1_000` | bit-exact the input for the whole stream — an all-unity gain is §5.1's exact identity |
| NR-V5 | NR-V3 with `min_band_gain = 0` | exactly zero after the slew: the hole §5.3 offers, on request |
| NR-V6 | NR-V3 interrupted by `reset(Requested)` at position 4,096 | the warm-up re-opens: the 1,024 positions after the reset are bit-exact the input again, and the stream then equals a fresh instance's |
| NR-V7 | NR-V3 with `voice_active` set before the first frame | the floor never rises past its seed, so the output differs from NR-V3's — and with the flag never set the run is NR-V3 exactly |
| NR-V8 | `prepare` at 44,100 Hz; at 8,000 Hz with 3 channels; at 0 Hz | `RateNotAccepted`; `ChannelsNotAccepted`; `UnsupportedRate` — and an 8,000 Hz mono `prepare` afterwards still succeeds |
| NR-V9 | `min_band_gain = 1_001`; `over_subtraction = 999`; `adaptation_positions = 0`; a `voice_active` given an integer | each refuses the whole set; the previous set is still in force, proved by re-running NR-V3 |
| NR-V10 | a declaration claiming `Optional { "not_declared" }`; one claiming `Optional { "adaptation_positions" }`; `warm_up_positions = 65_537`; `Device { "" }` | `ActivityParameterUndeclared`; `ActivityParameterNotAFlag`; `WarmUp`; `EmptyDeviceIdentifier` |
| NR-V11 | `admits_host(&[])` against `PortableInteger` and against `Device { "npu" }`; `admits_host(&["npu"])` against the latter | accepted; `DeviceUnavailable { required: "npu" }`; accepted |
| NR-V12 | the baseline and the external fixture, each against `DSP-K1`..`DSP-K12` | no failure; the unproven set exactly `DSP-K9` for both (§4.5) |
| NR-V13 | the baseline and the external fixture, driven through one `dyn NoiseReducer` list by identical code | both complete the whole lifecycle; the driving code names no implementation (§4.4) |
| NR-V14 | two instances of the baseline, one fed `stationary` and one fed `silence`, interleaved frame by frame | each instance's output equals what it produces alone — no adaptive state crosses instances (§7) |
| NR-V15 | the §8 corpus, all four conditions | §8's recorded integers exactly; every attenuation between 250 and 1,000 thousandths because `min_band_gain` bounds it below and unity bounds it above; and `overlapping` above `transient` above `stationary`, which is §5.6's ordering rather than one run's arithmetic |
| NR-V16 | `stationary` cut at 1, 7, 13, 160 and 4,096 positions per frame against one frame | identical samples, whatever the framing |
| NR-V17 | a step from amplitude 500 to amplitude 16,000 after the gain has settled | the output is within one sixteenth of the input 42 positions later — §8's derivation, and no other quantity in the design |
| NR-V18 (`X-109`) | each §8 condition, hashed | the four FNV-1a checksums of §8.1, and §8's lengths — the cost harness's copy of the recurrences is the same corpus these vectors measure |
