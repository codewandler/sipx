---
title: CLI reference
description: Every sipx command, flag, exit code and JSON field — the surface a shell script can rely on.
---

# CLI reference

One binary, `sipx`. Eight commands do work — `dial`, `answer`, `load`, `load-responder`, `register`, `peers`, `devices`
and `scenario`, documented below — alongside `help` and `version`. Global: `--json` switches the report to a single-line JSON
object on stdout; `-v`/`-vv` raise log verbosity on stderr (never stdout, so JSON stays
parseable); `-h`/`--help` on any command.

### INFO progress

One `-v` emits bounded lifecycle progress on stderr in both text and JSON result modes. No `-v`
keeps these records quiet; `-vv` adds DEBUG detail, and further repetitions remain at DEBUG. The
stable INFO events are:

| Event | When | Fields |
|---|---|---|
| `call.waiting` | answer begins its bounded wait | `role`, `address`, `wait_ms` |
| `call.placed` | dial is about to invite the selected peer | `role`, `peer`, `transport` |
| `call.caller_observed` | answer admits the caller | `role`, `caller` |
| `call.answered` | the confirmed call enters media exchange | `role`, `peer`, `setup_ms` |
| `call.ended` | call resources and export have joined | `role`, `peer`, `status`, `cause`, `elapsed_ms` |
| `load.admission_started` | bounded admission begins | `target`, `mode`, `rate`, `concurrency`, `calls`, `duration_ms` |
| `load.summary` | admission and all owned calls have joined | `status`, `attempted`, `connected`, `rejected`, `timed_out`, `failed`, `peak_concurrency` |

Dial orders placed, answered and ended. Answer orders waiting, caller observed, answered and ended;
a refusal or failure omits answered. Terminal causes are `remote`, `duration`, `interrupted`,
`refused`, `timeout` or `failed`, and the first cause produces exactly one end record. Load INFO is
always two aggregate records regardless of its call/rate bounds—there is no per-attempt INFO stream.

Every flag below whose name is followed by a placeholder — `--timeout <S>`, `--book <FILE>` — needs
a value, and a flag given none is a usage error (exit 2) naming the flag. It is never read as
absent: falling back to the default would run the command on something you did not ask for, and
say nothing. Both ways a value goes missing are refused:

- **Nothing after the flag.** `sipx register sip:alice@example.com --outbound --instance` is
  refused, rather than registering a device identity that was generated instead of given.
- **An empty value**, in either form — `--instance=` or `--instance ""`. No valued flag has a
  meaningful empty value, and omitting a flag is already how you ask for its default, so an empty
  one can only be a mistake. It is an easy one to make: an unset shell variable expands to exactly
  this, which is how `--target "$ADDR"` arrives with nothing in it.

Every `<S>` value is a whole number of seconds from `0` through `4294967295`. Negative values,
fractions, units such as `3s`, and values above that range are usage errors naming the flag. Zero is
deliberate where the command gives it a meaning: `--duration 0` ends an established call immediately
(and is refused as an admission bound by `load` and `load-responder`), `--timeout 0` uses the
transaction layer's expiry, `dial --cancel-timeout 0` performs no timed cancellation wait,
`--wait 0` returns immediately when no call is queued, and `--expires 0` asks the registrar to
remove the binding. `load-responder` refuses zero for `--cleanup` and
`--dialog-duration`, because neither a cleanup budget nor an accepted-dialog lifetime can be empty.

`--help` is answered before any of this, so it still prints when the rest of the line is wrong.

The long-running `dial`, `answer`, `load`, and `load-responder` commands handle Ctrl-C/SIGINT and,
on Unix, supervisor SIGTERM through the same graceful stop path. The first signal is reported as
`stop_signal: "interrupt"` or `"terminate"`; repeated supported signals do not shorten the
command's documented cleanup bound or produce another terminal record. A clean signal stop exits
0 after owned work joins. Handler or cleanup failure reports `failed` and exits 1.

`sipx version` prints exactly `sipx <version>` in text mode. `sipx version --json` emits one object
with `status: "version"` and the complete `version`; neither form accepts a positional argument.

Build-capability checks happen before destination resolution, local file/device setup, transport
binding or peer traffic. Opus, DTLS-SRTP, the browser-audio profile and explicit device endpoints
therefore fail as usage (exit 2) and name the missing `opus`, `dtls` or `device-audio` feature even
when the destination is unreachable. ICE is a baseline capability; `stun` requires a server, while
the other ICE modes refuse one. Full transport policy is checked next, still before signalling.

`dial`, `answer`, and `register` select `udp`, `tcp`, `tls`, `ws`, or `wss` with
`--transport <T>`. `dial` and `register` default to UDP; `answer` without a transport flag keeps its
historical UDP and TCP listeners. `--tcp` remains a compatible alias. TLS/WSS verify certificates
with the platform trust store plus `--tls-ca <FILE>`, and use the URI host unless
`--tls-server-name <NAME>` explicitly supplies the service identity. There is no flag that disables
verification and a `sips:` URI cannot select a cleartext transport.

`dial` and `register` may present a mutual-TLS identity with `--tls-cert <FILE>` and
`--tls-key <FILE>`. `answer` uses the same pair as its required server identity when listening on
TLS or WSS. Supplying only half the pair is a usage error before any socket is opened.

### Named destinations

Every outbound command — `dial`, `register`, `load`, `peers --registrar` and `scenario` — takes the
host you actually have. A name is looked up by the phone itself, following RFC 3263, so there is
nothing to resolve beforehand and no address to paste into `--target`:

```sh
sipx dial sip:bob@pbx.example
sipx register sip:alice@example.com --target pbx.example:5060
```

What is asked for depends on how much the URI already fixes. A host with an explicit port needs no
service discovery, so only its address records (`A` and `AAAA`) are looked up. A bare name goes
through `NAPTR`, then the `SRV` names those point at plus `_sip._udp`, `_sip._tcp` and `_sips._tcp`
— only `_sips._tcp` for a `sips:` URI — then addresses. An explicit `--transport` or URI
`transport=` parameter skips `NAPTR` and asks for that one service's `SRV` name, which is also how
`ws` and `wss` destinations are discovered. A literal IPv4 or IPv6 address performs no lookup at
all, and is unchanged in every way, including its timing.

The lookup is bounded twice: **two seconds** for any one question, and **eight seconds** for the
whole resolution including the ordering that follows it. Both are ceilings rather than waits.

Every command that states a deadline is the ceiling over its own lookup rather than something the
lookup is added to. `dial --timeout`, `load --timeout` and `scenario --timeout` — or a `dial`
frame's `timeout_ms` — lower both bounds under themselves; `register --timeout` lowers them under
whatever the attempt has left; and `peers`, which states no attempt deadline, lowers them under the
subscription lifetime `--expires` asks for. A command given two seconds spends at most two on the
lookup rather than eight before its own clock starts. A generous deadline changes nothing: these
are minimums against the two figures above, never extensions of them, and `--timeout 0` leaves the
resolver's own bounds in place along with transaction expiry.

Resolution keeps the identity you asked for. TLS and WSS connect to the selected address and verify
the name from the URI (or `--tls-server-name`), never the address; a `sips:` URI and an explicitly
secure transport never fall back to a cleartext candidate.

The three ways a named destination can fail are separate answers, because they have separate fixes.
Anything resolution refused is reported with `error` beginning `target resolution failed:`,
followed by which of them it was:

| What happened | `status` / exit | `error` continues |
|---|---|---|
| The zone answered, and has no usable record | `failed` (1) | `no usable candidate for <host>` |
| Nothing established an answer | `failed` (1) | `DNS lookup unavailable for <question>` |
| A question or the whole resolution ran out of time | `timeout` (5) | `DNS lookup timed out for <question>`, or `SIP target resolution timed out` |
| The name resolved and the peer refused the connection | `failed` (1) | *(not a resolution failure)* the transport cause, such as `transport: io: Connection refused`, with `candidates_attempted` beside it |

A deadline therefore has its own exit code, and the two remaining cases are told apart without
waiting by whether `error` opens with `target resolution failed:` at all — a connection failure
names the transport error and no resolution. `register` adds `registration_limit_ms` only when its
*own* deadline expired, so a name that will not resolve is never presented as a registrar that did
not answer.

A connection failure also says how far down the candidate list it got, because the transport cause
alone cannot: `Connection refused` reads the same from a name with one dead host behind it as from
a name where every address refused, and those are different problems with different owners.

| Field | Meaning |
|---|---|
| `candidates_attempted` | How many candidates were attempted **before the failure**, in order |
| `candidates_resolved` | How many resolution produced, attempted or not |

Equal numbers mean the list was walked to its end: nothing behind that name accepted. A
`candidates_attempted` lower than `candidates_resolved` means the pass stopped early — the
command's deadline is the ceiling over the whole of it — and says nothing about the candidates it
never reached. At most 16 are ever attempted, so a longer list stops there. Both fields appear only
when a serial pass actually ran; a refusal from the far end and a name that did not resolve carry
neither.

`SIPX_NAMESERVER` asks a specific resolver instead of the host's configured ones — an IP address,
optionally with a port, defaulting to 53. It is how a wrong zone is told from an unreachable
resolver, and it is refused rather than ignored when it cannot be read, including when an unset
shell variable expands to nothing. Literal destinations do not consult it, because they do not
consult a resolver.

## `sipx dial <URI>`

Place a call: `sipx dial sip:bob@pbx.example`

| Flag | Meaning |
|---|---|
| `--play <FILE>` | Play mono 16-bit WAV, linearly resampled from its header rate to the negotiated clock |
| `--record <FILE>` | Record the far end to WAV with that negotiated clock in its header |
| `--dtmf <DIGITS>` | Send these digits once the call is up |
| `--early-media` | Receive a reliable provisional media session before the final answer; incompatible with `--profile browser-audio` |
| `--duration <S>` | Hang up after this many seconds once connected (default 30); a supported process stop hangs up early and reports `interrupted` |
| `--timeout <S>` | Give up if not answered in this many seconds (default 20). It is also the ceiling on target resolution, so a name that will not resolve cannot spend the resolver's own eight seconds first. `0` waits as long as the transaction layer does — 32 seconds |
| `--cancel-timeout <S>` | Additional invitation-cancellation allowance after timeout or Ctrl-C (default 2). `0` performs no timed cancellation wait |
| `--from <URI>` | Our own address (default `sip:sipx@<local>`) |
| `--password <P>` | Digest password; prefer `SIPX_PASSWORD` because argv is world-readable |
| `--local <ADDR>` | Local address to bind (default `0.0.0.0:0`) |
| `--advertise <IP>` | Address written consistently into Via, Contact, and SDP; independent of `--local` |
| `--transport <T>` | Use `udp`, `tcp`, `tls`, `ws`, or `wss` (default `udp`) |
| `--tcp` | Legacy alias for `--transport tcp` |
| `--tls-server-name <N>` | Certificate identity to verify (default URI host) |
| `--tls-ca <FILE>` | Add PEM trust roots to the platform store |
| `--tls-cert <FILE>` | Mutual-TLS client certificate chain; requires `--tls-key` |
| `--tls-key <FILE>` | Mutual-TLS client private key; requires `--tls-cert` |
| `--profile <P>` | Select `standard` (default) or fail-closed `browser-audio`. The latter requires WSS plus the Opus and DTLS build features; it fixes codecs/keying and defaults ICE to `host` |
| `--codec <C>` | Select `pcmu`, `pcma`, `g722`, `l16`, or `opus`; repeat in preference order (default `pcmu`, then `pcma`). Opus requires the optional build feature; G.722 is always built in |
| `--media-security <M>` | Select `auto`, `plain`, `sdes`, or `dtls-srtp` (default `auto`). Explicit SDES requires TLS/WSS signalling |
| `--ice <P>` | Select `disabled`, `host`, or `stun` (default `disabled`) |
| `--stun-server <ADDR>` | STUN server as `host:port`; required by `--ice stun` and refused otherwise |
| `--audio-input <E>` | Local source: `wav:<path>`, `device:<id>`, or `null`. `--play` is the WAV alias |
| `--audio-output <E>` | Local sink: `wav:<path>`, `device:<id>`, or `null`. `--record` is the WAV alias |
| `--header <H>` | Add an application-owned INVITE field; repeat `Name: value` |
| `--stats` | Report call quality on exit: loss, jitter, round trip, MOS estimate |
| `--capture <FILE>` | Record the signalling to this [pcapng](https://en.wikipedia.org/wiki/Pcap) file for a bug report. Credentials are redacted — digest responses and opaque `Bearer`/`Basic` tokens, SRTP keys (`a=crypto`, `k=`), push tokens, instance URNs. **TLS and WSS are recorded decrypted**, because capturing ciphertext from inside the process would be worse than capturing outside it. What redaction cannot remove is identity: the file still says who called whom, when, and from where, so treat it as sensitive |
| `--counters <FILE>` | Write flattened signalling counters as JSON; `--capture` implies `<capture>.counters.json` |

Report fields: `status`, `ended_by`, `peer`, `media_advertised`, `media_bound`, `duration_ms`, `samples_recorded`, `heard_audio` — plus
`recording` when `--record` was given, and `loss`, `packets_lost`, `jitter_ms`, `mos`,
`round_trip_ms` under `--stats`. `--early-media` adds `early_media` and
`early_samples_recorded` to the terminal result. An explicit `--transport` also reports `requested_transport` and
`negotiated_transport`; omitting it adds neither transport field, and `--tcp` remains the legacy
alias rather than an explicit-selection report request.
`ended_by` is `duration`, `remote`, or `interrupt`; a locally originated BYE adds `bye_status` when
its final response was observed. A supported process stop emits one `status: interrupted` terminal
result with `stop_signal` after BYE and owned-work cleanup, and exits 0.
An invitation timeout reports `invitation_limit_ms`, measured `invitation_elapsed_ms`,
`cancel_limit_ms`, measured `cancel_elapsed_ms`, `cancel_sent`, `cancel_final_observed`,
`cancel_cleanup_completed`, and `cancel_cleanup_exhausted`. These make the maximum setup time the
sum of two named phases rather than hiding cancellation behind `--timeout`; Ctrl-C during setup
reports the same cancellation facts with `status: interrupted`.
Any explicit media selector adds `media_profile`, `requested_codecs`, `requested_media_security`, `requested_ice`,
`negotiated_codec`, `negotiated_media_security`, and `negotiated_ice`. Negotiated ICE is read from
the selected candidate pair and may be `checking`, `host`, `server-reflexive`, `peer-reflexive`, or
`relayed`; it is not copied from the request.
An established browser-audio call additionally reports `browser_role`, `ice_component`,
`negotiated_payload_type`, `negotiated_clock_rate`, `negotiated_keying`, `nominated_local`,
`nominated_remote`, `ice_generation`, both candidate types, `media_state`, and
`ingress_drops_total`. The nominated socket addresses and `running` state come from the live
media-owned component after ICE nomination and verified DTLS key installation.
When a device endpoint is selected, the result also names its exact stable identifier and effective
rate/channel/format configuration. `device_input_dropped_samples`,
`device_output_dropped_samples`, and `device_output_silence_samples` make callback pressure and
conversion gaps visible. These fields are measurements from the run, not requested settings.

WAV input is never silently reinterpreted. Its mono signed-16 format and header rate are explicit,
and supported rates are linearly resampled to the negotiated clock. Packet sizing likewise comes
from the running session (160 samples for a 20 ms G.711 or dynamic 8 kHz L16 packet, 882 for static
44.1 kHz L16, and 960 for Opus), and recordings use that rate in their WAV headers.

## `sipx answer`

Wait for a call and answer it: `sipx answer --play greeting.wav`

| Flag | Meaning |
|---|---|
| `--play <FILE>` | Play mono 16-bit WAV, linearly resampled from its header rate to the negotiated clock |
| `--record <FILE>` | Record the caller to WAV with that negotiated clock in its header |
| `--duration <S>` | Maximum call duration (default 30); remote BYE or a supported process stop ends it early |
| `--wait <S>` | Give up if no call arrives within this many seconds (default 60) |
| `--local <ADDR>` | Local address to bind (default `0.0.0.0:5060`) |
| `--advertise <IP>` | Address written consistently into Via, Contact, and SDP; independent of `--local` |
| `--transport <T>` | Listen for `udp`, `tcp`, `tls`, `ws`, or `wss` (default keeps the historical UDP/TCP listeners) |
| `--tcp` | Select the historical TCP listener explicitly |
| `--tls-cert <FILE>` | TLS/WSS server certificate chain; requires `--tls-key` |
| `--tls-key <FILE>` | TLS/WSS server private key; requires `--tls-cert` |
| `--profile <P>` | Select `standard` (default) or fail-closed `browser-audio`; the latter requires a WSS listener, Opus, DTLS, and ICE |
| `--codec <C>` | Select `pcmu`, `pcma`, `g722`, `l16`, or `opus`; repeat in preference order (default `pcmu`, then `pcma`) |
| `--media-security <M>` | Select `auto`, `plain`, `sdes`, or `dtls-srtp` (default `auto`) |
| `--ice <P>` | Select `disabled`, `host`, or `stun` (default `disabled`) |
| `--stun-server <ADDR>` | STUN server as `host:port`; required by `--ice stun` and refused otherwise |
| `--audio-input <E>` | Local source: `wav:<path>`, `device:<id>`, or `null`. `--play` is the WAV alias |
| `--audio-output <E>` | Local sink: `wav:<path>`, `device:<id>`, or `null`. `--record` is the WAV alias |
| `--header <H>` | Add an application-owned final-response field; repeat `Name: value` |
| `--reject` | Answer 603 Decline instead |
| `--busy` | Answer 486 Busy Here instead |
| `--once` | Exit after one call (the default; kept for clarity in scripts) |
| `--capture <FILE>` | Record the signalling to this [pcapng](https://en.wikipedia.org/wiki/Pcap) file for a bug report. Credentials are redacted — digest responses and opaque `Bearer`/`Basic` tokens, SRTP keys (`a=crypto`, `k=`), push tokens, instance URNs. **TLS and WSS are recorded decrypted**, because capturing ciphertext from inside the process would be worse than capturing outside it. What redaction cannot remove is identity: the file still says who called whom, when, and from where, so treat it as sensitive |
| `--counters <FILE>` | Write flattened signalling counters as JSON; `--capture` implies `<capture>.counters.json` |

Reports twice: `status: "listening"` with the bound `address` first, then
`status: "answered"` with `ended_by`, `caller`, `media_advertised`, `media_bound`, `duration_ms`, `samples_recorded`, `heard_audio` — plus
`dtmf` when digits arrived and `recording` when `--record` was given. Explicit selection adds the
requested transport to the listening report and both requested and negotiated transport to the
terminal report.
Remote BYE reports `ended_by: remote` only after its 200 response and media cleanup. Local duration
reports `ended_by: duration`; a supported process stop sends BYE, emits one
`status: interrupted`, `ended_by: interrupt` result with `stop_signal` after cleanup, and exits 0.
A locally originated BYE adds `bye_status` when its final response was observed.

`--profile browser-audio` is valid on both `dial` and `answer`. It cannot be combined with
`--codec` or `--media-security`, because the named profile fixes those choices; `--ice host` and
`--ice stun --stun-server <ADDR>` are the permitted gathering policies. A non-WSS selection or a
build without Opus/DTLS is refused before signalling or media I/O. Two sipx processes exercise the
composition directly; independent native-browser interoperability remains a separate proof and is
not inferred from this diagnostic command alone.
`--early-media` is also refused with this profile before transport binding: the first profile starts
ICE and DTLS only after a valid final answer, never from reliable provisional media.
Explicit media selection adds the three requested fields to the listening report and the same six
requested/negotiated fields documented for `dial` to the terminal report.
Device results carry the same selected-configuration and callback-counter fields documented for
`dial`.

## `sipx load <URI>`

Place a finite, reproducible call load:

```sh
sipx load sip:load@192.0.2.1:5060 --rate 10 --concurrency 32 --calls 100 --seed 41 --json
```

| Flag | Meaning |
|---|---|
| `--rate <CALLS/S>` | Positive finite arrival rate; required |
| `--concurrency <N>` | Positive ceiling on simultaneously active calls; required |
| `--calls <N>` | Stop after admitting this many calls |
| `--duration <S>` | Stop admission after this many seconds |
| `--call-duration <S>` | End each answered call after this many seconds (default 0) |
| `--timeout <S>` | Bound each call setup (default 20), and with it the one target resolution the run's calls share |
| `--mode <M>` | `signalling` (default) or the separately explicit `generated-media` workload |
| `--seed <N>` | Reproduce arrival jitter and deterministic workload data (default 0) |
| `--from <URI>` | Address used by the generated callers |
| `--password <P>` | Digest password; prefer `SIPX_PASSWORD` |
| `--local <ADDR>` | Local address to bind (default `0.0.0.0:0`) |
| `--transport <T>` | Use `udp`, `tcp`, `tls`, `ws`, or `wss` (default `udp`) |
| `--tcp` | Legacy alias for `--transport tcp` |
| `--tls-server-name <N>` | Certificate identity to verify (default URI host) |
| `--tls-ca <FILE>` | Add PEM trust roots to the platform store |
| `--tls-cert <FILE>` | Mutual-TLS client certificate chain; requires `--tls-key` |
| `--tls-key <FILE>` | Mutual-TLS client private key; requires `--tls-cert` |

At least one of `--calls` and `--duration` is required; when both are present, the first reached
closes admission. Reaching a bound or receiving a supported process stop signals all owned calls to
end and waits for their cleanup before emitting the summary. Cleanup has a 40-second failure bound,
longer than the SIP transaction ceiling; exhaustion exits 1 and reports `status: "failed"`.

The default sends bodyless INVITE/2xx/ACK/BYE dialogs and creates no SDP, RTP socket or media task,
matching `load-responder`'s default. Select `--mode generated-media` on both commands for the
deterministic PCMU/RTP workload. A paired mode mismatch is refused before dialog admission and both
commands report failure after cleanup.

JSON output is exactly one `sipx.load.v1` object. It records the effective mode, terminal reason,
seed and effective limits;
attempted, connected, rejected, timed-out and failed calls; peak concurrency; response-code counts;
p50/p95/p99 setup time; and aggregate media loss, jitter and MOS snapshots. Missing measurements
are `null`, not zero. `stop_signal` is `null` after natural completion and names the first supported
signal otherwise. A run that reaches a configured bound is `completed`; a cleanly drained process
stop is `interrupted`. An internal worker or media error is `failed`/exit 1 and retains its
actionable reason; it is never relabeled as an operator interruption.

## `sipx load-responder`

Answer a finite, machine-driven signalling load:

```sh
sipx load-responder --max-active 32 --calls 100 --cleanup 40 --seed 41 --json
```

| Flag | Meaning |
|---|---|
| `--max-active <N>` | Positive ceiling on simultaneously owned dialogs; required |
| `--calls <N>` | Close admission after this many surfaced INVITEs |
| `--duration <S>` | Close admission after this many seconds |
| `--cleanup <S>` | Positive deadline for dialog, task and transaction drain; required |
| `--seed <N>` | Reproduce policy choices and generated media (default 0) |
| `--provisional-percent <P>` | Percentage of admitted INVITEs receiving one `100 Trying` (default 0) |
| `--answer-percent <P>` | Percentage answered with `200`; the remainder use `--reject-status` (default 100) |
| `--reject-status <CODE>` | Policy rejection from 400 through 699 (default 486) |
| `--dialog-duration <S>` | Positive maximum lifetime of an accepted dialog (default 40) |
| `--mode <M>` | `signalling` (default) or the separately explicit `generated-media` workload |
| `--local <ADDR>` | UDP address to bind (default `127.0.0.1:0`) |
| `--transport <T>` | Must be `udp`; other transports are separate measurement profiles |

At least one of `--calls` and `--duration` is required; the first reached closes admission. Before
traffic is admitted, stdout receives one flushed `sipx.comparative-load.ready.v1` JSON record with
the exact bound address, process identity, effective limits and policy. This readiness record is
always JSON so a supervisor never has to scrape prose. In `--json` mode the only later stdout line
is the terminal `sipx.load-responder.v1` summary.

The default creates no SDP or media session. `--mode generated-media` is an explicit, separate
workload that drives deterministic PCMU media rather than silently changing the signalling
baseline. The responder validates ACK, CANCEL and BYE as dialog actions; arbitrary packets do not
become successful outcomes. Admission stop, interruption and internal error cancel and join every
owned dialog before reporting. A successful summary therefore has zero `active_dialogs`,
`dispatcher_routes`, `endpoint_transactions` and `owned_tasks` under `post_drain`.

The terminal summary records invitations and response statuses; admitted, established, completed,
cancelled, rejected and failed outcomes; active high-water; p50/p95/p99 setup and teardown latency;
invalid messages; the exact effective bounds; and `stop_signal`, which is `null` after natural
completion and names the first supported signal otherwise. A response status is counted once when the
responder successfully sends it, or when a valid final response returns for a BYE the responder
originated. Protocol retransmissions do not inflate the map, and invalid responses are counted as
invalid messages instead. UDP is the v1 baseline so connection setup and reuse costs cannot
contaminate the SIP transaction measurement.

Generated-media mode deliberately keeps the same small dialog vocabulary as signalling mode:
after the initial ACK it accepts ACK and BYE, and refuses other in-dialog methods with a measured
405. That keeps the load result about bounded call setup and teardown rather than application
features such as transfer or renegotiation. Duplicate Call-ID, From, To or CSeq fields are rejected
as malformed before they can match or mutate a dialog.

## `sipx register <AOR>`

Register with a registrar: `sipx register sip:alice@example.com`

| Flag | Meaning |
|---|---|
| `--password <P>` | Password. Prefer the `SIPX_PASSWORD` environment variable — argv is world-readable |
| `--target <ADDR>` | Where to send, if not derived from the AOR (`host:port`) |
| `--expires <S>` | Lease to ask for, in seconds (default 3600) |
| `--timeout <S>` | Give up if the registration has not completed in this many seconds (default 20). Same flag and units as `dial`, over the whole attempt: resolution, connection, the REGISTER transaction and any authentication retry. `0` waits as long as the transaction layer does — 32 seconds per transaction |
| `--local <ADDR>` | Local address to bind (default `0.0.0.0:0`) |
| `--transport <T>` | Use `udp`, `tcp`, `tls`, `ws`, or `wss` (default `udp`) |
| `--tcp` | Legacy alias for `--transport tcp` |
| `--tls-server-name <N>` | Certificate identity to verify (default AOR domain) |
| `--tls-ca <FILE>` | Add PEM trust roots to the platform store |
| `--tls-cert <FILE>` | Mutual-TLS client certificate chain; requires `--tls-key` |
| `--tls-key <FILE>` | Mutual-TLS client private key; requires `--tls-cert` |
| `--header <H>` | Add an application-owned REGISTER field; repeat `Name: value` |
| `--keep-alive` | Keep refreshing until interrupted |
| `--outbound` | Register as one Outbound flow (RFC 5626): `reg-id` and `+sip.instance` on the Contact, the `outbound` option tag offered |
| `--instance <URN>` | With `--outbound`: present this device identity rather than a freshly generated one — §4.1 wants it stable across restarts, and the CLI keeps no state, so persisting one is the caller's job |
| `--push-provider <P>` | Push notification service this device can be woken through (RFC 8599). Requires `--push-prid` |
| `--push-prid <T>` | The identifier the push service knows this device by. Requires `--push-provider` |
| `--push-param <X>` | Service-specific extra, when the service needs one |
| `--capture <FILE>` | Record the signalling to this [pcapng](https://en.wikipedia.org/wiki/Pcap) file for a bug report. Credentials are redacted — digest responses and opaque `Bearer`/`Basic` tokens, SRTP keys (`a=crypto`, `k=`), push tokens, instance URNs. **TLS and WSS are recorded decrypted**, because capturing ciphertext from inside the process would be worse than capturing outside it. What redaction cannot remove is identity: the file still says who called whom, when, and from where, so treat it as sensitive |
| `--counters <FILE>` | Write flattened signalling counters as JSON; `--capture` implies `<capture>.counters.json` |
| `--wake` | Act as though a push arrived once registered: send §4.1.3's binding-refresh REGISTER and report what it learned. Requires the push flags |

Report fields: `status`, `aor`, `expires`, `refresh_in` — plus `flow` under `--outbound`
(whether the registrar reported an Outbound registration, RFC 5626 §6) and `push` under the push
flags (whether the registrar named the same push service, RFC 8599 §8.2). `--wake` adds a second
report line with `status: "woken"` and, when the registrar assigned one, `purr`. Explicit transport
selection adds `requested_transport` and `negotiated_transport` to the registration result.

An attempt that runs out of time exits `timeout` (5) and reports `registration_limit_ms`, measured
`registration_elapsed_ms` and measured `cleanup_ms` beside `aor` and `error` — the same fields in
text and JSON, on stderr like every other failure. `cleanup_ms` is the join of what the abandoned
attempt owned, so the stated deadline is never presented as the whole of the elapsed process time.
Those fields are what keep a deadline distinguishable from a registrar that answered: a refusal
exits `rejected` (3) or `unauthorized` (4) with its SIP status, and a connection nothing accepted
exits `failed` (1) with the transport cause, neither of them waiting for the deadline. `--keep-alive`
refreshes after a bounded attempt succeeds — the deadline bounds each attempt, not the lifetime of a
registration being kept, so every refresh is bounded by it too and one invocation registers once
rather than twice — and `--wake`'s binding refresh is a second attempt bounded the same way.

Whichever way it ends, the record that ends a command is written after the work behind it has been
joined: the endpoint is shut down, its transactions and timers are cancelled and waited on, and only
then is the result printed. So a script that reads `--counters`, closes a `--capture` or reuses the
`--local` port the moment it has the result is not racing anything this invocation left running.
Under `--keep-alive` and `--wake` the registration line is progress rather than the last word, and
the barrier belongs to the line that ends the run.

Combinations that cannot work are usage errors (exit 2), never parsed and dropped: half a push
pair, `--push-param` alone, `--wake` without the push flags, `--instance` without `--outbound`,
an `--instance` that is not a URN (RFC 5626 §4.1's grammar is `instance-val = urn`), and a
`--push-prid` that a URI parameter cannot hold. A valued flag left without a value — `--instance`
with nothing after it, or `--target=` — is refused for every command by the rule stated at the top
of this page, not by anything specific to `register`.

## `sipx peers`

List what can be called: `sipx peers --json`

| Flag | Meaning |
|---|---|
| `--book <FILE>` | Read this peer book; with `--registrar`, merge it explicitly |
| `--registrar <AOR>` | Subscribe to this registrar's current registrations |
| `--password <P>` | Digest password; prefer `SIPX_PASSWORD` because argv is visible |
| `--target <ADDR>` | Registrar host or address, when it cannot be derived from the AOR |
| `--expires <S>` | Positive requested subscription lifetime (default 3600). This command states no attempt deadline, so it is also the ceiling on target resolution: a subscription that may live one second does not spend eight finding the registrar |
| `--watch <S>` | Keep applying updates for this many seconds after the first snapshot |
| `--local <ADDR>` | Local signalling bind address |
| `--transport <T>` | `udp`, `tcp`, `tls`, `ws`, or `wss`, with the shared TLS options |
| `--tcp` | Legacy alias for `--transport tcp` |
| `--tls-server-name <N>` | Certificate identity to verify (default AOR domain) |
| `--tls-ca <FILE>` | Add PEM trust roots to the platform store |
| `--tls-cert <FILE>` | Client certificate chain for mutual TLS; requires `--tls-key` |
| `--tls-key <FILE>` | Client private key for mutual TLS; requires `--tls-cert` |

The book is looked for in `--book`, then `$SIPX_PEERS`, then `$XDG_CONFIG_HOME/sipx/peers`, then
`$HOME/.config/sipx/peers`. It is a text file a shell can write — one peer per line, a name and a
URI separated by whitespace, `#` for a comment, blank lines ignored:

```text
# who this phone knows about
alice   sip:alice@192.0.2.17:5060
bob     sips:bob@example.com
```

```sh
echo "carol sip:carol@192.0.2.30:5060" >> ~/.config/sipx/peers
sipx peers --json | jq -r 'select(.source == "book") | .uri'
```

Reports one line per peer with `status` (always `peer`), `name`, `uri` and `source`. Book entries
carry `source=book` and no invented age. Live contacts carry `source=registrar` and `age`, in whole
seconds since the last complete snapshot was accepted.

```sh
SIPX_PASSWORD="$secret" sipx peers \
  --registrar sip:alice@example.com \
  --target registrar.example:5060 \
  --watch 30 --json
```

The command waits for a full registration snapshot, applies later partial NOTIFY documents, and
prints only the final current set. Pass `--book` in that form to merge a local book; environment and
default book locations are deliberately not implicit in a registrar query. A 403 exits
`unauthorized`, a 489 exits `rejected`, and a missing initial NOTIFY exits `timeout`. None falls back
to a book-only success, because that would present an incomplete answer as complete.

A book that cannot be read — missing, unreadable, or holding a line that is not a name and a URI —
exits non-zero and names the file and the line. It never prints an empty list: on a fresh machine
that would read as "there is nobody to call" when the truth is "you have not been told about
anyone". A book that exists and holds no peers prints nothing and exits 0.

Without `--registrar`, the command remains file-only and opens no socket.

## `sipx devices`

List stable audio device identifiers: `sipx devices --json`

This command is available in builds with the optional `device-audio` feature. Without that feature
it exits 1 and names the feature; the file-only binary neither resolves nor links a platform audio
dependency. The command enumerates devices but opens no stream.

JSON is one `sipx.devices.v1` object whose `devices` array is sorted by `id`. Each entry carries
`id`, human-readable `name`, and the `input`/`output` direction booleans. The identifier is opaque
and backend-qualified, for example `alsa:hw:CARD=Loopback,DEV=0`; pass the complete returned string
as `device:<id>` to `--audio-input` or `--audio-output`. Names are display text and cannot be used as
selectors.

An explicit selector never falls back to the default device. A binary without `device-audio`
refuses the selector as usage; in a device-enabled binary, missing, busy, permission-denied and
unsupported devices fail before signalling is bound. Streams accept bounded linear PCM conversion,
use one second of non-blocking callback queue per direction, report dropped or silent samples, and
are stopped and joined before the terminal call result is emitted.

## `sipx scenario`

Drive one call actor with correlated newline-delimited JSON. The process emits a
`scenario.ready` envelope, reads one command object per line from stdin, and echoes each command's
string `id` in its completion or refusal event.

| Flag | Meaning |
|---|---|
| `--local <ADDR>` | Local signalling address (default `0.0.0.0:0`) |
| `--transport <T>` | Use `udp`, `tcp`, `tls`, `ws`, or `wss` (default `udp`) |
| `--tcp` | Legacy alias for `--transport tcp` |
| `--tls-server-name <N>` | Certificate identity to verify (default URI host) |
| `--tls-ca <FILE>` | Add PEM trust roots to the platform store |
| `--tls-cert <FILE>` | Certificate chain for TLS/WSS; pair with `--tls-key` |
| `--tls-key <FILE>` | Private key paired with `--tls-cert` |
| `--codec <C>` | Select `pcmu`, `pcma`, `g722`, `l16`, or `opus`; repeat in preference order |
| `--media-security <M>` | Select `auto`, `plain`, `sdes`, or `dtls-srtp` |
| `--ice <P>` | Select `disabled`, `host`, or `stun` |
| `--stun-server <ADDR>` | STUN server as `host:port` for `--ice stun` |
| `--header <H>` | Add an application-owned field to originated INVITEs; repeat |
| `--timeout <S>` | Default outbound answer timeout (default 20), covering each `dial` command's target resolution; a frame's own `timeout_ms` replaces it and bounds that command's resolution instead |

The v1 commands are `dial`, `accept`, `reject`, `play`, `stop_playback`, `start_recording`,
`stop_recording`, `send_dtmf`, `hold`, `resume`, `transfer`, `hangup`, `wait_for`, and `shutdown`.
The canonical input is a flat frame such as
`{"id":"dial-1","command":"dial","uri":"sip:echo@127.0.0.1:5060"}`. `do` is accepted as a
compatibility alias only when `command` is absent. The nested form
`{"id":"dial-1","dial":{"uri":"…"}}` is not accepted.

| Command | Required fields | Optional fields |
|---|---|---|
| `dial` | `uri` | `target` alias, `from`, `timeout_ms`, string-array `headers` |
| `accept` | — | — |
| `reject` | — | `status` (300–699, default 603), `reason` |
| `play` | `path` | — |
| `stop_playback` | — | — |
| `start_recording` | `path` | — |
| `stop_recording` | — | — |
| `send_dtmf` | `digits` | — |
| `hold`, `resume` | — | — |
| `transfer` | `target` | — |
| `hangup` | — | — |
| `wait_for` | `event`, unsigned `timeout_ms` | — |
| `shutdown` | — | — |

This executable stream dials, waits for the actual answer event, hangs up, and shuts down:

```sh
printf '%s\n' \
  '{"id":"dial-1","command":"dial","uri":"sip:echo@127.0.0.1:5060","timeout_ms":5000}' \
  '{"id":"wait-1","command":"wait_for","event":"call.answered","timeout_ms":5000}' \
  '{"id":"hangup-1","command":"hangup"}' \
  '{"id":"shutdown-1","command":"shutdown"}' \
  | sipx scenario --local 127.0.0.1:0
```

There is no sleep command. EOF requests orderly shutdown. Malformed JSON and refused commands emit
correlated `scenario.command.refused` events without corrupting later frames. The final event is
`scenario.stream.completed` or `scenario.stream.failed`; the latter exits 1 after cleanup, even if
a later command succeeded. A clean empty stream is an explicit completed no-op. Duplicate IDs are
refused, and each ID is bounded to 128 UTF-8 bytes.

## Exit codes

Scripts branch on the exit code, not on parsing prose:

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Failed (transport or protocol error) |
| 2 | Usage — the command line itself was wrong |
| 3 | Rejected — the far end refused |
| 4 | Unauthorized — credentials wrong or missing |
| 5 | Timeout — nothing answered in time |
| 6 | Busy |

Both `dial` and `answer` exit 0 after a completed call that received no audio. Silence is not a
signalling failure: a caller can legitimately stay quiet, a one-way announcement can send without
receiving, and `--record` asks the command to preserve whatever arrives rather than asserting that
something must arrive. Giving those successful calls a failure status would make the exit code
depend on an application policy the command was never given.

The media result remains machine-readable. A script that requires received audio uses `--json` and
requires `heard_audio: true`; `heard_audio: false` with `samples_recorded: 0` is a successfully
completed silent call. This rule is identical for `dial` and `answer` so the direction of the call
does not change what an exit status means.

## The JSON contract

`--json` emits exactly one JSON object per report, on one line, on stdout — a command with more
than one thing to report, such as `answer`'s bound address or `peers`' list, emits one such line
each; failures emit
`{"status": …, "error": …}` on **stderr**. The text and JSON forms carry the same field set —
that equality is asserted by a test, so a field you see in one is in the other. Every object member
name is unique. The process tests decode raw members recursively before constructing a map, so a
repeated field at the root or inside a nested result is rejected instead of silently overwritten.

Five outputs have versioned schemas or envelopes. The checked table below is held against the Rust
producers by `./scripts/check-cli-reference.py --check`; the same checker executes root and
subcommand help and compares their commands and long options with the sections above. Event-specific
scenario details extend the `event` object and do not define a second envelope.

<!-- BEGIN cli-json-contracts -->
| Contract | Producer | Required structural fields |
|---|---|---|
| `sipx.devices.v1` | `device` | `schema`, `devices`, `id`, `name`, `input`, `output` |
| `sipx.load.v1` | `load` | `schema`, `status`, `stop_signal`, `reason`, `mode`, `seed`, `target`, `limits`, `rate`, `concurrency`, `calls`, `duration_ms`, `call_duration_ms`, `setup_timeout_ms`, `cleanup_ms`, `outcomes`, `attempted`, `connected`, `rejected`, `timed_out`, `failed`, `peak_concurrency`, `response_codes`, `setup_ms`, `p50`, `p95`, `p99`, `media`, `snapshots`, `packets_lost`, `mean_loss`, `mean_jitter_ms`, `mean_mos` |
| `sipx.comparative-load.ready.v1` | `load_responder_readiness` | `active`, `address`, `events`, `limits`, `pid`, `role`, `schema`, `stderr_bytes`, `stdout_bytes`, `transport` |
| `sipx.load-responder.v1` | `load_responder` | `active_dialogs`, `active_high_water`, `admitted`, `calls`, `cancelled`, `cleanup_ms`, `completed`, `count`, `counts`, `dialog_duration_ms`, `dispatcher_routes`, `duration_ms`, `endpoint_transactions`, `established`, `failed`, `invalid_messages`, `invitations`, `latency_ms`, `limits`, `max_active`, `maximum`, `mode`, `owned_tasks`, `p50`, `p95`, `p99`, `post_drain`, `reason`, `rejected`, `responses`, `schema`, `seed`, `setup`, `status`, `stop_signal`, `teardown` |
| `sipx.app.v1` | `scenario` | `contract`, `seq`, `at`, `call`, `event`, `id`, `leg`, `direction`, `state`, `from`, `to`, `headers`, `media`, `encrypted`, `on_hold`, `muted`, `legs`, `bridged`, `tags`, `type`, `command`, `message` |
<!-- END cli-json-contracts -->
