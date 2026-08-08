# Interchangeable speech providers

**Status:** normative · **Story:** A-25 · **Epic:** `local-speech` ·
**Design:** [local-speech](../designs/local-speech.md) · **Crates (planned):** contract types and
the selection state machine in `sipx-media`; deterministic providers and conformance suites in
`sipx-testkit` (X-105); bundled local providers per M-55 and M-56

This specification defines two substitutable provider contracts — speech **recognition** and speech
**synthesis** — selected by endpoint policy and overridable per call. It is the admission gate for
the `local-speech` epic: M-55/M-56 implement it, A-26/A-27 surface it through the application SDK,
A-28 owns its data-retention policy, and X-105 makes it executable for downstream replacements. It
specifies interfaces and state; it contains no model, no device API and no bundled implementation.

The words **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** are normative.

## 1. Normative references and precedence

- RFC 3261 defines SIP call, dialog and transaction state. Provider lifecycle is disjoint from all
  of it (§7); nothing in this specification changes a SIP state machine.
- RFC 3550 §5 defines the RTP timestamp and media clock from which every sample time in this
  contract derives. Wall-clock time appears nowhere.
- RFC 3551 defines audio encoding identity at the media boundary.
- RFC 4733 defines DTMF as telephone-events. Recognition of spoken digits does not replace them.
- RFC 5646 defines the language tags used in discovery and selection; RFC 4647 §3.3.1 (basic
  filtering) defines how a requested range matches a provider's declared tags.
- [`linear-pcm.md`](linear-pcm.md) defines the owned PCM boundary (`Pcm`, `PcmFormat`) and the
  shared resampler. All audio in this contract uses that boundary; this spec adds no second audio
  representation.
- [`media-runtime.md`](media-runtime.md) defines worker ownership and shutdown discipline. Session
  drivers inherit it: no spawn discards its handle, and drains are resumable.
- [`app-contract.md`](app-contract.md) defines the application event stream that A-26/A-27 extend
  with the events named here.
- [Design: local live-call speech](../designs/local-speech.md) records the epic's scope and
  boundaries; where this spec is silent, the design's Boundaries section controls.

Call-side audio enters and leaves through M-54's bounded PCM processing seam. This spec does not
define a second media tap; it defines what rides the seam.

## 2. Boundary and vocabulary

- **Provider** — one implementation of one contract kind (recognition or synthesis), registered
  with an endpoint. A provider is a leaf driver: it may open devices and load models behind the
  contract, subject to its declared locality properties (§3).
- **Registry** — the set of providers an endpoint host has explicitly registered. Registration is
  the only way a provider becomes selectable; nothing is discovered implicitly.
- **Session** — one per-call, per-direction instance of a provider contract. Sessions own their
  queues and state; two sessions share no mutable state, including two sessions of the same
  provider on different calls (A-28).
- **Driver** — the host-owned asynchronous shell around a session. It owns every task and bounded
  queue, feeds inputs in order, applies outputs in order, and fires deadlines.
- **Utterance** — the recognition result unit: one span of speech with one identity and exactly one
  terminal outcome.
- **Request** — the synthesis work unit: one bounded text input with one identity and exactly one
  terminal outcome.

**Sans-I/O placement.** The contract types, descriptors, events, errors and the selection state
machine (§4) MUST be importable and testable without an async runtime, socket, device or clock.
Time enters the contract in exactly two forms: sample time carried on frames and chunks (RFC 3550
media clock, via the M-54 seam), and driver-fired deadlines carrying a generation. A fired deadline
with a stale generation is ignored. The core crates `sipx-sip` and `sipx-sdp` are untouched by this
epic and MUST NOT gain any speech dependency.

**Locality is a property, not a name.** A provider whose descriptor declares no network egress and
no off-host processing MUST NOT open a network connection or move audio, text or derived data off
the machine, in any state including failure. The descriptor claim is admitted or refused by host
policy (§4 step 2); A-28 specifies enforcement and retention.

**Ownership.** Every input and output is an owned value. Audio uses `Pcm`/`PcmFormat` from
[`linear-pcm.md`](linear-pcm.md); text is owned UTF-8. A provider MUST NOT retain a borrowed view
of any input beyond the delivery that carried it, and never receives a call handle, a socket, or a
media session — its entire world is its session's inputs and outputs.

## 3. Discovery

Discovery is a synchronous read of the registry: it reports registered descriptors and performs no
probing, no model load and no network I/O. Device facts are gathered by the provider at
registration time, behind the leaf-driver boundary. Two consecutive discovery reads with an
unchanged registry MUST be identical, and the order of the discovery list carries no meaning:
selection (§4) never depends on it.

A `ProviderDescriptor` reports:

| Field | Applies to | Meaning |
|---|---|---|
| `id` | both | stable lowercase identity token, unique in the registry, identical across restarts and versions; never a marketing name |
| `version` | both | provider version, separate from identity |
| `kind` | both | `Recognition` or `Synthesis`; one descriptor describes one kind |
| `off_host` | both | whether audio, text or data derived from them ever leaves this machine |
| `network` | both | whether runtime operation requires any network egress at all |
| `languages` | both | RFC 5646 tags the provider supports; matching per RFC 4647 §3.3.1 |
| `voices` | synthesis | stable voice tokens, each with the RFC 5646 tags it can speak and declared properties |
| `accepted_formats` | recognition | explicit list of `PcmFormat` values accepted as session input |
| `emitted_formats` | synthesis | explicit list of `PcmFormat` values the provider can emit |
| `streaming` | both | recognition: whether revisions are emitted before `Flush`; synthesis: whether chunks are emitted before the request completes |
| `devices` | both | execution devices usable now, each described by capability: `kind` (`Cpu` or `Accelerator`), memory bytes, and declared concurrent real-time sessions |
| `resources` | both | estimates: bytes to warm the provider, resident bytes per session, and a warm-up duration estimate |

*Local/offline* is the conjunction `off_host = false` and `network = false`; it is defined by these
properties and never by a product identity. `resources` values are estimates for capacity planning;
the binding guarantee is behavioral — a provider that cannot meet its declared real-time profile
refuses setup with the measured requirement (M-55/M-56) rather than degrading silently.

A voice's **declared properties** are stable lowercase tokens and nothing more. No step of §4 reads
them, so a property can never quietly become the reason one voice was selected over another; a voice
is chosen by the identity the host wrote in the selection document and by nothing else. They exist
so discovery can report what a provider says about a voice beyond the tags it speaks, and a
consumer that does not understand a token ignores it.

Execution devices are described by capability because selection by marketing name is not portable
and not checkable. A device entry states what it is (`Cpu`/`Accelerator`), how much memory it
offers, and how many concurrent real-time sessions the provider declares for it; M-55/M-56 must
measure those declarations in fixtures.

## 4. Selection: defaults, overrides, precedence

A **selection document** is the unit of speech policy. Its fields are total: every field is either
required, forbidden for the kind, or optional with the absence meaning stated here — no field's
absence is ever filled from another document.

| Field | Recognition | Synthesis | When absent |
|---|---|---|---|
| `provider` | required id | required id | malformed |
| `language` | required RFC 4647 range | required RFC 4647 range | malformed |
| `voice` | forbidden | required voice token | malformed (synthesis) |
| `format` | optional pinned provider-side `PcmFormat` | optional pinned provider-side `PcmFormat` | the operating format is derived by the rule below |
| `device` | optional required device capability | optional required device capability | no device constraint is imposed |
| `conversion` | optional `allow`/`deny` | optional `allow`/`deny` | `allow` |
| `fallback` | optional ordered list of provider ids | optional ordered list of provider ids | empty — a refusal is final |

A document missing a required field, carrying a field its kind forbids, or whose `fallback` entries
are anything other than bare provider ids is refused as `MalformedSelection` before the registry is
read; well-formedness is a property of the document alone. `voice` is required for synthesis
precisely so that no machinery — neither a provider's preferred ordering nor a fallback step —
ever chooses a voice: the host wrote it, or selection refuses. The `language` wildcard `*` is
permitted; it is the host explicitly stating that any declared language is acceptable, not an
omission.

Precedence is deterministic and total:

| Rank | Source | Rule |
|---|---|---|
| 1 | per-call override | a complete selection document; it replaces the endpoint default entirely — an absent optional field means what the table above says, never what the endpoint default's field says |
| 2 | endpoint default | one optional document per contract kind |
| 3 | nothing configured | the operation returns `NoProviderConfigured`; no provider is ever selected implicitly, and discovery order never implies a default |

Field-level merging of override and default is deliberately excluded: an effective policy assembled
from two documents cannot be read from either, and the first mis-merged voice would be a silent
policy change.

A document is evaluated against the registry in a fixed order; evaluation stops at the first
failing step and returns that step's typed reason together with the provider `id`, the requested
value and the descriptor facts consulted:

| Step | Check | Typed reason on failure |
|---|---|---|
| 1 | `provider` is registered with the requested `kind` | `UnknownProvider` |
| 2 | host locality policy admits `off_host`/`network` (A-28 opt-ins) | `LocalityRefused` |
| 3 | `language` range matches `languages` per RFC 4647 §3.3.1 | `UnsupportedLanguage` |
| 4 | the named voice exists and speaks a tag matching the range | `UnsupportedVoice` |
| 5 | the operating-format rule below yields a format | `UnsupportedFormat` |
| 6 | required device capability is present in `devices` | `UnsupportedDevice` |

**Effective language tag.** When several declared tags match the range, the effective tag is the
first matching tag in the descriptor's declared order. It is derived, deterministic, and reported
in the selection result and in `FallbackEngaged`; the *policy* is the range, which no selection
step can alter.

**Operating format.** Every session runs in exactly one provider-side `PcmFormat`, fixed at
selection:

- a pinned `format` must appear in the descriptor's `accepted_formats`/`emitted_formats`; under
  `conversion = allow` an M-43 conversion between the pin and the negotiated call clock must
  exist, and under `deny` the pin must itself be the seam's call-clock format — signed 16-bit at
  the negotiated media clock ([`linear-pcm.md`](linear-pcm.md) §3);
- with no pin and `conversion = allow`, the operating format is the first entry in the
  descriptor's declared format list for which an M-43 conversion to and from the call clock
  exists;
- with no pin and `conversion = deny`, the operating format is the seam's call-clock format,
  which must appear in the declared list.

Any branch that yields no format is `UnsupportedFormat`. The operating format is part of the
selection result, so nothing downstream re-decides it.

A refusal MUST NOT select a different provider, language, voice, format or device. The only path
past a refusal is the explicit `fallback` chain: an ordered list of provider ids — ids only, never
nested documents and never further chains. Each candidate is evaluated in configured order under
the top-level document with only `provider` replaced by the candidate id; the constraint fields
never vary across the chain, so engaging a candidate cannot change language, voice, format, device
or conversion policy. Every candidate's refusal is recorded with its typed reason and the first
satisfying candidate is selected. If no candidate satisfies, the operation fails with the ordered
per-candidate reasons. An empty or absent chain means a refusal is final.

Selection runs at session start and is fixed for the session's lifetime. The same chain, and the
same rule, governs runtime provider loss (§7). Selection is part of the sans-I/O state machine: it
reads descriptors and documents, and performs no I/O.

## 5. Recognition contract

A recognition session consumes ordered inputs and emits ordered outputs. Every input is atomic;
outputs are applied in order.

| Input | Data |
|---|---|
| `Frame` | owned PCM in the session's operating format (§4), direction, sample time, sequence |
| `Discontinuity` | the seam's typed kind — `Loss`, `Overflow` or `Realign` — and the lost span (sequence gap and duration in samples) |
| `Flush` | end of audio input; no `Frame` may follow |
| `Cancel` | session scope, with a typed reason (§7) |
| `DeadlineFired` | deadline kind (warm-up, drain) and generation |

| Output | Meaning |
|---|---|
| `Warming` / `Ready` | lifecycle (§7); no result output may precede `Ready` |
| `Partial` | opens utterance `u` at revision 1 with its complete text so far and covered sample-time span |
| `Replacement` | revision `n+1` for the open utterance; replaces the utterance's entire prior text |
| `Final` | terminal: the utterance's complete text and span |
| `Cancelled` | terminal for the open utterance, with a typed reason |
| `Failed` | terminal for the session, with a typed cause |
| `Lost` | the provider's engine or execution device became unavailable (§7); open work has already been resolved terminally |
| `Stopped` | the session owns no task, queue, buffer or device allocation; always the last output |

**Utterance state.** At most one utterance is open per session; identities are strictly increasing
integers per session. `Partial` opens; `Replacement` revises with a revision exactly one greater
than the last; exactly one of `Final`/`Cancelled` terminates. No output for an utterance follows
its terminal, and utterance `n+1` cannot open before utterance `n` terminates. Every event carries
the utterance's complete text — never a delta — so a coalesced or missed revision cannot leave a
consumer permanently wrong.

**Non-streaming providers.** A provider with `streaming = false` emits no `Partial`/`Replacement`
before `Flush`; it still emits each utterance's `Partial` (revision 1) immediately followed by
`Final` after `Flush`, preserving the utterance state machine unchanged.

**Timing.** Result events carry the sample-time span of the audio they cover, derived from `Frame`
sample times. A provider MUST NOT stamp results from any clock.

**Input bound and backpressure.** The driver owns a bounded frame queue (§8). At the bound the
oldest queued frame is dropped and the driver MUST deliver one `Discontinuity` input with kind
`Overflow` naming the accumulated lost span before the next `Frame`. Frames offered before `Ready` follow the same
policy, so a slow warm-up is a bounded loss, never a stall. RTP decode, playback and capture are
never blocked by recognition; that guarantee is the M-54 seam's and is restated here as a driver
obligation.

**Discontinuity semantics.** `Discontinuity` is an ordered input, and its kind vocabulary is the
seam's, pinned normatively by M-57's call-audio processing contract (`call-audio-processing.md`):
`Loss` — upstream frames were lost (network or decode); `Overflow` — frames were dropped under the
bounded-queue loss policy, which is what the recognition input bound above delivers; `Realign` —
the seam re-anchored the timeline. Every output derived from pre-gap audio MUST be emitted before
any output derived from post-gap audio, and each result event carries the count of discontinuity
spans inside its covered span. A provider MAY bridge a gap inside one utterance; it MUST NOT
reorder around one. The deterministic test provider (§10) terminates its open utterance at every
discontinuity, which is what makes the vectors exact.

**Output bound.** Non-terminal outputs coalesce per utterance: at most one pending revision, newest
wins. Terminal and lifecycle outputs are never coalesced or dropped. When unconsumed terminals
reach their bound (§8), the driver stops consuming provider output, which stops frame consumption,
which engages the input policy above — the pipeline degrades to bounded, named loss rather than
unbounded memory.

**Flush, cancel, failure, shutdown.** After `Flush`, the open utterance resolves terminally
(`Final`, or `Cancelled` when the provider has no result), then `Stopped`. `Cancel` resolves the
open utterance as `Cancelled` with the input's reason, then `Stopped`. A provider failure resolves
the open utterance as `Cancelled` with reason `SessionFailed`, then emits `Failed` with the typed
cause, then `Stopped`. A `Frame` after `Flush` is a driver defect; the provider fails the session
with cause `ProtocolViolation`. If the drain deadline fires before `Stopped`, the driver aborts the
provider's tasks and emits `Stopped` itself, marking the stop aborted; an aborted stop is a
reportable provider defect, not a hang.

## 6. Synthesis contract

| Input | Data |
|---|---|
| `Enqueue` | request identity, owned UTF-8 text within the text bound, and `replace: bool` |
| `Cancel` | one request identity or the whole session, with a typed reason |
| `Drained` | request identity and number of chunks the driver consumed; returns window credit |
| `DeadlineFired` | deadline kind (warm-up, drain) and generation |

| Output | Meaning |
|---|---|
| `Warming` / `Ready` | lifecycle (§7); `Started` may not precede `Ready` |
| `Accepted` | the request is queued, with its queue position |
| `Refused` | typed: `QueueFull`, `TextTooLarge`, `SessionEnded`; a refused request has no further events |
| `Started` | the request began producing audio |
| `Chunk` | request identity, per-request monotonic sequence, sample-time offset from request start, owned PCM in the session's operating format (§4) |
| `Discontinuity` | a named production gap inside the current request (duration in samples) |
| `Completed` | terminal: total samples produced |
| `Cancelled` | terminal, with a typed reason |
| `Failed` | terminal for a request or — with no request identity — for the session, with a typed cause |
| `Lost` | the provider's engine or execution device became unavailable (§7); open work has already been resolved terminally |
| `Stopped` | nothing owned remains; always the last output |

**Request state.** `Enqueue` yields exactly one of `Accepted`/`Refused`. An accepted request is
`Queued`, then `Started`, then streams chunks, then exactly one terminal among
`Completed`/`Cancelled`/`Failed`. Requests start in FIFO order. `Enqueue` with `replace = true`
first cancels the started request and every queued request — each receives `Cancelled` with reason
`Replaced`, in queue order — and then accepts the new request; those cancellations are emitted
before the new request's `Accepted`.

**Chunk continuity.** Chunk `n+1`'s sample-time offset MUST equal chunk `n`'s offset plus chunk
`n`'s duration, unless a `Discontinuity` output between them names the gap. A provider that falls
behind real time marks the gap; it MUST NOT emit late audio labeled as continuous. Whether a gap
becomes silence or a shifted playout is the driver's policy at the M-54 seam (A-27), not the
provider's.

**Production bound.** The driver grants a chunk window (§8). The provider MUST NOT have more
unconsumed chunks outstanding than the window; `Drained` returns credit. A provider cannot run
ahead of a slow call into unbounded audio.

**Cancellation.** Cancelling a queued request yields its `Cancelled` immediately. Cancelling a
started request stops production within the window: at most one already-in-flight `Chunk` MAY
arrive after the input, then `Cancelled`. Cancelling a request already terminal, unknown, or
refused is ignored. Session-scope cancel resolves the started request and every queued request in
queue order, then `Stopped`.

**Failure and shutdown.** A production failure emits `Failed` for the active request; if the
session cannot continue, every queued request is `Cancelled` with reason `SessionFailed` in queue
order, then session `Failed`, then `Stopped`. Call teardown is a session-scope `Cancel` with
reason `CallEnded` followed by the same drain. The drain deadline rule of §5 applies unchanged.

## 7. Lifecycle: warm-up, readiness, loss, fallback — and what SIP never sees

Both session kinds share one lifecycle:

| State | Entered by | Leaves by |
|---|---|---|
| `Warming` | session start; the driver arms the warm-up deadline | `Ready`, or warm-up failure |
| `Ready` | provider signals readiness; result/`Started` outputs are now allowed | loss, failure, cancel, flush |
| `Lost` | the provider's engine or device becomes unavailable | `Stopped`, always; a configured fallback chain starts a successor session, which is a host action, not a transition of the lost session |
| `Stopped` | terminal; nothing owned remains | — |

- **Warm-up.** `Warming` and `Ready` are observable outputs. If the warm-up deadline fires first,
  the session fails with typed cause `WarmupTimeout`. No utterance can be open — result outputs
  require `Ready` — and queued synthesis requests each receive `Cancelled` with reason
  `SessionFailed` in queue order per §6; only an `Enqueue` arriving after session end is
  `Refused(SessionEnded)`. The deadline is driver-fired; the provider reads no clock.
- **Loss.** On loss, the session resolves all open work terminally (`Cancelled`, reason
  `ProviderLost`), emits the `Lost` output with a typed cause, and stops. `Lost` is a session
  output (§5, §6); the reason token `ProviderLost` names the same fact on a cancellation and is
  deliberately not the event's name. If the selection document carries a fallback chain, the host
  re-runs §4 over the chain and starts a **new** session on the first satisfying candidate,
  emitting `FallbackEngaged` — a host output on the speech event stream, never a session output —
  naming both provider identities and the chain position. Utterance and request identities do not
  carry across sessions, and every output of the lost session precedes the successor's first
  output.
- **Cancellation reasons** are one closed-for-meaning, open-for-extension set used by both
  contracts: `Application`, `Replaced`, `CallEnded`, `ProviderLost`, `SessionFailed`, `Shutdown`.

**Disjointness from SIP (RFC 3261).** Provider lifecycle events travel on the speech event stream
and nowhere else. No provider event changes dialog, transaction or call state; no provider failure
is representable as, or reported through, a SIP status code; a call MUST remain established through
warm-up failure, loss and fallback, and ending a call because speech failed is an application
decision, never stack behavior. In the other direction, SIP teardown appears in a session only as
`Cancel` with reason `CallEnded` — reported as cancellation, never as provider failure. A consumer
can therefore always answer "did the call fail, or did speech fail?" from the event type alone.

## 8. Bounds

The host configuration exposes these limits and refuses zero values with a typed error. Lowering
is supported; raising is an explicit host decision. Deadlines bound failure detection only — they
never stand in for an ordering relation, which the vectors assert by event order, not by waiting.

| Resource | Default | At the bound |
|---|---:|---|
| recognition input frames per session | 32 | drop oldest; one `Discontinuity` names the accumulated loss |
| pending non-terminal revisions per utterance | 1 | coalesce; newest revision wins |
| unconsumed terminal + lifecycle outputs per session | 16 | provider output consumption pauses; the input-frame policy absorbs the stall |
| queued synthesis requests per session | 8 | `Refused(QueueFull)` |
| synthesis request text | 8,192 octets | `Refused(TextTooLarge)` |
| synthesis chunk window per session | 4 chunks | provider withholds production until `Drained` |
| warm-up deadline | 30 s | session fails with `WarmupTimeout` |
| drain deadline | 5 s | driver aborts and emits an aborted `Stopped` |

Every queue in this contract is per-session, and sessions are per call: one call's stalled consumer
cannot consume another call's budget, and no queue is shared mutable state (A-28).

## 9. Extensibility record

This section is the public API review for the contract's types, recorded before implementation so
M-55/M-56/A-26/A-27 and downstream providers (X-105) inherit one compatibility rule.

**Extended compatibly (marked `#[non_exhaustive]`):**

- all reason and cause enums: selection refusal reasons (§4), cancellation reasons (§7), session
  failure causes, and the discontinuity kinds (`Loss`/`Overflow`/`Realign`, shared with the seam
  contract);
- all output enums of both contracts and the lifecycle events — consumers must write a wildcard
  arm, so a new event variant is additive;
- descriptor data: `ProviderDescriptor`, voice and device entries, resource estimates — constructed
  through builders/constructors so new fields are additive;
- the device `kind` enum (`Cpu`/`Accelerator`) — a future device class is a new variant, never a
  reinterpretation of an existing one.

**Extended only with defaults:** the provider traits are public and implementable downstream —
that is the point of the contract (X-105's external fixture uses only public traits and types).
Adding a required trait method is therefore a breaking change; a new capability enters as
descriptor data plus a defaulted method. Input enums are `#[non_exhaustive]` for type-level
compatibility, but a driver MUST NOT send an input variant to a provider whose descriptor has not
declared the corresponding capability; a provider receiving an input it does not recognize fails
the session with `ProtocolViolation` rather than guessing.

**Never changed compatibly:** the selection precedence and evaluation order (§4), the
exactly-one-terminal rules and ordering guarantees of §§5–7, and the meaning of any vector in §10.
A change that breaks a §10 vector is incompatible by definition and requires a successor
specification, not an edit.

## 10. Conformance vectors

These vectors are the source of X-105's public testkit suites. Provider-specific tests may add
coverage; they cannot replace these assertions. The **deterministic test provider** exists for both
contract kinds in `sipx-testkit`: it is constructed with an explicit script keyed to input counts,
runs with no accelerator, no model, no network and no runtime, can inject every lifecycle, refusal
and failure transition, produces byte-identical outputs for identical inputs, terminates its open
utterance at every discontinuity, and declares `off_host = false`, `network = false`.

### Discovery

| ID | Vector | Expected |
|---|---|---|
| DIS-1 | read discovery for the deterministic providers | every §3 field present; local/offline holds by property; identical across repeated reads |
| DIS-2 | permute registration order, read again | same descriptors; no selection outcome in SEL/LIF vectors changes |
| DIS-3 | register a descriptor with `off_host = true` | visible in discovery; selectable only past §4 step 2 with the explicit host opt-in |

### Selection and precedence

| ID | Vector | Expected |
|---|---|---|
| SEL-1 | endpoint default only | default selected for both kinds; discovery order irrelevant |
| SEL-2 | compatible per-call override | override selected; endpoint default untouched for other calls |
| SEL-3 | override with an unmatched language range | `UnsupportedLanguage` naming provider, range and declared tags; no substitute selected; the SIP call is unaffected |
| SEL-4 | override naming an absent voice | `UnsupportedVoice`; no other voice is chosen |
| SEL-5 | pinned format with `conversion = deny` not in the provider's list | `UnsupportedFormat`; with `conversion = allow` and an M-43 conversion, selection succeeds |
| SEL-6 | required accelerator capability absent | `UnsupportedDevice`; no silent CPU substitution |
| SEL-7 | nothing configured | `NoProviderConfigured`; nothing selected implicitly |
| SEL-8 | `fallback` of two provider ids, the first incompatible | first candidate's typed refusal recorded, second selected; effective language/voice/format/device equal the top-level document's |
| SEL-9 | override omitting `conversion` and `device` while the endpoint default sets `conversion = deny` and a device requirement | the override reads as `conversion = allow` and no device constraint — the §4 absence meanings, never the endpoint document's values |
| SEL-10 | document missing `language`, synthesis document missing `voice`, or a `fallback` entry that is itself a document | `MalformedSelection` before any registry read |

### Recognition

| ID | Vector | Expected |
|---|---|---|
| REC-1 | scripted frames into the deterministic provider, twice | identical ordered event sequences: `Warming`, `Ready`, then per-utterance `Partial` → `Replacement`* → `Final` |
| REC-2 | script with two revisions before final | revisions strictly increment; each event carries complete text; nothing follows `Final` for that utterance; utterance ids strictly increase |
| REC-3 | stall the provider past the input bound | oldest frames dropped; exactly one `Discontinuity(Overflow)` input names the accumulated span; RTP-side progress unaffected |
| REC-4 | frames, `Discontinuity(Loss)`, frames | all pre-gap outputs precede post-gap outputs; the open utterance terminates at the gap; the next utterance has a new identity |
| REC-5 | `Cancel(Application)` with an utterance open | exactly one `Cancelled(Application)` terminal, then `Stopped`; no output after `Stopped`; no owned buffer or task remains |
| REC-6 | scripted engine failure mid-utterance | `Cancelled(SessionFailed)` for the open utterance, `Failed` with the scripted cause, `Stopped`; the SIP call remains established |
| REC-7 | stop consuming outputs until the terminal bound | revisions coalesce to the newest; no terminal is lost; input frames degrade by the REC-3 policy |
| REC-8 | `Flush` with an open utterance | terminal for the utterance, then `Stopped`; a `Frame` after `Flush` fails the session with `ProtocolViolation` |

### Synthesis

| ID | Vector | Expected |
|---|---|---|
| SYN-1 | one scripted request, twice | byte-identical chunk payloads; `Accepted` → `Started` → chunks with monotonic sequence and contiguous sample-time offsets → `Completed` |
| SYN-2 | enqueue r1, r2, then r3 with `replace = true` | `Cancelled(Replaced)` for r1 then r2, before r3's `Accepted`; r3 plays; exactly one terminal each |
| SYN-3 | fill the request queue, then enqueue | `Refused(QueueFull)`; queued requests unaffected; oversized text yields `Refused(TextTooLarge)` |
| SYN-4 | script a production gap inside one request | a `Discontinuity` naming the gap between chunks whose offsets otherwise remain contiguous; the request still reaches exactly one terminal |
| SYN-5 | cancel the started request mid-stream | at most one further chunk, then `Cancelled`; other queued requests unaffected; session-scope cancel resolves them in queue order |
| SYN-6 | scripted engine failure mid-request | `Failed` for the active request, `Cancelled(SessionFailed)` for each queued request in order, session `Failed`, `Stopped`; the SIP call remains established |
| SYN-7 | withhold `Drained` past the chunk window | the provider emits no further chunk until credit returns; total outstanding chunks never exceed the window |

### Lifecycle

| ID | Vector | Expected |
|---|---|---|
| LIF-1 | start either session kind | `Warming` then `Ready` precede any result or `Started` output; frames offered while `Warming` follow the REC-3 policy |
| LIF-2 | fire the warm-up deadline before `Ready` | queued synthesis requests `Cancelled(SessionFailed)` in order, then session `Failed(WarmupTimeout)`, then `Stopped`; a later `Enqueue` is `Refused(SessionEnded)`; the call remains established |
| LIF-3 | scripted provider loss, no fallback chain | open work `Cancelled(ProviderLost)`, then `Lost`, then `Stopped`; no successor session |
| LIF-4 | scripted provider loss with a satisfying chain | every lost-session output precedes the successor's first output; `FallbackEngaged` names both identities; policy fields unchanged; identities restart |
| LIF-5 | tear down the call mid-session | session-scope cancel with reason `CallEnded`; drains observable via `Stopped`; distinguishable by type from every failure event |
| LIF-6 | fire the drain deadline against a wedged scripted provider | the driver aborts and emits an aborted `Stopped`; nothing owned remains |
