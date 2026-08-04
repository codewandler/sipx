# Diagnostic phone specification

**Status:** normative target · **Epic:** `phone` · **Stories:** `P-8` … `P-13`

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
CodecPreference     = non-empty ordered list drawn from pcmu | pcma | opus
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
| ordered `pcmu`, `pcma`, `opus` | `Codecs::ordered` over the corresponding `CodecPreference` values |
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

WAV endpoints carry mono signed 16-bit linear samples at the clock rate of the codec selected by
the running media session. The command MUST read and validate the file structure before signalling,
but it cannot decide rate compatibility from a preference list: that decision is made immediately
after negotiation and before the first sample is queued. A mismatched input is a typed command
failure naming both rates and asking the operator to resample it; the command does not silently
reinterpret or resample the samples.

RFC 3551 assigns PCMU and PCMA an 8 kHz RTP clock. RFC 7587 assigns Opus a 48 kHz RTP clock even
when the encoded signal bandwidth is narrower. At the session's 20 ms packet interval this means
160 decoded samples per G.711 packet and 960 decoded samples per Opus packet. Both `dial` and
`answer` MUST take the packet size from the negotiated session rather than retaining either literal.
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

An invocation that does not use `--transport` retains its existing output byte for byte. An
explicit selection adds `requested_transport` and `negotiated_transport` to terminal results; the
pre-call `answer` announcement carries only the requested transport because nothing has negotiated
yet.

`dial` and `answer` accept repeatable ordered `--codec` values, `--media-security`, `--ice`,
`--stun-server`, `--audio-input` and `--audio-output`. An audio endpoint is written
`wav:<path>`, `device:<id>`, `generator:<kind>` or `null`; the first colon separates the kind and
the remainder is its value. `--play <path>` is exactly `--audio-input wav:<path>` and
`--record <path>` is exactly `--audio-output wav:<path>`. Naming both spellings for one direction
is a setup error rather than an ordering rule. Generator kinds are closed by the story that ships
them; an unknown or not-yet-shipped kind is refused.

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
wrapping. Thus G.711 uses 8 kHz/160-sample media frames while Opus uses its negotiated 48 kHz clock;
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

`--header 'Name: value'` MAY be repeated. Values pass the same injection checks as the message
builders. These stack-owned fields **MUST** be refused: `Via`, `Route`, `Record-Route`,
`Max-Forwards`, `Call-ID`, `CSeq`, `From`, `To`, `Contact` and `Content-Length`. The command reports
the refused name before binding or dialing.

## 4. Interactive protocol

`sipx scenario` reads one JSON object per line and writes the existing versioned JSON event envelope
one object per line. Every command carries a caller-supplied `id`; completion or refusal echoes it.
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
  "seed":0,
  "target":"sip:load@192.0.2.1:5060",
  "limits":{"rate":10.0,"concurrency":32,"calls":100,"duration_ms":null,"call_duration_ms":0,"setup_timeout_ms":20000,"cleanup_ms":40000},
  "outcomes":{"attempted":100,"connected":98,"rejected":1,"timed_out":1,"failed":0,"peak_concurrency":12},
  "response_codes":{"200":98,"486":1},
  "setup_ms":{"p50":18,"p95":31,"p99":45},
  "media":{"snapshots":98,"packets_lost":0,"mean_loss":0.0000,"mean_jitter_ms":1,"mean_mos":4.38}
}
```

An unavailable percentile or media aggregate is `null`, not zero. Per-call events are optional,
but this summary is emitted only after cleanup and is always exactly one machine-readable record.

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
