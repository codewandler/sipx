# Diagnostic phone specification

**Status:** normative target · **Epic:** `phone` · **Stories:** `P-8` … `P-16`

## 1. Scope

This specification defines the public behavior of the `sipx` diagnostic endpoint. It extends the
existing `dial`, `answer` and `register` contracts without changing their defaults. It covers
selectable signalling, media policy, device audio, interactive control, custom headers and bounded
load generation.

It does not define a graphical interface, transcription, text-to-speech, a dial plan, a registrar,
or a proxy. ICE restart and relayed endpoint candidates remain `M-23` and `M-24`.

Normative words **MUST**, **MUST NOT**, **SHOULD** and **MAY** are used as in RFC 2119 and RFC 8174.

## 2. Configuration values

The command layer maps arguments into these closed values before opening a socket:

```text
SignallingTransport = udp | tcp | tls | ws | wss
MediaSecurity       = auto | plain | sdes | dtls-srtp
IcePolicy           = disabled | host | stun(server)
AudioEndpoint       = wav(path) | device(id) | generator(kind) | null
CodecPreference     = non-empty ordered list drawn from pcmu | pcma | l16 | opus
```

`auto` preserves the existing behavior: plain RTP on unprotected signalling and SDES-SRTP on a
protected signalling path. Explicit `sdes` **MUST** be refused on unprotected signalling. Explicit
`dtls-srtp` **MUST** either negotiate DTLS-SRTP or fail; it never falls back to SDES or plain RTP.
`disabled` preserves symmetric RTP with no ICE attributes. `host` gathers host candidates; `stun`
adds server-reflexive candidates and degrades to host candidates only when the user selected that
policy rather than requiring a server-reflexive result.

The default codec preference remains the existing G.711 set. `opus` is accepted only when the
binary was built with the corresponding feature. Unsupported requested values fail before network
I/O.

The command layer does not construct capabilities or inspect SDP. Its complete mapping into the
public call policy is:

| Command value | Call policy |
|---|---|
| ordered `pcmu`, `pcma`, `l16`, `opus` | `Codecs::ordered` over the corresponding `CodecPreference` values |
| no `--codec` | `Codecs::default()` (`pcmu,pcma`) |
| `auto` | `Keying::Auto` |
| `plain` | `Keying::Plain` |
| `sdes` | `Keying::Sdes` after verifying the selected signalling transport is protected |
| `dtls-srtp` | `Keying::DtlsSrtp` |
| `disabled`, `host`, `stun` | `IcePolicy::Disabled`, `Host`, `Stun(server)` |

The call policy remains the sole owner of capability construction and negotiation. In particular,
the command MUST NOT rewrite an offer, infer a codec from a payload number, or turn an unsupported
combination into a weaker policy.

### 2.1 WAV and media-clock contract

WAV endpoints carry mono signed 16-bit linear samples with the rate named in their header. The
command MUST read and validate the file structure before signalling. After negotiation it passes
that explicit format through the `M-43` PCM boundary, which linearly resamples to the session clock
before the first sample is queued. A malformed or unsupported rate is a typed command failure; a
different supported rate is converted rather than reinterpreted or distorted.

RFC 3551 assigns PCMU and PCMA an 8 kHz RTP clock, and assigns mono L16 at 44.1 kHz to static
payload 11; selected 8 kHz L16 uses a dynamic mapping. RFC 7587 assigns Opus a 48 kHz RTP clock even
when the encoded signal bandwidth is narrower. At the session's 20 ms packet interval this means
160 decoded samples for G.711 or 8 kHz L16, 882 for static L16, and 960 for Opus. Both `dial` and
`answer` MUST take the packet size from the negotiated session rather than retaining any literal.
A WAV recording MUST write the negotiated clock rate in its header. The number of recorded samples
therefore has the same time meaning for both call roles: `samples / negotiated_clock_rate` seconds.

The command-level Opus vector uses two independently distinguishable one-second, 48 kHz signals,
one in each direction between two command processes. Each recording MUST have a 48 kHz header,
contain 920–1000 ms of the one-second source (allowing at most four packets sent before recorder
readiness), and have a dominant frequency matching its far end rather than its local input. This
simultaneously detects the old 8 kHz header, 160-sample answer frames, one-direction proof, and a
recording that merely contains some non-zero samples. The corresponding G.711 vector continues to
use 8 kHz input, 160-sample packets and an 8 kHz recording header.

## 3. Command surface

`dial`, `answer` and `register` accept `--transport <udp|tcp|tls|ws|wss>`. The existing `--tcp`
flag remains an alias. On outbound TLS/WSS, `--tls-server-name` overrides the URI host used for
verification, `--tls-ca` adds PEM roots to the platform store, and the paired `--tls-cert` /
`--tls-key` flags provide an optional client identity. On `answer`, that certificate/key pair is
the required server identity for TLS/WSS; outbound-only name and trust options are refused. A
secure URI cannot be combined with a cleartext transport. Certificate verification is on by
default; disabling it is not part of this contract.

Transport selection itself adds no fields when `--transport` is omitted. An explicit selection
adds `requested_transport` and `negotiated_transport` to terminal results; the pre-call `answer`
announcement carries only the requested transport because nothing has negotiated yet.

Outbound setup preserves the terminal cause from the transport transaction. A concrete stream
connection, handshake, certificate-verification or established-connection failure maps to
`failed`/exit 1 and retains the transport cause. A datagram request that was handed to a usable
socket but received no final SIP response maps to `timeout`/exit 5. The command MUST NOT infer this
classification from elapsed time or error text, and it MUST NOT rewrite a concrete transport
failure as `NoResponse` merely because no SIP response object exists.

`dial` and `answer` accept repeatable ordered `--codec` values, `--media-security`, `--ice`,
`--stun-server`, `--audio-input` and `--audio-output`. An audio endpoint is written
`wav:<path>`, `device:<id>`, `generator:<kind>` or `null`; the first colon separates the kind and
the remainder is its value. `--play <path>` is exactly `--audio-input wav:<path>` and
`--record <path>` is exactly `--audio-output wav:<path>`. Naming both spellings for one direction
is a setup error rather than an ordering rule. Generator kinds are closed by the story that ships
them; an unknown or not-yet-shipped kind is refused.

### 3.1 WAV output ownership

`dial` and `answer` normalize `--record <path>` and `--audio-output wav:<path>` into the same WAV
output selection and use one reservation/finalization implementation. After command syntax, local
audio selection and WAV input validation, but before destination resolution, transport bind, listener
readiness or SIP emission, the command MUST reserve its output:

1. The requested final path MUST name a file whose parent already exists. An existing file or
   symlink at the final path is a usage refusal and MUST NOT be truncated, replaced or followed.
2. The command creates a uniquely named sibling temporary file with exclusive creation and keeps
   that exact file handle open for the call lifetime. Name collisions are retried a finite number
   of times. Failure names the requested path, not only the internal temporary name.
3. Captured samples remain in memory until the call's bounded media work joins. Finalization writes
   the complete WAV through the reserved handle, flushes it durably, closes the handle, and installs
   the sibling at the requested path without replacing a file created after preflight. A same-
   filesystem atomic link/rename operation is used where the platform permits it.
4. Successful installation is never undone by a later reporting or cleanup failure. Before
   installation, every usage refusal, call failure, cancellation, remote hangup, media failure,
   write failure and ordinary drop closes the handle and removes the sibling temporary entry.

The temporary name is derived only from the operator-supplied destination and local randomness;
network identities and header values never become paths. A destination race after preflight is a
terminal command failure that names the final path and does not rewrite the call as never having
occurred. The final result names `recording` only after installation succeeds.

`dial --early-media` opts into the reliable-provisional call path. The command consumes provisional
responses until an SDP-bearing reliable response starts a media session or a final response
arrives. It acknowledges a reliable response with PRACK before reading its media. If early media
starts, a WAV recording includes those samples before samples received after the final answer; the
terminal result adds `early_media: true` and the measured `early_samples_recorded`. A final response
that arrives without an early session reports `early_media: false` and zero early samples. The flag
does not change an invocation that omits it, and DTLS-SRTP retains the call layer's typed refusal on
this path because its active handshake cannot safely precede the final response.

Device selectors use the complete stable backend identifier returned by `sipx devices --json`.
The identifier includes the backend and round-trips as an opaque string; a display name is never an
identifier. The listing is sorted by identifier and has this stable v1 shape:

```json
{"schema":"sipx.devices.v1","devices":[{"id":"alsa:hw:CARD=Loopback,DEV=0","name":"Loopback","input":true,"output":true}]}
```

Human output is one `id`, direction set and display name per line. Listing opens no stream. Opening
an explicitly named input or output looks up that exact identifier and never consults the platform
default. An absent, busy, permission-denied or unsupported requested device is a typed setup
failure before signalling transport bind; no case switches to another device.

### 3.1 Device stream contract

Device I/O exists only behind the `device-audio` feature of the command crate. A build without the
feature has no platform-audio dependency, retains the WAV and null endpoint behavior byte for byte,
and refuses `sipx devices` or a `device:` endpoint with a setup error naming the required feature.
No core, call or media crate depends on a device API.

The driver opens requested streams paused, before signalling I/O, and starts them only after a call
is established. It accepts linear `i16`, `f32` or unsigned 16-bit device samples and rejects every
other format. From the device's supported configurations it deterministically chooses a rate
closest to 8 kHz (8 kHz itself when the range contains it), then the fewest non-zero channels, then
sample format in the order `i16`, `f32`, unsigned 16-bit. More than 32 channels is refused. After
negotiation, input is downmixed by the arithmetic mean, linearly resampled to the media session's
clock rate and cut into the session's packet-sized frames. Output is linearly resampled from that
clock rate and copied to every device channel. Conversion clips at the `i16` range rather than
wrapping. Thus G.711 and dynamic L16 use 8 kHz/160-sample media frames, static L16 uses 44.1 kHz/882,
while Opus uses its negotiated 48 kHz clock;
the device does not constrain codec selection.

Device rates above 384 kHz and a single callback larger than 1,048,576 interleaved samples are
refused as unsupported rather than used to size an allocation. Enumeration is capped at 1,024
stable identifiers per invocation. These are resource bounds, not truncation rules: crossing one
fails visibly and never returns a partial device list or partial callback.

The callback boundary is bounded and non-blocking. Each direction holds at most 50 media frames
(one second at the 20 ms packet interval); a callback uses only `try_send`/`try_recv` and never
waits for the call. A full input queue drops the newest converted frame. A full output queue drops
the newest received media frame. An empty output queue produces silence. The terminal result names
the selected input/output identifiers and configurations and reports
`device_input_dropped_samples`, `device_output_dropped_samples` and
`device_output_silence_samples`; the counters are zero when no loss occurred. Thus conversion and
scheduler loss are observable rather than hidden in a successful call.

One stream error moves the driver to stopping and fails the command with its direction and typed
category. Shutdown is causal, not timed: request both relay tasks to stop, await both tasks, pause
and drop both streams, then emit the terminal result. A command MUST NOT emit its result while a
device relay task it started is still live.

| State | Input | Output |
|---|---|---|
| configured | exact identifiers parsed; no stream exists | same |
| opened | stream built and paused; bounded queue empty | same |
| running | callback produces media frames; relay plays them into the call | relay receives call frames; callback consumes them |
| stopping | callback producer dropped; relay observes stop | relay observes stop; callback receives silence until paused |
| joined | relay task awaited; stream dropped | relay task awaited; stream dropped |

### 3.2 Invitation deadline and cancellation

`dial --timeout <S>` is the invitation-answer budget. A positive value starts when the initial
INVITE is handed to the endpoint and ends before cancellation begins; zero delegates answer
expiry to the SIP client transaction. `--cancel-timeout <S>` is a separate cancellation-cleanup
allowance and defaults to two seconds. It starts only when the answer budget or an operator
interrupt wins. The documented process bound for a positive answer budget is therefore the sum of
these two values followed by the endpoint's causal task-join barrier; the error MUST NOT describe
the answer budget alone as total elapsed time.

Cancellation is one owned operation with this state table:

| State | Input | Required action | Next state |
|---|---|---|---|
| inviting | final response before answer deadline | retain that final result; do not cancel | terminal or confirmed |
| inviting | answer deadline | freeze the timeout result; begin cancellation allowance | cancelling |
| inviting | operator interrupt | freeze the interrupt result; begin cancellation allowance | cancelling |
| cancelling | provisional observed and allowance remains | create exactly one CANCEL for the INVITE transaction | joining |
| cancelling | final non-2xx INVITE response | retain the frozen local cause; no CANCEL or BYE | joining |
| cancelling | final 2xx INVITE response | ACK it, originate BYE, retain the frozen local cause | joining |
| cancelling | allowance expires | stop waiting; retain the frozen cause and mark cleanup exhausted | joining |
| joining | endpoint work reaches zero | emit the terminal record | reported |

The deadline branch wins an exact-boundary tie with a newly readable final response. A response
that was observed before the deadline wins normally; one observed after the deadline can only
complete cancellation cleanup and MUST NOT turn the timeout into success. A zero cancellation
allowance performs no timed wait: already-ready cancellation state may be consumed, then endpoint
shutdown joins every owned task. It does not mean an unbounded fallback.

Timeout text and JSON contain the same fields: `status=timeout`, `invitation_limit_ms`, measured
`invitation_elapsed_ms`, `cancel_limit_ms`, measured `cancel_elapsed_ms`, `cancel_sent`,
`cancel_final_observed`, `cancel_cleanup_exhausted` and an actionable `error`. Interrupted setup
uses the same cleanup facts with `status=interrupted`; a pre-deadline SIP rejection retains its SIP
status and does not invent cancellation fields. Durations use the monotonic clock. A fixed duration
may bound failed cleanup, but transaction events and the endpoint join barrier are the successful
happens-before relations.

### 3.3 Confirmed-call lifecycle

After `dial` or `answer` confirms a dialog, the command MUST continuously drive that dialog while
media work runs. The same input pump consumes ACK, BYE and every other in-dialog request admitted by
the call layer; a media future or local duration MUST NOT replace that pump. In particular, an ACK
is dequeued promptly enough to stop INVITE 2xx retransmission, and an accepted BYE receives its
final response before terminal output.

The command has these states and transitions:

| State | Input | Required action | Next state |
|---|---|---|---|
| confirming | Ctrl-C/SIGINT | cancel the owned INVITE; send CANCEL, or ACK then BYE if confirmation crossed cancellation | joining |
| confirmed | valid remote BYE | stop originating requests, answer the BYE, stop media | joining |
| confirmed | Ctrl-C/SIGINT | stop media work, originate at most one BYE, finitely await its final response | joining |
| confirmed | local duration or completed media work | originate at most one BYE and finitely await its final response | joining |
| confirmed | terminal transport/session failure | stop media and retain the typed failure | joining |
| joining | crossed remote BYE | answer it; do not originate another BYE | joining |
| joining | owned work reaches zero | close the endpoint and finalize counters | reported |

When multiple confirmed inputs are ready in one poll, a valid remote BYE wins over Ctrl-C, which
wins over local completion. After local teardown has started, a crossed valid BYE is still answered
but cannot change the already selected terminal cause or cause a second originated BYE. A pending
outbound invitation never manufactures a BYE before a dialog exists: cancellation retains the call
layer's CANCEL/late-2xx cleanup behavior.

The terminal result adds `ended_by`, with value `remote`, `duration` or `interrupt`. When this side
originates BYE and observes a valid final response, `bye_status` carries its status code; a remote
end originates no BYE and omits that field. Remote BYE and local-duration completion retain
`status=answered` and exit 0. A handled Ctrl-C emits `status=interrupted`, `ended_by=interrupt` and
exits 0 after cleanup. A typed transport, session, media or cleanup failure emits `status=failed`
and exits 1. An unanswered local BYE is bounded and does not resurrect an ended call; a concrete
failure to hand the BYE to the transport is a cleanup failure. SIGTERM and other supervisor
signals are specified separately by story `P-22`.

Terminal output is a join barrier, not a request to begin cleanup. Before the record is emitted,
the command MUST stop or finish media operations, join device relays and media workers, release the
call, shut down the endpoint driver, and finalize its counter export. No dialog, transport, media
or device task owned by the invocation may still be running after terminal output.

`--header 'Name: value'` MAY be repeated. Values pass the same injection checks as the message
builders. These stack-owned fields **MUST** be refused: `Via`, `Route`, `Record-Route`,
`Max-Forwards`, `Call-ID`, `CSeq`, `From`, `To`, `Contact` and `Content-Length`. The command reports
the refused name before binding or dialing.

## 4. Interactive protocol

`sipx scenario` reads one JSON object per line and writes the existing versioned JSON event envelope
one object per line. Every command carries a caller-supplied `id`; completion or refusal echoes it.
The normative frame grammar, command fields, correlation, recovery and process-exit rules are in
[`scenario-automation.md`](scenario-automation.md).
The v1 command set is:

```text
dial, accept, reject, play, stop_playback, start_recording, stop_recording,
send_dtmf, hold, resume, transfer, hangup, wait_for, shutdown
```

`wait_for` names an event predicate and a finite timeout. A bare sleep is not a command. EOF requests
an orderly shutdown: active calls are terminated, recordings are finalized, and then the process
exits. Invalid JSON or an unknown command produces a correlated error without corrupting the event
stream.

## 5. Bounded load

`sipx load <URI>` requires `--rate <CALLS/S>`, `--concurrency <N>`, and at least one finite
termination bound: `--calls <N>` or `--duration <S>`. Rate and concurrency MUST be positive and
finite; calls and duration, when present, MUST be positive. `--seed <U64>` defaults to zero and
controls call timing and deterministic media generation. `--call-duration <S>` defaults to zero;
it bounds how long an answered call remains established. `--timeout <S>` retains the diagnostic
phone's outbound setup bound and defaults to 20 seconds. All values are validated before a socket
is opened. When both termination bounds are supplied, the first reached stops admission.

Every started call is owned by the run. Reaching an admission bound, receiving an interrupt, or
observing an internal runner error closes admission exactly once. Active calls receive the same
stop signal, send `CANCEL` or `BYE` as appropriate, and are joined before the command returns. The
cleanup budget is 40 seconds: longer than the 32-second SIP transaction ceiling, and finite so a
broken worker cannot retain the process indefinitely. Exhausting it is an internal failure, never
a successful partial cleanup.

With `--json`, the final line is one object with this stable v1 shape (map keys that represent SIP
status codes are decimal strings):

```json
{
  "schema":"sipx.load.v1",
  "status":"completed|interrupted|failed",
  "reason":null,
  "mode":"signalling",
  "seed":0,
  "target":"sip:load@192.0.2.1:5060",
  "limits":{"rate":10.0,"concurrency":32,"calls":100,"duration_ms":null,"call_duration_ms":0,"setup_timeout_ms":20000,"cleanup_ms":40000},
  "outcomes":{"attempted":100,"connected":98,"rejected":1,"timed_out":1,"failed":0,"peak_concurrency":12},
  "response_codes":{"200":98,"486":1},
  "setup_ms":{"p50":18,"p95":31,"p99":45},
  "media":{"snapshots":0,"packets_lost":0,"mean_loss":null,"mean_jitter_ms":null,"mean_mos":null}
}
```

An unavailable percentile or media aggregate is `null`, not zero. Per-call events are optional,
but this summary is emitted only after cleanup and is always exactly one machine-readable record.

### 5.1 Shared workload modes and terminal causes

`load` and `load-responder` share the same `--mode` vocabulary. `signalling` is the default on both
commands. It sends a bodyless INVITE, completes the bodyless 2xx/ACK dialog, holds it for the
configured call/dialog duration, and completes it with BYE. It creates no SDP, media port, RTP task
or media snapshot. `generated-media` is selected explicitly on both commands and retains the
deterministic PCMU offer/answer, one bounded generated frame and RTP quality snapshot used by the
media workload. The `mode` field appears in both terminal summaries; signalling summaries report
zero media snapshots and unavailable media aggregates as `null`.

Every INVITE emitted by `load` carries the private extension field
`X-Sipx-Workload-Mode: signalling|generated-media`. The paired responder checks a present field
before admitting the dialog. A value different from its configured mode receives 488 `Workload
Mode Mismatch`, is counted as a pre-admission rejection, closes the paired run as `failed`, and
exits 1 after cleanup. The marker is advisory for other clients: an absent field retains the
body-based interoperability behavior. An unknown local `--mode` is command usage, exits 2, and
opens no socket.

`completed` means a configured call or duration admission bound was reached and every admitted
worker joined. `interrupted` is reserved for an operator or supported process stop request whose
cleanup completed. A worker error, workload-mode mismatch, poisoned measurement store, media
failure or exhausted cleanup budget closes admission, cancels and joins owned calls, emits
`status=failed` with an actionable `reason`, and exits 1. Such a failure MUST NOT be reported as an
operator interruption merely because the internal error used the common stop token. Ordinary SIP
policy rejections remain measured call outcomes and do not alone make a bounded run fail.

## 6. Secrets and output

Passwords, private keys, digest responses and SRTP key material **MUST NOT** appear in human output,
JSON, Debug output, traces or captures. Environment/file sources remain the documented route for
secrets; command-line secret flags carry the existing visibility warning.

When any media selector is explicit, the terminal result adds `requested_codecs` (a comma-separated
ordered list), `requested_media_security`, `requested_ice`, `negotiated_codec`,
`negotiated_media_security` and `negotiated_ice`. The negotiated values are read from the running
call. `negotiated_ice` is `disabled`, `checking`, `host`, `server-reflexive`, `peer-reflexive` or
`relayed`; it is never copied from `requested_ice`. A pre-call `answer` announcement carries only
the three requested fields because no call exists yet.

An implemented lower-layer capability receives no product credit unless this result can show that
a real call selected it.

### 6.1 Public-reference drift contract

The public CLI reference MUST be checked against the executable, not against a second copy of its
Rust help constants. The check builds the default `sipx-cli` package once, executes `sipx --help`
and every working subcommand's `--help`, and compares the command and long-option sets with the
corresponding public reference sections. Global `--json` and `--help` are documented once and are
excluded from each command's repeated option set. A command or option present on only one side is a
failure.

The same check inventories every versioned JSON schema or envelope produced by the CLI. The
inventory is discovered from the Rust producers, including the `sipx.app.v1` contract imported by
`scenario`, and is compared with the public page's checked contract table. Every literal structural
field emitted by a producer MUST appear in that table; an unknown version, missing field or prose-only
schema is a failure. Event-specific `scenario` detail fields remain additions inside the documented
`event` object rather than distinct envelopes.

The checker MUST have fixture tests that reverse each comparison: an undocumented executable flag,
a documented flag absent from help, a missing JSON field and a newly discovered versioned contract.
The checker and its fixture tests are separate gate steps. The executable comparison runs after the
workspace build in the local gate and CI, so it observes the binary that will ship.

## 7. Vectors

| ID | Scenario | Required result |
|---|---|---|
| `DPH-1` | Explicit TLS with a trusted peer | Connected; negotiated transport is TLS |
| `DPH-2` | Explicit WSS with a certificate-name mismatch | Typed TLS failure; no downgrade |
| `DPH-3` | Opus selected in a build without Opus | Setup failure before network I/O |
| `DPH-4` | Explicit SDES over UDP | Refused as an unsafe combination |
| `DPH-5` | Explicit DTLS-SRTP | Fingerprint negotiated and media flows, or a typed DTLS failure |
| `DPH-6` | STUN ICE where host candidates cannot connect | A nominated server-reflexive pair carries audio |
| `DPH-7` | `device:alsa:missing` selected while a bound UDP observer watches the target | Exit `failed` names `audio input`, `alsa:missing` and `not available`; observer receives no datagram |
| `DPH-8` | Custom `Supported` plus an attempted custom `Via` | `Supported` is sent; `Via` is refused before bind |
| `DPH-9` | Scenario waits for answer, sends DTMF, then hangs up | Correlated events occur in causal order |
| `DPH-10` | Load run reaches its call bound | No new call starts; every owned call is cleaned up |
| `DPH-11` | Load run is interrupted | Admission stops and cleanup finishes before the summary |
| `DPH-12` | WAV input and a Linux virtual microphone containing the same deterministic clip call the same recorder | Both 8 kHz recordings pass the same quantised-sample assertion; the device result names its exact configuration and reports all three loss counters |
| `DPH-13` | `dial` or `answer`, using either WAV-output spelling, names a missing parent, an existing final file or a destination whose sibling cannot be created | usage refusal names the requested path before resolver, bind, readiness or peer traffic; an existing final file retains its bytes |
| `DPH-14` | a reserved destination becomes occupied before finalization | the call outcome remains observable, finalization fails without replacing the competing file, and no reservation temporary remains |
| `DPH-15` | invitation limits 1, 2, 3, 5 and 8 seconds expire against a ringing peer; cancellation allowance is 2 seconds | paused time reaches each invitation limit and at most its explicit cancellation allowance; one CANCEL is sent and the timeout report separates both measured phases |
| `DPH-16` | final response is ready immediately before, exactly at or immediately after the invitation deadline | before wins as a final result; exact and after retain timeout while cleanup handles the crossed response at most once |
