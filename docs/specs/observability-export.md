# Spec: application-owned observability export

## 1. Scope and sources

This contract makes two facts that sipx already computes reachable outside the process:

1. redacted SIP messages leave the existing signalling-capture boundary as HEP3 datagrams; and
2. RTCP report blocks become per-stream quality samples delivered to an application callback.

It deliberately does not select, configure or depend on a metrics backend. The application owns
the callback and any queue, aggregation, labels, retention or network export behind it. Coupling a
media worker to one backend would make that backend's latency and availability part of every call,
and would make a backend choice part of sipx's public media contract.

The normative protocol sources for the media half are:

- **RFC 3550 §6.4.1 and §6.4.2** — sender/receiver reports and report-block fields;
- **RFC 3550 §6.4.3** — the round-trip calculation from `LSR` and `DLSR`; and
- **RFC 5761 §4** — recognizing RTCP when RTP and RTCP share one port.

HEP3 has no IETF RFC. Section 3 is therefore sipx's complete byte contract for the subset it
emits; implementations and tests do not depend on an unnamed external description.

## 2. Types and ownership

| Type | Owner | Meaning |
|---|---|---|
| `HepConfig` | `sipx-transport` endpoint configuration | collector socket address and capture-agent id |
| `CaptureConfig::hep` | existing capture configuration | optional best-effort HEP3 companion to the pcapng writer |
| `RtcpQualitySample` | `sipx-media` | one peer report block describing one local RTP stream |
| `RtcpQualityHook` | application | cloneable callback installed on a media session or call |

HEP export is intentionally attached to `CaptureConfig`, not to a second observation point. A
message is stamped once in the transport driver, passed through the existing redaction function,
then handed over the existing bounded capture queue. The file and collector receive the same
redacted bytes. Because a network export leaves the process, configuring HEP while capture
redaction is disabled is a typed configuration refusal.

`RtcpQualityHook` is synchronous. sipx invokes it after RTCP authentication and parsing, outside
all sipx locks. The callback must return promptly; an application that needs blocking work puts a
bounded queue behind the callback. A callback panic is caught and logged so application code
cannot terminate the RTCP receive worker.

## 3. HEP3 datagram subset

Every datagram starts with ASCII `HEP3`, followed by a two-byte big-endian total datagram length.
The remainder is a sequence of chunks. Each chunk has a six-byte header:

```text
vendor-id: u16 big-endian (zero)
type-id:   u16 big-endian
length:    u16 big-endian, including this six-byte header
value:     length - 6 bytes
```

Chunks appear exactly in this order:

| Type | Value |
|---:|---|
| `0x0001` | IP family: `2` for IPv4, `10` for IPv6 |
| `0x0002` | IP protocol: `17` for UDP/QUIC, `6` for TCP/TLS/WS/WSS |
| `0x0003` or `0x0005` | source IPv4 or IPv6 octets |
| `0x0004` or `0x0006` | destination IPv4 or IPv6 octets |
| `0x0007` | source port, `u16` big-endian |
| `0x0008` | destination port, `u16` big-endian |
| `0x0009` | Unix timestamp seconds, low `u32`, big-endian |
| `0x000a` | timestamp microseconds within the second, `u32` big-endian |
| `0x000b` | payload protocol `1` (SIP) |
| `0x000c` | configured capture-agent id, `u32` big-endian |
| `0x000f` | redacted SIP bytes |

The source and destination follow wire direction: outbound uses local then peer; inbound uses peer
then local. A record whose endpoints have different address families or whose encoded size exceeds
`u16::MAX` is dropped from HEP export and remains eligible for the pcapng file.

## 4. Signalling export state and failure isolation

| State | Event | Result |
|---|---|---|
| off | any message | no socket, allocation or worker beyond the existing `Option` check |
| active | capture queue accepts | pcapng write proceeds; non-blocking UDP HEP send is attempted |
| active | capture queue full | record is dropped once and `capture.dropped` increments |
| active | HEP send succeeds | `capture.hep_records` increments |
| active | HEP encode/send fails | `capture.hep_dropped` increments; the first failure is warned, later failures are debug logged; file capture and call continue |
| active | pcapng write fails | existing `capture.errors` behavior disables capture and logs once |

The collector socket is non-blocking. No HEP send, DNS operation, retry, acknowledgement or
collector response runs on the transport driver. UDP is deliberately best effort: retrying inside
sipx would either reorder observations or create an unbounded delivery queue. Applications that
require durable export consume the local pcapng file after the call.

## 5. RTCP quality sample

For each authenticated, well-formed sender or receiver report, sipx selects the report block whose
`SSRC` names the local session's outbound stream. If present, one callback sample is formed:

| Field | Derivation |
|---|---|
| `reporter_ssrc` | report packet sender SSRC |
| `stream_ssrc` | report block SSRC |
| `loss` | `fraction_lost / 256`, clamped by the field's `u8` range |
| `cumulative_lost` | signed 24-bit cumulative loss already decoded by `sipx-rtp` |
| `jitter` | report-block jitter divided by the negotiated RTP clock rate |
| `round_trip` | RFC 3550 `A - LSR - DLSR`, or `None` when `LSR` is zero or arithmetic is invalid |

Malformed or unauthenticated RTCP produces no callback. A report about another SSRC produces no
callback for this single-stream session.

The hook belongs to the logical call, not one socket generation. `Call::set_rtcp_quality_hook`
installs it on the running session. A re-INVITE that keeps the session keeps the same slot; a
re-INVITE that replaces the media session copies the hook before the replacement becomes visible.
An ICE restart changes the selected path inside the running session and therefore retains the same
hook. Clearing the hook follows the same rules.

## 6. Test vectors

| ID | Input | Required observation |
|---|---|---|
| OE-H-1 | outbound IPv4 UDP SIP message at second `1`, microsecond `2`, agent id `0x01020304` | exact chunk order in §3; local is source; payload type is SIP |
| OE-H-2 | capture containing a digest `response`, HEP enabled | neither HEP payload nor pcapng packet contains the response; diagnosable challenge fields remain |
| OE-H-3 | HEP sink refuses a datagram | call/capture observation returns immediately; `hep_dropped` increments; later pcapng records remain writable |
| OE-Q-1 | RR block for local SSRC with loss `64`, jitter `800` at 8 kHz, valid `LSR`/`DLSR` | callback reports 0.25 loss, 100 ms jitter and computed round trip |
| OE-Q-2 | callback installed, then media-moving re-INVITE, then ICE restart | the same hook remains installed after each generation transition |
| OE-Q-3 | callback panics | panic is isolated and a later RTCP report is still processed |
