<div class="sipx-hero">
  <img src="assets/logo.svg" alt="">
  <h1>sipx</h1>
</div>

<p class="sipx-tagline">A SIP and VoIP stack in Rust. Place and answer calls, register against a
PBX, carry real audio — as a library you embed, or as a command you run.</p>

## Can it do what I need?

Today sipx is a **user agent** — a phone. It calls, answers, registers, transfers, bridges and
conferences. It is **not a proxy or a registrar**: it does not fork requests or hold other
people's registrations.

| | |
|---|---|
| **Calls** | Place and answer, SDP offer/answer, hold and resume, blind and attended transfer |
| **Audio** | G.711 µ-law and A-law, Opus behind a feature, DTMF, play and record WAV |
| **Signalling security** | TLS and secure WebSocket, with certificate verification that **cannot be turned off** |
| **Media security** | **Not yet** — audio goes out unencrypted. It is next on [the RFC roadmap](rfc-roadmap.md). |
| **Transports** | UDP, TCP, TLS, WebSocket, secure WebSocket |
| **Reachability** | NAT via `rport` and symmetric RTP. No Outbound, Path, GRUU or push yet. |
| **Multi-party** | Bridge two calls, or conference several with N−1 mixing |
| **Quality** | Loss, jitter, round-trip time and an estimated MOS, readable mid-call |

It is verified against **Kamailio**, not only against itself: registration over UDP, TCP, TLS
and WebSocket — and the refusals that make the successes mean something.

## The honest version

Two things are worth knowing before you decide.

**Media is not encrypted.** Signalling can be — `sips:` and WSS work, and the TLS policy has no
way to disable verification anywhere — and then the audio goes out in the clear. That is the
largest gap in the stack and the first thing on the roadmap.

**It is a phone, not an exchange.** If you need something that routes other people's calls,
sipx is not that yet, and the [compliance table](compliance.md) says so per RFC rather than
leaving you to find out.

## What is here

- **[What sipx supports, RFC by RFC](compliance.md)** — 61 RFCs, each marked implemented,
  partial, parse-only or not started. Generated from a registry and checked in CI, so it is a
  measurement rather than a claim.
- **[RFC roadmap](rfc-roadmap.md)** — which gaps close next and why in that order.
- **[Project status](roadmap.md)** — what has been delivered, milestone by milestone.
- **[Why it exists](vision.md)** — the principles the code is held to.

## How it is built

**The core does no I/O.** Parsing, the transaction state machines and dialog state are pure
functions over inputs and outputs — no sockets, no tasks, no clock. Time arrives as a
fired-timer input and leaves as a set-timer output.

That is the load-bearing decision. The parts of SIP that are genuinely hard — retransmission
timing, transaction matching, hostile input — are tested deterministically and fuzzed without a
runtime, rather than chased through timing flakes in integration tests.

Three things follow from it:

- **Nothing is lost on the wire.** A parsed message borrows the bytes it arrived in, and headers
  sipx has no behaviour for still survive intact. That is why *parse-only* is a status in the
  compliance table rather than a gap in it.
- **Ownership, not sharing.** A call owns its media pipeline. Bridging moves frames over
  channels; no media session sits behind a mutex.
- **Malformed input is a value, not a panic.** `unsafe` is forbidden across the workspace and
  parse failures are typed errors. The whole RFC 4475 torture corpus is asserted — including the
  messages that must be *rejected*, and by which layer.

## Getting it

The source is on [GitHub](https://github.com/codewandler/sipx), under `MIT OR Apache-2.0`.
Guides and API documentation are being written; until they land, the
[README](https://github.com/codewandler/sipx#readme) has the shortest path to a working call.
