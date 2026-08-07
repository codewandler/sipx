# Design: local live-call speech

**Status:** proposed · **Pillar:** Application · **Epic:** `local-speech` · **Stories:** A-25,
M-54, M-55, M-56, A-26, A-27, A-28, X-104, X-105

## Why

A live call on a machine with a local accelerator should be able to transcribe received speech and
synthesize speech back into the call without sending audio away from that machine. The same feature
must remain usable on a CPU-only device with stated limits, and an application must receive every
result and lifecycle transition through the application SDK rather than polling provider-specific
objects.

The durable product boundary is not one model or one device API. It is two substitutable provider
contracts — speech recognition and speech synthesis — selected by endpoint policy and overridable
per call. sipx ships practical local/offline implementations of both, while a downstream replacement
implements the same capability discovery, cancellation, failure and conformance contract. No
network-backed provider is selected implicitly.

This is a direct product request after the demand survey recorded in `demand.md`; it does not rewrite
that historical observation. It also does not turn `M-43` into a speech feature. `M-43` remains the
unopinionated PCM/resampling boundary, and this epic is one explicitly requested consumer of it.

## Approach

`A-25` specifies separate recognition and synthesis interfaces before code. Discovery reports stable
provider identity, local/offline status, languages, voices, accepted/emitted sample formats,
streaming support, accelerator kinds, CPU support and resource estimates. Endpoint configuration
chooses defaults; a call may choose another compatible provider. Selection is explicit and returns
typed reasons when the requested language, voice, format or execution device is unavailable. The
normative contract is [`docs/specs/speech-providers.md`](../specs/speech-providers.md): the
sans-I/O boundary (§2), the discovery descriptor (§3), total selection documents with deterministic
precedence and id-only fallback chains (§4), the recognition and synthesis session contracts
(§5–§6), the shared lifecycle disjoint from SIP (§7), bounds (§8), the extensibility record (§9)
and the `DIS`/`SEL`/`REC`/`SYN`/`LIF` vectors (§10).

`M-54` adds one call-owned PCM processing seam after decode and before application fan-out, with
explicit direction, sample time, discontinuities and resampling. It is bounded and non-blocking: a
slow processor loses named frames according to policy and receives a discontinuity rather than
stalling RTP. The real-time analysis epic consumes the same seam instead of adding another tap.

`M-55` and `M-56` provide the local/offline recognition and synthesis implementations. Accelerator
selection is capability-driven, never inferred from a marketing device name. CPU behavior is part
of the contract: either a declared real-time profile runs within its limits, or setup refuses with
the measured requirement; fallback never silently changes language, voice, quality, privacy or
latency policy. External implementations run against `X-105`'s same conformance suite.

`A-26` and `A-27` carry the application surface. Recognition emits ordered partial, replacement,
final, cancellation and error events with per-utterance identity. Synthesis accepts bounded enqueue,
play, cancel and ducking instructions and emits accepted, started, completed, cancelled, ducked,
resumed and failed transitions. Provider warm-up, fallback and loss are visible separately from SIP
call failure. Neither direction can block the call event stream.

`A-28` owns the privacy and isolation rule. Audio and transcripts are retained for no longer than
the live operation by default; debug capture, model caches containing derived user data and any
off-host provider are explicit opt-ins. Every call receives its own bounded queues, cancellation and
resource budget. Credentials, model paths and synthesized text are redacted from ordinary logs.

`X-104` publishes a runnable application example and measurements from the exact packaged surface.
It exercises a live call on an available accelerator and the defined CPU path, but has bounded
fixtures that run without special hardware. `X-105` separately proves provider substitution so the
example cannot become an accidental contract.

## Boundaries

- SIP, transactions and dialogs remain unchanged; speech is an application/media attachment to an
  established call.
- The protocol and media cores do not discover devices, load models, read clocks or retain audio.
  Those are leaf-driver responsibilities behind the provider interfaces.
- The default is local/offline execution and no audio retention. A downstream provider that moves
  data off-host must be selected under explicit host policy and reports that property in discovery.
- This epic does not promise speaker identification, translation, conversation memory, intent
  routing, model training or a bundled assistant.
- Recognition does not replace RFC 4733 DTMF events. Synthesis ducking is a bounded gain policy on
  call audio, not SIP hold and not mute.

## Exit

Two interchangeable local/offline providers pass the common recognition/synthesis contracts,
accelerator and CPU paths have measured limits, a real call emits ordered recognition and synthesis
lifecycle events, cancellation drains every task and queue, default runs retain no audio or text,
and a clean application runs the documented example from the packaged SDK surface. M16 is tracked
after M13 and does not alter M13's selected stories.
