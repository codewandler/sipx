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

## 3. Command surface

`dial`, `answer` and `register` accept `--transport`. TLS and WSS additionally accept a server name,
trust roots and optional client identity. A secure URI cannot be combined with a cleartext
transport. Certificate verification is on by default; disabling it is not part of this contract.

`dial` and `answer` accept ordered `--codec` values, `--media-security`, `--ice`, `--stun-server`,
`--audio-input` and `--audio-output`. File flags remain aliases for WAV endpoints. Device selectors
use stable backend identifiers returned by `sipx devices --json`; an absent or busy requested
device is a typed setup failure, never an implicit switch to the default device.

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

`sipx load` requires a target, a positive rate, a positive concurrency limit, and at least one finite
termination bound: total calls or duration. When both are supplied, the first reached stops new
work. A seed controls call timing and deterministic media generation.

Every started call is owned by the run. On timeout, interrupt or internal error the runner stops
admission, terminates owned calls within a documented cleanup budget, waits for cleanup, and only
then reports. Results include attempted, connected, rejected, timed out and failed calls; peak
concurrency; response-code counts; setup-duration distribution; media loss/quality snapshots; and
the seed and effective limits. Per-call events are optional, but the final summary is always one
machine-readable record.

## 6. Secrets and output

Passwords, private keys, digest responses and SRTP key material **MUST NOT** appear in human output,
JSON, Debug output, traces or captures. Environment/file sources remain the documented route for
secrets; command-line secret flags carry the existing visibility warning.

The terminal result records requested and negotiated transport, codec, media security and ICE path.
An implemented lower-layer capability receives no product credit unless this result can show that a
real call selected it.

## 7. Vectors

| ID | Scenario | Required result |
|---|---|---|
| `DPH-1` | Explicit TLS with a trusted peer | Connected; negotiated transport is TLS |
| `DPH-2` | Explicit WSS with a certificate-name mismatch | Typed TLS failure; no downgrade |
| `DPH-3` | Opus selected in a build without Opus | Setup failure before network I/O |
| `DPH-4` | Explicit SDES over UDP | Refused as an unsafe combination |
| `DPH-5` | Explicit DTLS-SRTP | Fingerprint negotiated and media flows, or a typed DTLS failure |
| `DPH-6` | STUN ICE where host candidates cannot connect | A nominated server-reflexive pair carries audio |
| `DPH-7` | Missing requested device | Typed device failure; no fallback |
| `DPH-8` | Custom `Supported` plus an attempted custom `Via` | `Supported` is sent; `Via` is refused before bind |
| `DPH-9` | Scenario waits for answer, sends DTMF, then hangs up | Correlated events occur in causal order |
| `DPH-10` | Load run reaches its call bound | No new call starts; every owned call is cleaned up |
| `DPH-11` | Load run is interrupted | Admission stops and cleanup finishes before the summary |
| `DPH-12` | WAV and virtual-device runs carry the same deterministic clip | Both recordings pass the same sample assertion |
