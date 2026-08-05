# Design: custom call-audio DSP

**Status:** proposed · **Pillar:** Media · **Epic:** `custom-call-dsp` · **Stories:** M-63, M-64,
M-65, M-66, M-67, M-68, X-109, A-34

## Why

Applications need to shape live call audio without forking the media runtime: ordinary gain and
filtering, creative distortion and intentional glitch/stutter effects, and practical noise
reduction. The durable feature is not one effect collection. It is a deterministic, bounded DSP
contract on which built-in and application-supplied processors behave identically.

This epic consumes `M-54`'s direction-aware PCM seam. It does not add audio callbacks to the SIP
core or put JavaScript on a real-time worker. Crate-owned processors may use the proven inline
profile. Application-supplied processors use a bounded, supervised isolation profile when the
application requires the stack to contain stalls; a separately named cooperative-native profile is
trusted code and cannot make that containment claim.

## Approach

`M-63` specifies a synchronous sans-I/O frame transform: exact PCM format, direction, sample
position, discontinuity and finite parameter state enter; bounded audio and typed observations
leave. A processor declares supported formats, maximum frame size, latency, tail, channel count,
whether it changes length, and reset/flush behavior before attachment. It also defines the minimum
execution/failure policy consumed by `M-64`: proven inline, supervised isolated, or explicitly
trusted cooperative-native; deadline action; and fail-open bypass versus fail-closed termination.

`M-64` owns the call-local graph and implements that minimum policy. Ordered chains attach
separately to receive and transmit paths, validate completely before atomic publication, and can be
bypassed or replaced without a partially updated frame. Every queue and scratch allocation is fixed
by configuration. The media worker never waits for an isolated processor: a bounded request/result
channel either yields the matching frame by its sample deadline or applies the declared failure
action. Its supervised worker process can be terminated and reaped. Removal, call teardown and
failed replacement release all stack-owned state under an observable barrier.

`M-65` ships useful deterministic building blocks and effects through that same public contract:
gain, polarity, soft/hard clipping, bit crushing, delay/stutter-style glitching, and stable
high-pass/low-pass/peaking filters. Parameters have named ranges and transitions; no effect may
produce NaN, overflow, out-of-range PCM or unbounded delay state.

`M-66` defines interchangeable noise-reduction processors and ships a local baseline implementation.
It reports algorithmic delay, supported rates and channels, warm-up and CPU profile; keeps adaptive
state per call/direction; and has an explicit bypass/refusal outcome when its declared real-time
profile cannot be met. Noise reduction is distinct from VAD: it may consume activity observations,
but it cannot silently redefine VAD events.

`M-67` exposes typed parameter and lifecycle control through the application SDK. Control messages
are finite and applied at declared sample boundaries. SDK code selects registered processor IDs and
closed parameter values; arbitrary host-language callbacks never execute on the media worker.

`M-68` hardens and proves the policy already implemented by `M-64`: impulses, full-scale alternating
samples, long silence, format changes, discontinuities, invalid parameters, processor errors and
over-budget work cannot panic, retain stack-owned audio without bound or starve RTP on the proven
inline and supervised-isolated profiles. It tests worker crash, hang, malformed result, deadline and
reaping. A cooperative-native callback is measured and may be quarantined only after it returns; a
non-returning callback is outside sipx's containment and teardown guarantees, which the API and
events state explicitly.

`X-109` owns the corpus and measurements for bit-exact processors, frequency/impulse response,
noise attenuation versus speech damage, algorithmic delay, CPU, allocation and glitch/drop counts.
`A-34` publishes a runnable call example that changes an ordered graph live and compares bypassed
and processed output through only packaged APIs.

## Boundaries

- This epic processes decoded/produced linear PCM. Codec negotiation, RTP loss/jitter statistics,
  device I/O and resampling ownership remain in their existing layers.
- “Glitch” means an explicitly selected bounded audio effect. Accidental deadline misses, dropped
  frames and discontinuities are defects/measurements, never described as an effect.
- Built-in VAD and signal metrics remain in `call-audio-analysis`; speech recognition and synthesis
  remain provider contracts in `local-speech`.
- Echo cancellation, automatic gain control spanning devices, source separation, dereverberation and
  model downloading require separate measured requirements. They are not implied by “noise
  reduction.”
- Cooperative native processors are trusted application code. The stack validates their declared
  shape and measures returning calls, but cannot prevent them from copying audio, spawning work or
  failing to return. They are never described as contained. Applications that need hard stall,
  lifetime and stack-owned-buffer isolation select the supervised process profile. Sandboxed/WASM
  DSP execution remains future work and is not silently promised here.

## Exit

An application composes built-in and external processors on either call direction, changes their
bounded parameters at deterministic sample boundaries, and observes every activation, bypass,
failure and teardown. Effects satisfy exact vectors; the bundled noise reducer meets predeclared
attenuation, speech-damage, latency, CPU and memory thresholds; overload in the proven inline and
supervised-isolated profiles never stalls RTP; concurrent calls share no adaptive state; and a clean
consumer runs the documented example from packaged APIs. M18 is post-M13 work and does not expand
the selected endpoint-completeness wave.
