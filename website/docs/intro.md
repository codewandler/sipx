---
title: What sipx is
description: A SIP and VoIP stack in Rust — a phone as a library you embed or a command you run, with an honest account of what it does not do yet.
slug: /
---

# sipx

A SIP and VoIP stack in Rust. Place and answer calls, register against a PBX, carry real
audio — as a library you embed, or as a command you run.

## Can it do what I need?

Today sipx is a **user agent** — a phone. It calls, answers, registers, transfers, bridges and
conferences. It is **not a proxy or a registrar**: it does not fork requests or hold other
people's registrations.

| | |
|---|---|
| **Calls** | Place and answer, SDP offer/answer, hold and resume, blind and attended transfer, session timers |
| **Audio** | G.711 µ-law and A-law, Opus behind a feature, DTMF, play and record WAV |
| **Signalling security** | TLS and secure WebSocket, with certificate verification that **cannot be turned off** |
| **Media security** | SRTP, keyed by SDES when the signalling is secure or by DTLS-SRTP on the media path |
| **Transports** | UDP, TCP, TLS, WebSocket, secure WebSocket |
| **Reachability** | NAT via `rport` and symmetric RTP; `Path` and `Service-Route` honoured; RFC 5626 Outbound down a client-opened flow. No GRUU, push or ICE yet. |
| **Multi-party** | Bridge two calls, or conference several with N−1 mixing |
| **Quality** | Loss, jitter, round-trip time and an estimated MOS, readable mid-call |

It is verified against **Kamailio**, not only against itself: registration over UDP, TCP, TLS
and WebSocket — and the refusals that make the successes mean something.

## The honest version

Two things are worth knowing before you decide.

**It is a phone, not an exchange.** If you need something that routes other people's calls,
sipx is not that yet, and the [compliance table](reference/compliance.md) says so per RFC
rather than leaving you to find out.

**The encryption has edges.** Media is encrypted when the signalling is — `sips:` or WSS —
using SRTP's default transform, keyed one of two ways. SDES puts the key in the SDP body, so any
intermediary that terminates the TLS can read it. DTLS-SRTP (RFC 5763 and RFC 5764) keys on the
media path instead and does not have that property; everything those RFCs decide is compiled
always, and only the handshake sits behind the off-by-default `dtls` feature. What is left is
one transform and no rekeying, which is why [the table](reference/compliance.md) marks RFC 3711
*partial* rather than implemented.

## Where to go from here

- **[Getting started](getting-started.md)** — a working call from a terminal in five minutes.
- **[Does sipx fit?](guides/does-this-fit.md)** — the honest version, including the edges.
- **Guides** — [place a call](guides/place-a-call.md), [answer one](guides/answer-a-call.md),
  [register against a PBX](guides/register.md), [use it as a
  library](guides/as-a-library.md).
- **[The SDK preview](sdk/overview.md)** — where "build call behaviour without writing Rust"
  is headed, and what is real today.
- **[Migrating?](migrate/from-kamailio.md)** — coming from Kamailio or Asterisk, what maps
  where.
- **[API reference](https://codewandler.github.io/sipx/api/)** — every crate, generated from
  the source.

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

## Public docs vs project docs

This site is the public documentation for users and integrators: what sipx does, how to use
it, and what it deliberately does not do. The repository also contains internal contributor
material under [`docs/`](https://github.com/codewandler/sipx/tree/main/docs) — design records,
specifications, the story board and the roadmap. Those are useful when contributing, but they
are more detailed and more volatile than this site; when the two disagree, the code and its
tests win, and both trees say so.

## Getting it

The source is on [GitHub](https://github.com/codewandler/sipx), under `MIT OR Apache-2.0`. The
guides here are the shortest path to a working call; every code sample in them is a real file
that CI compiles, because a sample that has rotted is worse than none.
