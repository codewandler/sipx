# Spec: the browser SDK contract — `sipx.browser.v1`

**Status:** normative target · **Epic:** `browser-sdk` · **Contract story:** `A-16` ·
**Implementing stories:** `S-41`, `T-33`, `M-52`, `A-17`, `A-18`, `X-100` ·
**Design:** [browser-sdk](../designs/browser-sdk.md) · **Scope:** the WebAssembly ABI, the
JavaScript lifecycle, the package and browser-support policy, and the security boundary of the
browser SDK

Where this document and an implementation disagree, this document is right until it is changed
deliberately. The component specifications remain normative for their own protocols:
[sip-message.md](sip-message.md) for parsing, [sip-transaction.md](sip-transaction.md) for
transaction state, [sip-auth.md](sip-auth.md) for digest authentication,
[sip-tls.md](sip-tls.md) §4 for SIP over WebSocket, and [webrtc-audio.md](webrtc-audio.md) for the
browser-audio media profile. This document defines what crosses the WebAssembly boundary, in which
order, under which bounds, and what the packaged JavaScript surface promises — before any package
or demo turns an accidental interface into a promise.

## 1. Scope and the two SDKs

The browser SDK makes a web page a SIP endpoint. The page registers, dials, answers and hangs up
in its own right: SIP, SDP policy, transactions and dialogs run inside a sans-I/O WebAssembly
kernel compiled from the sipx crates, and the browser supplies everything that touches the outside
world. There is no sipx host process anywhere in this picture.

That is the opposite trust shape from `A-3`'s server-side application SDK, and the two must not be
conflated:

| | Server-side application SDK (`A-3`) | Browser SDK (this contract) |
|---|---|---|
| Wire contract | `sipx.app.v1` per [app-contract.md](app-contract.md) | `sipx.browser.v1`, defined here |
| Who owns the call | a sipx **host** process; the app sends instructions about calls the host owns | the page itself; the kernel in the page *is* the SIP user agent |
| Vocabulary | app-contract verbs (`play`, `gather`, `bridge`, …) over host-owned calls | SIP endpoint operations: `"register"`, `"dial"`, `"answer"`, `"hangup"` |
| Media | never reaches the app | negotiated by the kernel's SDP policy, carried by the browser's native engine |
| Package | `@sipx/app` (working name) | `@sipx/browser` (working name, §7) |

The browser SDK does not speak `sipx.app.v1`, adds none of its verbs, and shares no wire line with
it. A future story that wants app-contract semantics in a page writes a new spec; this one refuses
the merge because the two contracts answer to different owners: an app-contract instruction is a
request to a host that may refuse it, while a browser-SDK command drives the endpoint directly.

This contract is audio-only and endpoint-scoped. §10 lists everything it deliberately omits.

### 1.1 The vision boundary, stated rather than moved

[../vision.md](../vision.md) names "a WebRTC stack" and "a browser media engine" as non-goals.
This contract does not contradict them, so the vision is deliberately not changed by `A-16`; the
boundary is stated here instead, normatively:

**sipx does not implement, and this SDK does not promise, a WebRTC media engine in Rust or
WebAssembly.** Capture, render, ICE connectivity, DTLS-SRTP, SRTP, RTP transport and codec
execution belong to the browser's native `RTCPeerConnection` engine. The compiled Rust is the
sans-I/O signalling and description-policy kernel and nothing else. Video, data channels and SCTP
are refused outright (§3.3, §10). A change to any sentence in this subsection is a vision-level
decision and requires revisiting [../vision.md](../vision.md) first.

## 2. Normative references

- **RFC 3261** — SIP. Registration, dialogs, transactions and the CANCEL/BYE/486 semantics behind
  `"dial"`, `"answer"`, `"hangup"` and refusal. Implemented and specified by
  [sip-message.md](sip-message.md) and [sip-transaction.md](sip-transaction.md); not restated here.
- **RFC 3264** — the offer/answer model the kernel enforces over browser-created descriptions.
- **RFC 7118** — SIP over WebSocket: the `sip` subprotocol, one SIP message per WebSocket message,
  and the client's unroutable `Contact`. The sipx contract is [sip-tls.md](sip-tls.md) §4.
- **RFC 8259** — the control plane (§5) is JSON, UTF-8, no BOM.
- **RFC 4086** — randomness requirements; why entropy is a typed host input and never a fallback
  (§4.7, §8.4).
- **RFC 8825** — WebRTC overview: the separation between signalling, transport and media that makes
  a signalling-only kernel composable with a native media engine.
- **RFC 8827** — WebRTC security architecture: DTLS-SRTP for media, and the binding of the SDP
  fingerprint's trust to the integrity of the signalling channel (§8.5, §8.6).
- **RFC 8829** — JSEP: session descriptions are applied whole, and an answer must not start media
  under terms it did not select.
- **RFC 8866** — SDP. The audio profile the kernel enforces is [webrtc-audio.md](webrtc-audio.md)
  §4 with its §9.4 native-browser boundary; this document does not redefine one line of it.

The browser-side interfaces this contract names — `WebSocket`, `RTCPeerConnection`,
`MediaStreamTrack`, `crypto.getRandomValues`, `performance.now`, WebAssembly — are the web
platform's, defined by its standards bodies and provided by the host browser. They are named as
host facts. This document specifies sipx's use of them, never their internals, and cites no
third-party implementation.

## 3. Division of authority

### 3.1 The kernel (WebAssembly, sans-I/O)

The kernel owns SIP message parsing and serialisation, transaction state, dialog state,
registration state, digest authentication, and SDP **policy** — validating every description,
local and remote, against [webrtc-audio.md](webrtc-audio.md) §4 before it crosses to the other
side. The kernel is a pure state machine: bytes in, fired timers in, monotonic time in, entropy
in; bytes out, timer requests out, typed events out. It has **no** WebAssembly imports — no
socket, no clock read, no `Date`, no ambient randomness, no host callback. Everything it knows
arrived through an exported entry point, which is what makes its vectors (§9) runnable identically
in native Rust and in WASM (`S-41`).

### 3.2 The browser (host platform)

The browser owns WebSocket/WSS signalling transport, `RTCPeerConnection` — and with it ICE
connectivity, DTLS-SRTP, SRTP, RTP and codec execution — media capture and render, permission
prompting, timers, monotonic time and cryptographic entropy. SDP is authored and consumed by the
browser (`createOffer`, `createAnswer`, `setRemoteDescription`); the kernel is its gatekeeper in
both directions, never its author in this mode.

### 3.3 Refused outright

This SDK refuses, at the contract level and not as a missing feature:

- **video** — no video media section is offered, answered or tolerated
  ([webrtc-audio.md](webrtc-audio.md) §4.1's one-audio-section rule stands);
- **data channels and SCTP** — no `m=application` section, no `RTCDataChannel` surface;
- **a WebRTC engine implemented in Rust or compiled to WebAssembly** — no ICE agent, DTLS stack,
  SRTP transform or jitter buffer of sipx's crosses into the browser build (§1.1);
- **a second SIP or media library underneath** — the SDK composes the browser's native engine and
  nothing else.

An implementation that "helpfully" accepts any of these has left this contract.

### 3.4 The server side

The SIP service the page registers against is out of scope. This contract constrains only what
the SDK sends and accepts: WSS by default, the `sip` WebSocket subprotocol, and the audio profile.

## 4. The kernel ABI

### 4.1 Module shape

One WebAssembly module (asset working name `sipx_browser.wasm`), exporting linear `memory` and the
functions in §4.3, importing **nothing**. The module uses no threads, no atomics and no shared
memory, and MUST remain loadable in a non-cross-origin-isolated context (§8.7). Declared maximum
linear memory is 32 MiB (§4.9). One module instantiation serves one JavaScript agent; sharing an
instance across workers is outside the contract.

Because the module imports nothing, the kernel can never call the host: reentrancy is structurally
impossible, not merely forbidden. All outputs are queued inside the kernel and drained by the host
(§4.6). Every entry point completes all resulting work before it returns; there is no deferred or
background execution.

### 4.2 Value conventions

| Convention | Rule |
|---|---|
| scalars | `i32`/`i64` only; pointers and lengths are `u32` byte offsets into the exported memory |
| handles | opaque positive `i32`, allocated by `sipx_kernel_new`, never reused within an instantiation — use-after-free is deterministically `E_INVALID_HANDLE` |
| results | `0` success, negative `i32` from §4.10 on host-contract violation |
| time | `now_ms`: `u64` milliseconds on a host-chosen monotonic epoch (`performance.now`, truncated); wall-clock time never crosses the ABI |
| control plane | UTF-8 JSON (§5) in host- or kernel-owned buffers |
| byte plane | raw SIP bytes, opaque to JSON — a SIP message is never wrapped in a JSON string across the ABI |
| packed buffer | a `u64` with pointer in the high 32 bits, length in the low 32; `0` means "none"; pointer `0` with nonzero length carries an error-code magnitude in the length field |

### 4.3 Exports

| Export | Signature | Semantics |
|---|---|---|
| `sipx_abi_version` | `() -> i32` | the ABI integer, `1` for this document; generated glue MUST refuse a mismatch at load |
| `sipx_alloc` | `(len: u32) -> u32` | allocate a host-input buffer; `0` on failure |
| `sipx_free` | `(ptr: u32, len: u32) -> ()` | release a buffer obtained from `sipx_alloc` |
| `sipx_kernel_new` | `(cfg_ptr: u32, cfg_len: u32) -> i32` | create a kernel from a `BSDK-CFG` JSON document; returns a handle or a negative error |
| `sipx_kernel_free` | `(handle: i32) -> i32` | cancel everything and destroy the kernel (§6.5); idempotent-safe: a second call is `E_INVALID_HANDLE` |
| `sipx_command` | `(handle: i32, ptr: u32, len: u32, now_ms: u64) -> i32` | submit one §5.2 command |
| `sipx_input_bytes` | `(handle: i32, ptr: u32, len: u32, now_ms: u64) -> i32` | one received signalling message (one WebSocket message = one SIP message, RFC 7118 §5) |
| `sipx_input_timer` | `(handle: i32, timer_id: u64, now_ms: u64) -> i32` | a previously requested timer fired |
| `sipx_input_entropy` | `(handle: i32, ptr: u32, len: u32) -> i32` | append host entropy to the pool (§4.7) |
| `sipx_next_output` | `(handle: i32) -> u64` | packed buffer of the next output record (§4.6); `0` when drained |
| `sipx_snapshot` | `(handle: i32) -> u64` | packed buffer of a read-only JSON state/counter snapshot (§4.11) |

### 4.4 Ownership

- **Host inputs.** The host allocates with `sipx_alloc`, writes, and calls the entry point. When
  the entry point returns, the buffer belongs to the host again and MUST be released with
  `sipx_free`. The kernel copies whatever it retains; it never keeps a reference into a host
  buffer.
- **Kernel outputs.** The buffer returned by `sipx_next_output` or `sipx_snapshot` is borrowed: it
  is valid until the next call of **any** export on the same handle (or `sipx_kernel_free`), and
  the host MUST copy out what it needs before making that call. The host never frees kernel-owned
  buffers.
- **Out-of-range access.** A pointer/length pair that leaves the buffer's bounds or was not
  obtained as required is `E_BAD_POINTER`; the kernel state is unchanged.

### 4.5 Time and timers

Every state-advancing entry point carries `now_ms`. Per handle, `now_ms` MUST be non-decreasing;
a regression returns `E_TIME` and changes nothing. The kernel schedules through output records:
`TIMER_SET` carries a fresh `u64` timer id (monotonically increasing, never reused per handle) and
an absolute `fire_at_ms`; `TIMER_CANCEL` names an id the host should clear. The host fires a timer
by calling `sipx_input_timer` at or after `fire_at_ms`. Firing an unknown, cancelled or already
fired id returns `0` and increments `stale_timer_fires` — host races are inevitable and are not
errors. All SIP timer values (T1 and friends) remain [sip-transaction.md](sip-transaction.md)'s.

### 4.6 Output records and the drain obligation

After every call to `sipx_command`, `sipx_input_bytes`, `sipx_input_timer` or
`sipx_input_entropy`, the host MUST loop on `sipx_next_output` until it returns `0`. Records are
strictly FIFO. Each record is:

```text
offset 0: u32 little-endian  record type
offset 4: u32 little-endian  payload length N
offset 8: N payload bytes
```

| Type | Name | Payload |
|---:|---|---|
| `1` | `WIRE` | raw bytes of exactly one SIP message, to be sent as one WebSocket message |
| `2` | `TIMER_SET` | `u64` LE timer id, then `u64` LE `fire_at_ms` (16 octets) |
| `3` | `TIMER_CANCEL` | `u64` LE timer id (8 octets) |
| `4` | `EVENT` | one §5.3 JSON event document |

### 4.7 Entropy

The kernel's randomness is a host input, never an ambient capability. The host MUST feed bytes
obtained from `crypto.getRandomValues` — nothing else — via `sipx_input_entropy`. The kernel
maintains a pool: capacity 1024 octets, low-water mark 64. When the pool drops below the low-water
mark the kernel emits the `"need-entropy"` event (`BSDK-EVT-1`); feeding beyond capacity is
`E_BOUNDS` with the pool unchanged. An operation that needs more entropy than the pool holds fails
whole with `E_ENTROPY` — no partial consumption, no weaker generator, no silent reuse (§8.4).

Identifiers are derived from the pool as an ordered tape, so that a pinned tape yields pinned
identifiers (`BSDK-ENT-1`):

| Identifier | Consumes | Form |
|---|---:|---|
| Call-ID | 16 octets | 32 lowercase hex characters, no `@host` part |
| From/To tag | 8 octets | 16 lowercase hex characters |
| Via branch | 8 octets | `z9hG4bK` + 16 lowercase hex characters |
| digest cnonce | 16 octets | 32 lowercase hex characters, per [sip-auth.md](sip-auth.md) |

Consumption order is the order the identifiers are first required: a new registration or dialog
consumes Call-ID then From-tag; each new client transaction consumes its branch at serialisation;
a challenge response consumes its cnonce when the authorization header is built. Each derivation
consumes its octets atomically from the front of the tape.

### 4.8 Reentrancy and concurrency

Entry points are non-reentrant by construction (§4.1): the kernel never calls out, so no host code
runs while a kernel frame is on the stack. The host side of the contract: one kernel handle is
driven from one JavaScript agent only, calls are made one at a time, and the borrowed-buffer rule
in §4.4 is the only cross-call obligation. The generated glue enforces single-agent use; the
handwritten layer (§6) additionally guarantees that user callbacks never run inside a kernel call
(§6.4).

### 4.9 Memory and resource bounds

| Resource | Bound | Behaviour at the bound |
|---|---:|---|
| declared linear memory maximum | 32 MiB | `memory.grow` failure surfaces as `E_OOM`; the failed operation is aborted whole and kernel state is unchanged |
| live handles per instantiation | 16 | `sipx_kernel_new` returns `E_LIMIT` |
| one SIP message, in or out | 64 KiB | inbound: `E_BOUNDS`, message not parsed ([sip-tls.md](sip-tls.md) §4's decoder bound); outbound over the bound is a kernel defect |
| one command document | 32 KiB | `E_BOUNDS` before JSON parsing |
| one SDP body inside a command | 16 KiB | typed refusal `sdp-too-large` in the command outcome |
| one event document | 32 KiB | kernel MUST NOT emit larger; truncation is forbidden, oversize is a defect |
| entropy pool | 1024 octets, low-water 64 | §4.7 |
| pending timers | 128 | exceeding is a kernel defect → poisoned (below) |
| queued output records | 256 records or 256 KiB total | reachable only by a host that violates §4.6's drain obligation → poisoned |
| concurrent calls | 8 | ninth outbound `"dial"`: typed refusal `call-limit`; ninth inbound INVITE: automatic `486 Busy Here`, counted `refused_incoming` |
| registrations per kernel | 1 AOR | a second concurrent registration identity is a typed refusal |
| create/free cycles | unbounded | repeated `sipx_kernel_new`/`sipx_kernel_free` MUST return linear memory use to its baseline (`S-41` proves this) |

**Poisoned.** If an internal invariant fails — the situations that would panic natively — the
kernel records the fault and every subsequent entry point except `sipx_kernel_free` and
`sipx_next_output` returns `E_POISONED`; draining remains legal so the host can retrieve the fatal
`"error"` event. A WebAssembly trap is a defect of the same class observed later: the glue MUST
treat any trap as fatal to the instance — destroy it, surface `SipxAbiDefect`, and never call into
that instance again.

### 4.10 Error codes

ABI error codes report **host-contract violations**, not protocol outcomes. Malformed SIP arriving
in `sipx_input_bytes` returns `0` — hostile network input is a value ([../vision.md](../vision.md)
principle 2), handled inside the kernel with typed internal errors and counters, exactly as the
native stack does. A SIP request that fails is reported through events, not through return codes.

| Code | Name | Meaning |
|---:|---|---|
| `-1` | `E_INVALID_HANDLE` | unknown, freed or foreign handle |
| `-2` | `E_BAD_POINTER` | pointer/length outside the exported memory or not per §4.4 |
| `-3` | `E_UTF8` | control-plane buffer is not UTF-8 |
| `-4` | `E_JSON` | control-plane buffer is not RFC 8259 JSON |
| `-5` | `E_SCHEMA` | JSON valid, document not a §5 command: unknown `"cmd"`, missing or ill-typed field |
| `-6` | `E_STATE` | command is well-formed but illegal in the current state (§5.4, §6.2) |
| `-7` | `E_BOUNDS` | an input exceeds §4.9 |
| `-8` | `E_ENTROPY` | pool cannot cover the operation |
| `-9` | `E_OOM` | linear memory growth failed |
| `-10` | `E_LIMIT` | a countable resource cap in §4.9 |
| `-11` | `E_TIME` | `now_ms` regressed |
| `-12` | `E_POISONED` | a prior internal fault; instance is dead |

The mapping to JavaScript error classes is §6.6. Codes are stable within `sipx.browser.v1`: a new
code may be appended; an existing value never changes meaning.

### 4.11 Snapshot

`sipx_snapshot` returns a JSON document with the kernel's current registration state, per-call
states, pool level, pending timer count, and monotonic counters (`parse_errors`,
`stale_timer_fires`, `refused_incoming`, `dropped_after_close`, and the §4.10 rejection counts).
It never contains credentials, entropy bytes, or SIP message bodies. It is a diagnostic read: a
snapshot is individually consistent but concurrent progress is not transactional across fields.

## 5. The control plane — `sipx.browser.v1`

### 5.1 Envelope

Commands and events are single JSON objects. Every command carries `"v":1`, a `"cmd"` verb, and a
host-chosen positive integer `"id"` unique among that kernel's unfinished commands. Every event
carries `"v":1` and an `"evt"` type. Within wire line `v` 1, both sides MUST ignore unknown
*fields*; the JavaScript layer MUST ignore an unknown *event type* (counted as drift diagnostics);
an unknown *command verb* is `E_SCHEMA` — a kernel that skipped a verb would run a different
program than the page wrote. A change that breaks any §9 vector requires `sipx.browser.v2` and an
ABI integer bump.

For vector determinism the kernel emits canonical JSON: UTF-8, no insignificant whitespace, fields
in the order this section defines them.

### 5.2 Commands (host → kernel)

| Verb | Fields | Meaning |
|---|---|---|
| `"register"` | `"expires"` (seconds, ≥ 1) | register the configured AOR; the kernel owns refreshes and digest challenges |
| `"unregister"` | — | deregister (`expires: 0`) and stop refreshing |
| `"dial"` | `"target"` (SIP URI) | new outbound call; the kernel allocates a call number and asks for local media before any SIP is sent |
| `"ring"` | `"call"` | send `180 Ringing` for an incoming call |
| `"answer"` | `"call"` | answer an incoming call; triggers `"need-local-media"` if no local description is staged |
| `"reject"` | `"call"`, `"status"` (300–699) | refuse an incoming call with a final response |
| `"hangup"` | `"call"` | end a call in any state: CANCEL before the final response, BYE after it, `486` for an unanswered incoming call |
| `"local-media"` | `"call"`, `"kind"` (`"offer"`\|`"answer"`), `"sdp"` | the browser-created description, validated against [webrtc-audio.md](webrtc-audio.md) §4 before the kernel carries it in SIP |
| `"media-applied"` | `"call"` | the browser accepted the remote description (`setRemoteDescription` succeeded) |
| `"media-failed"` | `"call"`, `"reason"` | the browser refused the remote description; the kernel completes the SIP exchange it owes (ACK) and ends the call |

Command completion is reported by exactly one `"outcome"` event carrying the command's `"id"` —
at protocol completion, not at acceptance: `"register"`'s outcome follows the final response to
REGISTER, not the entry-point return. There is no generic cancel verb; cancellation is the inverse
verb (`"hangup"` cancels `"dial"`, `"unregister"` cancels `"register"`), and §6.3 maps
`AbortSignal` onto that rule.

### 5.3 Events (kernel → host)

Call events are snapshots, not deltas: each `"call"` and `"registration"` event replaces the
previous state wholesale, so a missed delivery cannot leave the page permanently wrong — the same
discipline [app-contract.md](app-contract.md) established for its own events.

| Type | Fields | Meaning |
|---|---|---|
| `"need-entropy"` | `"min"` | pool below low-water; feed `crypto.getRandomValues` bytes |
| `"registration"` | `"state"` (`"registering"`\|`"registered"`\|`"unregistered"`\|`"failed"`), `"expires"?, "status"?, "reason"?` | registration state replaced wholesale; the kernel never auto-retries a failed registration beyond its owed refresh — retry policy belongs to the application (bounded, per `T-33`'s no-unbounded-retry rule) |
| `"call"` | `"call"`, `"dir"` (`"in"`\|`"out"`), `"state"` (§5.4), `"from"?, "to"?` | call state replaced wholesale |
| `"need-local-media"` | `"call"`, `"kind"`, `"constraints"` (`{"audio":true,"video":false}`, always) | the browser must produce a description of that kind |
| `"remote-media"` | `"call"`, `"kind"`, `"sdp"` | a profile-validated remote description for `setRemoteDescription`; hostile or off-profile SDP is refused inside the kernel and never reaches this event |
| `"call-ended"` | `"call"`, `"cause"` (`{"class": "local"\|"remote"\|"refused"\|"sip"\|"media"\|"timeout", "status"?, "reason"?}`) | final event for that call number |
| `"outcome"` | `"id"`, `"ok"`, `"error"?` (`{"code","reason"}`) | the single completion of one command |
| `"error"` | `"fatal"`, `"code"`, `"reason"` | kernel-level fault; `"fatal":true` accompanies the poisoned state |

### 5.4 Kernel call states

One table per direction; the kernel refuses commands outside the listed rows with `E_STATE`.
State never moves backwards.

Outbound:

| State | Input | Action and next state |
|---|---|---|
| `Dialing` | `"dial"` accepted | emit `"need-local-media"` (offer); **no SIP yet** — permission failure before media costs no signalling |
| `Dialing` | `"local-media"` offer, profile-valid | serialise INVITE → `InviteSent` |
| `Dialing` | `"local-media"` off-profile | typed outcome failure; call → `Ended(media)` |
| `Dialing` | `"hangup"` | no SIP owed; `Ended(local)` |
| `InviteSent` | 1xx | `Ringing`, event `"call"` |
| `InviteSent`/`Ringing` | 2xx with SDP answer | validate profile; valid → emit `"remote-media"` (answer), hold the ACK → `AnswerDelivered`; invalid → ACK then BYE, `Ended(media)` |
| `InviteSent`/`Ringing` | 3xx–6xx | ACK per RFC 3261; `Ended(sip, status)` |
| `InviteSent`/`Ringing` | `"hangup"` | CANCEL; on the 487 exchange completing, `Ended(local)` |
| `AnswerDelivered` | `"media-applied"` | send ACK → `SipEstablished` |
| `AnswerDelivered` | `"media-failed"` | send ACK then BYE → `Ended(media)` |
| `SipEstablished` | `"hangup"` / BYE received | BYE exchange → `Ended(local)` / `Ended(remote)` |

Inbound:

| State | Input | Action and next state |
|---|---|---|
| — | INVITE, offer profile-valid | allocate call → `Incoming`; events `"call"` then `"remote-media"` (offer) |
| — | INVITE, offer off-profile | respond `488` with no media resources; counted `refused_incoming`; no call object |
| `Incoming` | `"ring"` | send 180; stay `Incoming` |
| `Incoming` | `"answer"` | emit `"need-local-media"` (answer) → `AnswerPending` |
| `Incoming` | `"reject"` / `"hangup"` | final status (`"hangup"` = 486) → `Ended(refused)` |
| `Incoming` | CANCEL received | 487 exchange → `Ended(remote)` |
| `AnswerPending` | `"local-media"` answer, profile-valid | send 200 → `AnswerSent` |
| `AnswerPending` | `"local-media"` off-profile | typed outcome failure; send 488 → `Ended(media)` |
| `AnswerPending` | `"hangup"` | send 486 → `Ended(local)` |
| `AnswerSent` | ACK received | `SipEstablished` |
| `AnswerSent` | ACK timeout ([sip-transaction.md](sip-transaction.md) timers) | BYE → `Ended(timeout)` |
| `SipEstablished` | as outbound | as outbound |

The kernel's `"call"` events carry the nonterminal states verbatim (lower-camel forms:
`"dialing"`, `"inviteSent"`, `"ringing"`, `"answerDelivered"`, `"incoming"`, `"answerPending"`,
`"answerSent"`, `"sipEstablished"`). Every `Ended(…)` row emits the `"call-ended"` event with its
cause instead of a `"call"` state — there is exactly one terminal notification per call, never
two spellings of it. "The call is established" as presented to the application is a stricter,
combined fact defined in §6.2.

### 5.5 The negotiated-media report

The SDK reports what was actually negotiated as two labelled fact sets, and never infers one from
the other (the discipline [browser-audio-proof.md](browser-audio-proof.md) §3 established):

| Origin | Facts |
|---|---|
| `kernel` | dialog identity, negotiated answer's codec list and selected primary codec, fingerprint algorithm presence, `a=setup` role, `rtcp-mux` presence — everything a description states |
| `browser` | selected candidate pair, DTLS transport state, SRTP cipher/profile, inbound/outbound packet and byte counts — everything only the engine's statistics interfaces can state |

A missing browser statistic is a missing field, never a kernel substitute. The JavaScript surface
exposes the combined report (`call.negotiatedMedia()`, §6.1) with each fact's origin preserved,
and a call is never presented as `established` while the facts required by §6.2 are absent.

## 6. The JavaScript lifecycle

This section is normative for the handwritten layer `A-17` packages. The layer adds ergonomics —
promises, `AbortSignal`, event subscription — and **no protocol state hidden from the kernel**: every
SIP-visible fact it reports is a kernel fact, and every media fact is a browser fact.

### 6.1 Surface shape

```ts
const client = await SipxClient.create(config);   // loads WASM, checks capabilities, opens WSS
await client.register({ expires: 600, signal });
const call = await client.dial("sip:bob@example.net", { signal });
client.on("incoming", (call) => call.answer());
call.on("state", (s) => ...);
const report = call.negotiatedMedia();            // §5.5, origins preserved
call.mute(true);                                   // local track fact: track.enabled — never SIP
await call.hangup();
await client.close();
```

`mute` is deliberately a local media-track operation with no kernel verb and no SIP signalling in
v1 (no hold re-INVITE — §10). Device selection wraps the platform's device enumeration and is
likewise kernel-invisible.

### 6.2 Client and call states

| State | Meaning |
|---|---|
| `starting` | capabilities checked (§7.3), WASM instantiated, entropy seeded, WSS connecting |
| `connected` | WSS open with the `sip` subprotocol; not registered |
| `registered` | kernel registration state `registered` |
| `closing` | §6.5 teardown running |
| `closed` | terminal; no callback fires after it |

A capability or WSS failure in `starting` rejects `create()` typed; there is no half-started
client.

Call states presented to the application: `dialing`, `ringing`, `incoming`, `answering`,
`established`, `ending`, `ended`, `failed`. The mapping from kernel states is direct except for
the one combined gate:

**`established` requires all of:** kernel state `sipEstablished`, **and** the
`RTCPeerConnection`'s connection state is `connected` — DTLS complete under the browser's
fingerprint check (RFC 8827) — **and** the §5.5 kernel facts show the profile held (`rtcp-mux`,
fingerprint, one audio section). A call whose media never connects within the setup bound is ended
(BYE) with `cause.class = "media"` and presented as `failed`, not as connected (`M-52`'s
fail-closed rule). After `established`, a peer-connection failure ends the call the same way; a
transient `disconnected` is surfaced as a diagnostic event and does not tear down by itself.

### 6.3 Cancellation

Every awaited verb accepts an `AbortSignal` and has a defined canceller — the inverse verb:

| Awaited verb | Abort maps to | Settled as |
|---|---|---|
| `register` | `unregister` | rejection with `SipxCancelled` after the kernel reports |
| `dial` | `hangup` (CANCEL or local release per §5.4) | `SipxCancelled`; any tracks acquired for the call are stopped **before** the promise settles |
| `answer` | `hangup` (486 path) | `SipxCancelled`, tracks stopped |
| `hangup`, `close` | not abortable; they are the cancellation | — |

Cancellation is idempotent and never detaches work: the promise settles only after the kernel and
media cleanup it names have completed.

### 6.4 Callback ordering

1. Events are delivered in kernel emission order, one at a time; each event's listener list runs
   to completion before the next event is dispatched.
2. No listener runs synchronously inside an SDK method call or inside the WebSocket/timer handler
   that drove the kernel; delivery is always a separate task on the agent's event loop.
3. A command's promise settles after the listeners of the state events the kernel ordered before
   that command's `"outcome"` (§5.2) have run.
4. `call.on("state", …)` observes every state exactly once, in order; `"ended"` is a call's final
   callback, and no callback for that call follows it.
5. A throwing listener is reported through the client's diagnostic hook and does not stop
   delivery to remaining listeners or later events.
6. After `close()` resolves — and equally after a fatal `SipxAbiDefect` — no callback fires.
   Undelivered queued events are dropped and counted (`dropped_after_close`).

### 6.5 Teardown order

`client.close()`, page teardown and fatal defects all use one path, in this order:

1. stop accepting new commands; reject them with `SipxStateError`;
2. hang up every live call (§5.4's per-state rule), bounded by the close deadline;
3. unregister if registered, bounded by the same deadline;
4. `sipx_kernel_free` — the kernel cancels its state and the glue clears every scheduled host
   timer it owns;
5. close the WebSocket;
6. stop every `MediaStreamTrack` the SDK acquired and close every `RTCPeerConnection` it created;
7. resolve `close()`; deliver `closed` as the final event.

A deadline expiry skips forward — it never leaves steps 4–6 unrun and never detaches them. On
`pagehide`/page destruction the SDK performs steps 4–6 synchronously as best effort; the endpoint
dies with the page, and no track or peer connection may outlive it (§8.8).

### 6.6 Error taxonomy

| Class | Sources | Notes |
|---|---|---|
| `SipxAbiDefect` | `E_BAD_POINTER`, `E_UTF8`, `E_JSON`, `E_SCHEMA`, `E_POISONED`, `E_TIME`, any trap | a bug in glue or kernel, never user input; instance is destroyed |
| `SipxStateError` | `E_STATE`, commands after `close()` | |
| `SipxLimitError` | `E_LIMIT`, `E_BOUNDS`, `E_OOM`, `call-limit` | |
| `SipxTransportError` | WebSocket failure, close, offline, subprotocol refusal | `T-33`'s typed events surface here |
| `SipxSipError` | a SIP final response outcome: `{status, reason}` | distinct from every media class |
| `SipxMediaError` | `{kind: "permission" \| "device" \| "autoplay" \| "negotiation" \| "track-ended", …}` | browser media failures never masquerade as SIP failures (`M-52`) |
| `SipxCancelled` | §6.3 | a typed outcome, not an exception subclass of failure |
| `SipxCapabilityError` | §7.3 load-time feature detection | names the missing interface |

`E_ENTROPY` never surfaces as an application error: the glue owns the feed obligation (§4.7), and
its failure to feed is a `SipxAbiDefect`. Error strings never contain credentials, authorization
headers, entropy bytes or full SIP messages (§8.3).

## 7. Packages, generation and support policy

### 7.1 Names

Working names, recorded so the artifacts land consistently — **not** claims of stable 1.0 APIs:

| Artifact | Working name |
|---|---|
| npm package (glue + types + handwritten layer + WASM asset) | `@sipx/browser` |
| WebAssembly asset inside it | `sipx_browser.wasm` |
| Rust kernel crate (feature-gated, workspace) | `sipx-wasm` |

One npm package: the WASM asset ships inside it, resolved same-origin by the consumer's bundler
or server; the package never fetches code or WASM from a remote origin at runtime (§8.7). ESM
only; browser-targeted; no Node runtime dependency (`A-17`).

### 7.2 Generated versus handwritten

| Surface | How produced | Why |
|---|---|---|
| ABI bindings (§4.3), command/event/error types (§4.10, §5) | **generated** from one checked source in the Rust workspace; a drift test fails when Rust and the package disagree (`A-17`) | vocabulary must be incapable of drifting from the kernel |
| lifecycle layer (§6) | **handwritten** | its worth is exactly the designed ergonomics; it adds no vocabulary and no hidden protocol state |
| demo site | handwritten, imports the packed SDK only (`A-18`) | the demo is a consumer, not a second implementation |

The generated/handwritten line is a contract: a convenience that cannot be expressed over §4 and
§5 does not belong in the handwritten layer — it belongs in this spec first.

### 7.3 Supported browser policy

The policy is capability-based; brand and version enumeration is operational provisioning that
lives in `X-100`'s matrix configuration, and the public compatibility claim is carried by that
matrix's results — never by this document naming products
([browser-audio-proof.md](browser-audio-proof.md) §2's rule).

Required capabilities, feature-detected at `SipxClient.create` and failing closed with
`SipxCapabilityError` naming the first missing one:

| Capability | Why |
|---|---|
| WebAssembly (core, single-threaded; no threads, no shared memory) | the kernel |
| secure context | WSS, capture and WebCrypto all require it |
| `WebSocket` with subprotocol negotiation | RFC 7118 §4's `sip` subprotocol |
| `RTCPeerConnection` audio with DTLS-SRTP, `rtcp-mux` and unified descriptions | the media engine (§3.2) |
| media capture with per-track stop | §8.8's ownership rules |
| `crypto.getRandomValues` | §4.7; there is no fallback |
| `performance.now` | monotonic `now_ms` |
| ES module loading | the package format |

Capability presence is necessary, not sufficient: a browser is *supported* when `X-100`'s matrix
proves both call roles on it. The support statement published with the package is generated from
that matrix.

### 7.4 Versioning

- The npm package is pre-1.0 and states its **experimental** status at install and import, as
  `A-3`'s package does. No stable 1.0 API is claimed by this contract, and completing the epic
  does not imply one; 1.0 is a separate deliberate decision after `X-100` carries the matrix.
- `sipx_abi_version` (§4.3) is the compiled contract: generated glue refuses a mismatched kernel
  at load, before any call.
- Within `sipx.browser.v1`: additive fields, additive event types and appended error codes are
  compatible (§5.1). Anything that breaks a §9 vector is `sipx.browser.v2` plus an ABI bump plus a
  semver-breaking package release.
- The package follows semver on that rule once 1.0 exists; before 1.0, a breaking change is a
  minor bump with a CHANGELOG entry, per the workspace's existing pre-1.0 practice.

## 8. Threat analysis

Assets: the user's credentials, the user's microphone, call confidentiality and integrity, and the
page's origin. The attacker models per threat:

### 8.1 Untrusted SIP bytes

Every byte entering `sipx_input_bytes` is attacker-controlled (a compromised or hostile server, or
anything that can write to the socket). The kernel parses under
[sip-message.md](sip-message.md)'s rules: bounds before parsing (§4.9), typed errors, no panic —
and in WASM a panic is a trap, so the no-panic rule is also an availability rule. Any trap is
treated as instance-fatal (§4.9) rather than retried. `S-41` MUST run the existing RFC 4475/5118
torture corpora against the WASM build with native-identical outcomes; the fuzzing obligation on
the parser is unchanged by the new target.

### 8.2 Hostile SDP

A description is attacker input twice: once to the kernel's parser, once to the browser's. The
kernel is the gatekeeper (§3.1): a remote description reaches `"remote-media"` — and therefore
`setRemoteDescription` and the browser's parser — only after full validation against
[webrtc-audio.md](webrtc-audio.md) §4, and a local description is validated before it is carried
in SIP. Off-profile input (SDES `a=crypto`, plain RTP, video sections, data-channel sections,
second media sections) is refused typed inside the kernel; no string-spliced SDP mutation exists
anywhere in the SDK (`M-52`).

### 8.3 Script-visible credentials

The trust model is stated rather than papered over: **any script running in the origin can read
anything the SDK holds**, including configuration credentials and WASM linear memory — an XSS is
a full credential compromise, and no in-page mechanism changes that. What the contract does
control: credentials are never placed in URLs, logs, thrown error strings, events, snapshots
(§4.11) or the negotiated-media report; the kernel zeroises credential and entropy buffers on
`sipx_kernel_free` as hygiene, documented as *not* a confidentiality boundary. Deployments SHOULD
issue short-lived, per-device credentials from their application server rather than embedding
long-lived secrets; the demo (`A-18`) must model that posture and ship a restrictive content
security policy.

### 8.4 Entropy

Predictable Call-IDs, tags, branches or cnonces make dialogs guessable and digest responses
replayable (RFC 4086). Hence §4.7: entropy is fed only from `crypto.getRandomValues`; the kernel
refuses to derive identifiers from an insufficient pool with `E_ENTROPY` — there is no
time-seeded, counter-seeded or constant fallback path, and `Math.random` is forbidden in every
SDK source file. The derivation tape is deterministic *given* the fed bytes; the security of the
identifiers is exactly the security of the fed bytes, which is why nothing but the platform CSPRNG
may feed them.

### 8.5 Wrong fingerprints

Media authenticity hangs on the SDP fingerprint (RFC 8827): the browser verifies the DTLS peer
certificate against `a=fingerprint`, and the kernel guarantees the descriptions that cross it
carry a well-formed SHA-256 fingerprint at all ([webrtc-audio.md](webrtc-audio.md) §4's
`FingerprintRequired`/§6.1 rules). The SDK MUST NOT present `established` before the browser
reports the DTLS transport connected (§6.2), MUST surface the fingerprint facts in the §5.5
report, and MUST NOT offer any API that relaxes certificate checking.

### 8.6 Insecure signalling

A fingerprint carried over unauthenticated signalling authenticates nothing (RFC 8827's binding).
WSS is therefore the default and the contract: plain `ws:` requires an explicit
development-only configuration (`"insecure":"allow-development"`), and with it the kernel refuses
to answer a digest challenge — credentials do not cross a transport the host declared insecure.
Secure contexts make mixed-content `ws:` unusable on deployed pages anyway; the flag exists for
local development against loopback, and `T-33` owns the socket-side enforcement.

### 8.7 Cross-origin isolation and origin policy

The SDK requires **no** cross-origin isolation: no `SharedArrayBuffer`, no threads, no
high-resolution-timer escalation, so no page is pushed into COOP/COEP configurations it did not
want. Under content security policy the SDK needs same-origin script/wasm evaluation for its own
module and WASM asset only — it never uses string evaluation, never injects script, and never
loads code or WASM from a remote origin at runtime (§7.1). The demo publishes a restrictive CSP
as the worked example (`A-18`).

### 8.8 Leaked media tracks

A live microphone track that outlives its call is a wiretap. Ownership rule: every track and
peer connection the SDK acquires belongs to exactly one call; `call.hangup()`, `reject`, failed
dial, aborted answer, `close()`, fatal defects and page teardown all reach §6.5 steps 4–6, and a
call's terminal event fires only after its tracks are stopped and its peer connection closed
(§6.3). The SDK never hands out a track it acquired without recording it against the owning call,
and `X-100` asserts zero residual tracks, sockets and timers after every case, positive and
negative.

## 9. Vectors

### 9.1 Encoding convention

JSON vectors are UTF-8, exactly the displayed characters on one line, with **no trailing
newline**. Binary vectors are given as hex octets. Tests MUST consume these vectors or derive
fixtures byte-for-byte from them; `S-41` runs them against native Rust and WASM and requires
identical results, and `A-17`'s drift test holds the generated types to the same bytes.

| ID | Octets | SHA-256 |
|---|---:|---|
| `BSDK-CFG-1` | 178 | `018dc212a2ff5646bc36a9737e28f9403407251d26eb23d77e1a1d11f7d20249` |
| `BSDK-CMD-1` | 45 | `73f99097e0a7dd0d96276ddf13c723cdb8e0e4da696d4d2440f98b0b6c5b26e0` |
| `BSDK-CMD-2` | 58 | `fdfd4cb1d02483cdc87a86d71d395285bf4f82f46760cd8e8e8b63f0448f26c3` |
| `BSDK-CMD-3` | 38 | `c04df0adf181eebea4dd15be89ff258b9549641807f28bd5366e55de9b6806ee` |
| `BSDK-EVT-1` | 37 | `f2e4ac91f369ca513024f07b09135a0adf279b82b76eb8252877cc78c1614037` |
| `BSDK-EVT-2` | 63 | `c500a036d7ccef02c9f27834703a26be2db6398808fad8b0c428d767d78799c7` |
| `BSDK-EVT-3` | 99 | `63ece231c4c76af024d701aca7558611a99cfaf3e6c504c2eebbbbe070d4ed4a` |
| `BSDK-OUT-1` | 45 | `e94b52e04f1ee6991926e77805024290f88906c7a1c027c32afea07ac85975e6` |
| `BSDK-OUT-2` | 24 | `5f35832f0b2d782d3da8d35a53f98a21f0ddb6fac1520e8cdbb0280297fc8ac2` |

### 9.2 Control-plane byte vectors

`BSDK-CFG-1` — a complete configuration for `sipx_kernel_new`:

```text
{"v":1,"aor":"sip:alice@example.net","auth":{"username":"alice","password":"secret"},"transport":{"scheme":"wss","host":"edge.example.net","resource":"/sip"},"insecure":"refuse"}
```

`BSDK-CMD-1` — register:

```text
{"v":1,"cmd":"register","id":1,"expires":600}
```

`BSDK-CMD-2` — dial:

```text
{"v":1,"cmd":"dial","id":2,"target":"sip:bob@example.net"}
```

`BSDK-CMD-3` — hangup:

```text
{"v":1,"cmd":"hangup","id":3,"call":1}
```

`BSDK-EVT-1` — entropy demand, in canonical emission form:

```text
{"v":1,"evt":"need-entropy","min":64}
```

`BSDK-EVT-2` — registration reached `registered`:

```text
{"v":1,"evt":"registration","state":"registered","expires":600}
```

`BSDK-EVT-3` — local media demanded for an outbound call:

```text
{"v":1,"evt":"need-local-media","call":1,"kind":"offer","constraints":{"audio":true,"video":false}}
```

### 9.3 Output-record framing vectors

`BSDK-OUT-1` — the §4.6 record carrying `BSDK-EVT-1`: type `4`, payload length `37`, then the
payload. First eight octets:

```text
04 00 00 00 25 00 00 00
```

`BSDK-OUT-2` — a `TIMER_SET` record, timer id `1`, `fire_at_ms` `500`, complete 24 octets:

```text
02 00 00 00 10 00 00 00 01 00 00 00 00 00 00 00 f4 01 00 00 00 00 00 00
```

### 9.4 Entropy derivation vector

`BSDK-ENT-1` — feed the 32-octet tape `00 01 02 … 1f` into a fresh kernel, then submit
`BSDK-CMD-1`. The REGISTER the kernel serialises MUST use exactly:

| Identifier | Value |
|---|---|
| Call-ID | `000102030405060708090a0b0c0d0e0f` |
| From tag | `1011121314151617` |
| Via branch | `z9hG4bK18191a1b1c1d1e1f` |

and the pool then holds 0 octets, so the outputs include a `"need-entropy"` event. Submitting a
command that needs another identifier before more entropy arrives fails `E_ENTROPY` with nothing
consumed.

### 9.5 ABI negative vectors

Each row is one call against an otherwise healthy kernel; the required result includes "and kernel
state is unchanged" in every case except the two marked fatal.

| ID | Call | Required result |
|---|---|---|
| `BSDK-NEG-1` | any entry with handle `0` or an unallocated handle | `E_INVALID_HANDLE` |
| `BSDK-NEG-2` | `sipx_command` with a pointer/length leaving linear memory | `E_BAD_POINTER` |
| `BSDK-NEG-3` | `sipx_command` whose buffer holds invalid UTF-8 | `E_UTF8` |
| `BSDK-NEG-4` | `sipx_command` with `{"v":1,"cmd":` | `E_JSON` |
| `BSDK-NEG-5` | `sipx_command` with `{"v":1,"cmd":"transfer","id":9}` | `E_SCHEMA` — the verb is not in §5.2 |
| `BSDK-NEG-6` | `"answer"` naming a call in `Dialing` | `E_STATE` |
| `BSDK-NEG-7` | a 32769-octet command document | `E_BOUNDS` before JSON parsing |
| `BSDK-NEG-8` | any entry after `sipx_kernel_free` of that handle | `E_INVALID_HANDLE` — handles are never reused |
| `BSDK-NEG-9` | ninth concurrent `"dial"` | outcome failure `call-limit`; the eight live calls are untouched |
| `BSDK-NEG-10` | `sipx_input_entropy` overflowing the 1024-octet pool | `E_BOUNDS`, pool unchanged |
| `BSDK-NEG-11` | `sipx_command` with `now_ms` lower than the previous call's | `E_TIME` |
| `BSDK-NEG-12` | `sipx_input_bytes` carrying 64 KiB + 1 | `E_BOUNDS`; nothing parsed |
| `BSDK-NEG-13` | `sipx_input_bytes` carrying garbage bytes | returns `0`; `parse_errors` increments; no event invents a call |

### 9.6 State and lifecycle vectors

| ID | Scripted events | Required result |
|---|---|---|
| `BSDK-STATE-1` | new kernel → entropy → `"register"` → 401 challenge → 200 | exactly two WIRE REGISTERs (second with digest, cnonce from the tape), a refresh `TIMER_SET`, events `"registration"` (`registered`) then `"outcome"` ok, in that order |
| `BSDK-STATE-2` | `"dial"` → `"local-media"` offer → 180 → 200 answer → `"media-applied"` | `"need-local-media"` before any WIRE; INVITE only after the offer validates; ACK only after `"media-applied"`; kernel state `sipEstablished` |
| `BSDK-STATE-3` | profile-valid INVITE in → `"ring"` → `"answer"` → `"local-media"` answer → ACK in | events `"call"` (incoming) then `"remote-media"`; 180, then 200 only after the answer validates; `sipEstablished` on ACK |
| `BSDK-STATE-4` | `"dial"` → abort before `"local-media"` | no WIRE ever emitted; `"call-ended"` cause `local`; JS layer stopped any acquired tracks before the dial promise settled |
| `BSDK-STATE-5` | `"dial"` → INVITE sent → `"hangup"` | CANCEL emitted; on 487 exchange, `"call-ended"` cause `local`; every timer the call set is cancelled by `TIMER_CANCEL` |
| `BSDK-STATE-6` | mid-call `sipx_kernel_free` | returns `0`; every subsequent entry on the handle is `E_INVALID_HANDLE`; no output records survive the free |
| `BSDK-STATE-7` | 200 answer carrying `a=crypto` (weaker media) | kernel refuses per profile: ACK then BYE, `"call-ended"` cause `media`; the SDES key bytes never appear in any event |
| `BSDK-STATE-8` | INVITE in whose offer has a video section | automatic 488; `refused_incoming` increments; no call object, no `"remote-media"` event |
| `BSDK-JS-1` | `await register()` with listeners on `"registration"` | listener observes `registered` before the promise resolves (§6.4 rule 3) |
| `BSDK-JS-2` | `close()` racing a delivered-but-undispatched event | the event's listener never runs; `dropped_after_close` increments; `closed` is the final callback |

## 10. Explicit omissions

This contract does not include, and completing the epic MUST NOT imply:

- video, data channels, SCTP, or any Rust/WASM WebRTC engine (§3.3);
- trickled candidate delivery — descriptions are complete-gathering, and a peer's `trickle` token
  is tolerated only under [webrtc-audio.md](webrtc-audio.md) §10's terms;
- a TURN or relay support claim; relay behaviour through application-supplied ICE server
  configuration is browser-owned and outside sipx's published claim until `M-24` widens it;
- hold/resume re-INVITEs, transfer (REFER), DTMF sending, MESSAGE, SUBSCRIBE/NOTIFY, PUBLISH —
  the v1 vocabulary is exactly §5.2;
- multiple simultaneous registrations, multiple AORs per kernel, or shared kernels across tabs,
  workers or a SharedWorker;
- Node, Deno or edge-runtime targets; the package is browser-targeted (§7.1);
- background operation past page death — the endpoint dies with its page (§6.5), and no service
  worker keeps it "registered";
- a stable 1.0 API (§7.4).

A future story widens any of these by changing this spec first; an implementation does not widen
them by accepting an input accidentally.

## 11. Implementation and evidence map

| Contract | Owner | Evidence story |
|---|---|---|
| §4 ABI, §5 vocabulary, §9 vectors native-and-WASM identical | `sipx-wasm` kernel crate | `S-41` |
| §4.7 entropy feed, §4.5 timers, WSS socket ownership, §8.6 enforcement | generated glue + `T-33` binding | `T-33` |
| §5.4 media flow over `RTCPeerConnection`, §6.2 established gate, §8.2/§8.5 at the media edge | browser media adapter | `M-52` |
| §6 lifecycle, §6.6 taxonomy, §7.1/§7.2 packaging, drift test | `@sipx/browser` package | `A-17` |
| §8.3/§8.7 deployment posture, demo CSP, lifecycle guide | demo site | `A-18` |
| §7.3 matrix, §8.8 residual-resource assertions, fail-closed negatives | packaged-artifact CI proof | `X-100` |

Later tests cite the stable vector IDs in §9. A child story may add cases, but it MUST NOT replace
a vector, weaken a negative, or restate this contract's boundaries in its own prose.
