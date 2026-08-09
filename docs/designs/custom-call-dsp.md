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

## How processor heap growth is measured (`X-128`)

`DSP-K9` reported the heap half of `state_bytes` `Unproven` from `M-63` onward. §9.1 was right that
the workspace half is structural — scratch and sink are caller-owned and sized from the declaration
— and right that the heap half was unobservable. It was wrong to leave the reason as "no counting
allocator can be installed" without saying where one *could* be. This records the four options and
what each was measured against, because the decision is the deliverable and not the code.

**Option A — a counting allocator inside `sipx-audio`, behind `#[cfg(test)]` or a dev-only feature.
Impossible, proved rather than assumed.** `[workspace.lints.rust] unsafe_code = "forbid"` reaches
rustc as `-F unsafe_code` on the command line, and Cargo's `[lints]` table applies to *every* target
of a member package. An integration test under `crates/sipx-audio/tests/` is therefore as forbidden
as the library: `unsafe impl GlobalAlloc` there fails with "implementation of an `unsafe` trait", and
a narrowly scoped `#[allow(unsafe_code)]` fails with `E0453`, "overruled by previous forbid". A
`forbid` is not overridable by construction; that is the whole difference between it and `deny`. The
only door is deleting `[lints] workspace = true` from a published crate, which would drop every other
workspace lint from it as collateral and void non-negotiable 3 for `sipx-audio` itself.

**Option B — measurement outside the forbidding crate. Chosen, and already precedented.** The root
manifest excludes `fuzz/` and `wasm/` from the workspace *for exactly this reason*, and says so: the
browser module needs `#[unsafe(no_mangle)]`, "so it lives outside the workspace too, keeping the
non-negotiable intact for every crate that answers to it". A counting global allocator is the same
shape of need and gets the same answer. `heap-probe/` is a fourth-wall package outside the workspace
that installs the allocator, implements a **safe** trait `sipx-audio` declares, and drives the same
`Conformance` harness every other caller drives. Nothing in the workspace gains `unsafe`;
`scripts/comparison-report.py::rule_unsafe_policy` reads `unsafe_code = "forbid"` out of the
workspace manifest and publishes it, and that generated claim stays true.

**Option C — proving the bound structurally.** The structural half is already done and cannot be
extended to the rest. `size_of::<P>()` sees a pointer, not what it points at, and Rust offers no safe
traversal of a foreign type's heap: a `P: FrameProcessor` from an application crate may hold any
`Vec` it likes. Making the heap half structural would mean forbidding owned heap in the trait, which
`Stutter`'s delay line — a legitimate `vec![0; samples]` sized from a bounded parameter — shows is
the wrong contract.

**Option D — a permanent `Unproven` with a better reason. Adopted for the workspace, and only
there.** Under `cargo test`, no meter exists and none can, so `DSP-K9` keeps reporting `Unproven`.
What changes is that the reason now names the mechanism that *does* measure it and the command that
runs it, instead of stopping at "cannot". An unproven check that tells you where the proof lives is a
different artifact from one that tells you to give up.

The seam between B and D is one safe trait, `HeapMeter`, and one additive entry point,
`Conformance::run_with_heap_meter`. The harness's arithmetic — what is compared against
`state_bytes`, when a figure fails — lives in `sipx-audio` and is tested in the workspace against a
scripted test-double meter, so the logic is covered by `cargo test`. Only the *bytes* come from
outside. That split is deliberate: if the whole check lived in `heap-probe/`, a refactor in the
workspace could silently stop calling it and nothing in the gate would notice.

Two figures are measured, because §9.1 makes two separate claims. **Peak live bytes** over a
processor's whole life, held against `state_bytes` less its inline size — this is the claim
`state_bytes` *is*. And **bytes allocated after `prepare`**, which must be zero — this is §9.1's "no
allocation after `prepare`, not per frame, not per position, not per observation", and until now
nothing checked it at all. Construction is inside the metered window: `Stutter` allocates its line in
`new()`, so a window starting at `prepare` would measure zero and leave the one built-in with a real
heap component exactly as unverified as before.

The window is kept clean by the harness allocating nothing inside it — buffers are taken before the
meter starts and reused across frames — and that property is self-checking rather than asserted: the
eight built-ins with no heap measure exactly 0 bytes, which they could not do if the harness's own
allocations were landing in the window.

## Exit

An application composes built-in and external processors on either call direction, changes their
bounded parameters at deterministic sample boundaries, and observes every activation, bypass,
failure and teardown. Effects satisfy exact vectors; the bundled noise reducer meets predeclared
attenuation, speech-damage, latency, CPU and memory thresholds; overload in the proven inline and
supervised-isolated profiles never stalls RTP; concurrent calls share no adaptive state; and a clean
consumer runs the documented example from packaged APIs. M18 is post-M13 work and does not expand
the selected endpoint-completeness wave.
