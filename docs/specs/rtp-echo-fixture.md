# Bounded RTP echo fixture

**Status:** accepted · **Story:** M-53 · **Surface:** `sipx-testkit::rtp_echo`

## 1. Scope

This specification defines a finite diagnostic peer for one RTP/AVP PCMU stream. It exists to let a
downstream test or shell process prove that RTP packets cross its media boundary in both directions.
It is not a SIP user agent, an SDP negotiator, an RTCP participant, a mixer, an acoustic-echo
canceller, a general reflector service or a load generator.

The fixture uses the public `sipx-rtp` packet model and public `sipx-audio` G.711 codec. UDP ownership
lives here, above those pure crates. It does not duplicate `sipx-media`'s session, jitter, pacing,
SRTP, ICE or reporting machinery.

## 2. Configuration and bounds

`EchoConfig` contains:

| Field | Meaning | Refusal |
|---|---|---|
| `bind` | Exact local UDP address | address-family mismatch with `peer` |
| `peer` | Sole admitted source and destination | unspecified address or port zero |
| `packets` | Number of valid packets to echo | represented by `NonZeroUsize` |
| `within` | Whole-run deadline | zero duration |

The receiver owns one UDP socket, one 2049-byte receive buffer and no background task. A datagram
longer than 2048 bytes is refused as truncated rather than parsed partially. Only RTP version 2,
payload type 0 PCMU from the configured peer is admitted. A malformed packet, unexpected peer,
unsupported payload type, closed socket or elapsed deadline is a typed terminal error.

## 3. State and ownership

| State | Input | Action | Next |
|---|---|---|---|
| `Bound` | `run` | set one absolute deadline; receive a datagram | `Receiving` |
| `Receiving` | valid PCMU from `peer` | decode samples, encode them, send one RTP packet | `Receiving` or `Complete` |
| `Receiving` | malformed/foreign/oversized input | return typed error and drop socket | `Failed` |
| `Receiving` | deadline | return typed timeout and drop socket | `Failed` |
| any live state | future dropped | drop the sole socket; no task remains to cancel | terminal |

`run` consumes the fixture. Completion, error and cancellation therefore all drop the socket. The
implementation MUST NOT spawn a task, because a task handle would create a second lifecycle that
the caller would have to reap.

## 4. RTP transformation

RFC 3550 §5.1 defines the sequence number as increasing by one per RTP packet and the timestamp as
the sampling instant. The fixture owns an independent deterministic outbound stream:

- SSRC `0x53505854` (`SPXT`), payload type 0, marker clear;
- first sequence number `0`, wrapping as `u16`;
- first timestamp `0`, wrapping as `u32`;
- after each reply, sequence advances by one and timestamp advances by the number of decoded PCMU
  samples in that reply.

Input sequence numbers and timestamps do not become output identity. Payload is decoded to signed
samples and encoded again, so the peer proves the public codec seam rather than a byte reflector.

## 5. Test vectors

For three 160-byte PCMU inputs with arbitrary input identities, the reply headers are:

| Reply | Fixed header bytes before SSRC | SSRC | Next state |
|---:|---|---|---|
| 1 | `80 00 00 00 00 00 00 00` | `53 50 58 54` | sequence `1`, timestamp `160` |
| 2 | `80 00 00 01 00 00 00 a0` | `53 50 58 54` | sequence `2`, timestamp `320` |
| 3 | `80 00 00 02 00 00 01 40` | `53 50 58 54` | complete |

The integration proof injects recognizable non-silent frames, parses every reply with
`sipx_rtp::Packet`, compares decoded samples, asserts the table's relative progression, and then
rebinds the released address while the runtime task count equals its baseline.
