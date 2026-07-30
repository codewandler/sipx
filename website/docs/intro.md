---
title: What sipx is
description: A SIP and VoIP stack in Rust — a phone as a library you embed or a command you run, with an honest account of what it does not do yet.
slug: /
---

# sipx

A SIP and VoIP stack in Rust. Place and answer calls, register against a PBX, carry real
audio — as a library you embed, or as a command you run.

## Can it do what I need?

Today sipx is a **user agent** — a phone. It calls, answers, registers and transfers. It is **not
a proxy or a registrar**: it does not fork requests or hold other people's registrations.

| | |
|---|---|
| **Calls** | Place and answer, SDP offer/answer, hold and resume, blind and attended transfer, session timers |
| **Audio** | G.711 µ-law and A-law, DTMF, play and record WAV. Opus too, behind `sipx-call`'s `opus` feature: a call offers G.711 unless it selects Opus — see [the edges](#the-honest-version) |
| **Signalling security** | TLS and secure WebSocket, with certificate verification that **cannot be turned off** |
| **Media security** | SRTP, keyed by SDES when the signalling is secure. DTLS-SRTP keying lives in `sipx-sdp` and `sipx-media`; no call and no CLI invocation offers it yet |
| **Transports** | UDP, TCP, TLS, WebSocket, secure WebSocket |
| **Reachability** | NAT via `rport` and symmetric RTP; `Path` and `Service-Route` honoured; RFC 5626 Outbound down a client-opened flow, GRUU, and a binding refreshed on a push. No ICE yet. |
| **Multi-party** | Bridging two media sessions and conferencing several with N−1 mixing live in `sipx-media`; reaching them from a `Call` is being finished |
| **Quality** | Loss, jitter, round-trip time and an estimated MOS, readable mid-call |

It is verified against **two independent peers**, not only against itself — a proxy (Kamailio) and
a PBX and back-to-back user agent on an unrelated SIP library (Asterisk). Every peer runs the same
list: registration over UDP, TCP, TLS and WebSocket, and the refusals that make the successes mean
something. The one that answers calls also places and answers them with sipx, with SDES-keyed SRTP
on the media.

## The honest version

Four things are worth knowing before you decide. The last three are one shape: a capability that
is real in a crate and cannot be reached from a call, which is the distinction the [compliance
table](reference/compliance.md) draws per RFC and this page had been blurring.

**It is a phone, not an exchange.** If you need something that routes other people's calls,
sipx is not that yet, and the [compliance table](reference/compliance.md) says so per RFC
rather than leaving you to find out.

**The encryption has edges.** Media is encrypted when the signalling is — `sips:` or WSS —
using SRTP's default transform, keyed by SDES. That puts the key in the SDP body, so any
intermediary that terminates the TLS can read it. (The `sipx` binary cannot get there at all: it
takes no flag for a secure transport, so nothing you type at it produces encrypted media — see
[the CLI reference](reference/cli.md).) DTLS-SRTP
(RFC 5763 and RFC 5764) keys on the media path instead and does not have that property, and
everything those two RFCs decide about certificates and fingerprints is implemented — but **no
media session can be keyed by DTLS today, by any route.** The handshake returns finished SRTP
contexts and a media session is configured with master keys and salts; the two do not meet. Nor
can the handshake run on the media port RFC 5764 §5.1.2 requires it to share, because a media
port does not lend out its socket. So there is no arrangement of `sipx-sdp` and `sipx-media` that
gets you a DTLS-keyed call, and writing your own capabilities does not either; the work is
tracked as `M-28`. What is also left is one transform and no rekeying, which is why [the
table](reference/compliance.md) marks RFC 3711 *partial* rather than implemented.

**Opus is on offer, and off by default.** `M-30` built the selection: `DialOptions::with_codecs`
on the offering side, `answer_with` and the other `_with` entry points on the answering side, and
`Codecs::Opus` reaches both. It is behind `sipx-call`'s `opus` feature because Opus links a C
library, and the default codec set stays the G.711 pair — mandatory-to-implement (RFC 3551
§4.5.14), pure Rust, and accepted by practically every endpoint. So the better codec is always a
choice you made and never one your build made for you, and an offer that selects it still carries
G.711 alongside: an endpoint offering only Opus would fail to call most of the telephone network.

What has *not* changed is how a payload type is read. A dynamic number means whatever `a=rtpmap`
said, so nothing guesses Opus from 111 — a format is matched by its rtpmap, and the number the
far end assigned travels with the codec rather than being reassumed. That is also why selecting
Opus cannot pull it in through the back door: negotiation refuses to settle on a codec outside
the set you selected, so an Opus offer arriving at a G.711 call is answered G.711.

**Bridging and conferencing are library pieces, not call operations.** `sipx-media` has both, and
they take a shared media session; a `Call` owns its media outright and lends only a reference, so
there is no way to hand two calls to a bridge. Connecting them is `C-6`. Two `MediaSession`s you
built yourself can be bridged today.

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
