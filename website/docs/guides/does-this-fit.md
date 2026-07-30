---
title: Does sipx fit?
description: The honest answer — sipx is a phone, not an exchange. What it does, what it deliberately does not, and where every claim is measured.
---

# Does sipx fit?

The shortest honest answer: **sipx is a phone, not an exchange.** It places and answers calls,
registers and transfers. It does not route other people's calls.

## It fits if you want to

- **Place or answer calls from a program** — a dialler, an alerting system, a test harness, a
  voice application.
- **Register against a PBX or carrier** and be reachable — including from behind NAT, down a
  flow the client opened (RFC 5626 Outbound), with `Path` and `Service-Route` honoured, with a
  GRUU for one instance of the registration, and with a binding refreshed when a push wakes the
  client.
- **Carry real audio**: G.711 both ways, DTMF, playback and recording. Opus too, behind
  `sipx-call`'s `opus` feature — a call offers G.711 unless it selects Opus.
- **Build on the pieces**: a SIP parser and transaction machines with no async runtime at all,
  or SDP offer/answer as a pure function.
- **Encrypt a call end to end**, signalling and media, without a way to accidentally turn the
  verification off.
- **Notice a far end that vanishes.** RFC 4028 session timers turn "the other phone lost power"
  from a call that stays up forever into one that ends — see [placing a call](place-a-call.md).
- **Serve subscriptions.** A notifier with a subscription store and packages registered by name
  (RFC 6665), with `dialog`, `reg` and `presence` documents and PUBLISH behind an entity tag.
  You supply what the documents describe; see the caveat below.

## It does not fit if you want

- **A proxy or a registrar.** sipx does not fork requests, insert `Record-Route`, or hold
  registrations for other people. That is a different kind of program — see
  [migrating from Kamailio](../migrate/from-kamailio.md) for where those roles live.
- **Media through NAT that symmetric RTP cannot solve.** `rport`, symmetric RTP and Outbound
  cover the registered-phone case; there is no ICE yet, so the paths only a relay or
  connectivity checks would fix are not fixed.
- **A codec beyond G.711 and Opus.** Those two are what `sipx-audio` implements, and `Codecs` is
  the whole of the choice — there is no G.722, no G.729, and no way to hand a call a codec sipx
  does not have. Opus needs `sipx-call`'s `opus` feature, which links a C library; G.711 is the
  default and needs nothing.
- **Bridging or conferencing two *calls*.** `sipx-media` implements both, over media sessions you
  hold. A `Call` owns its media session outright and lends only a reference, so two calls cannot
  be handed to a bridge — that is `C-6`, and the [migration
  notes](../migrate/from-asterisk.md) mark it in progress rather than done.
- **Browser interoperability.** WebSocket transport works, so one of the two pieces browsers
  insist on is there. ICE is not, and neither is a DTLS-keyed media session — see the edges
  below — so a browser and sipx will agree on a session and then fail to find a media path in
  most networks.
- **Presence or busy-lamp fields as a finished feature.** The event framework is built (RFC
  6665), and so are the `dialog`, `reg` and `presence` packages with PIDF and PUBLISH — but the
  packages produce documents, and joining them to sipx's live dialogs and registrations is
  yours to write. A watcher gets what you publish to it, not an automatic view of what the
  stack is doing.
- **Messaging.** `MESSAGE` parses and nothing acts on it.

## The state of it, precisely

Every claim above is backed by [the compliance table](../reference/compliance.md), which is
generated from a registry and checked in CI — a header it says sipx parses must actually be
known to the parser, and a file it cites must exist. It marks 70 RFCs as implemented, partial,
parse-only or not started, and *partial* entries say which part is missing.

Two things are worth reading there before committing to sipx:

**Media encryption is real but has edges.** SRTP's default transform, keyed by SDES. SDES puts the
key in the SDP body, so an intermediary that terminates the TLS can read it. DTLS-SRTP keys on the
media path and does not have that property, and RFC 5763 and RFC 5764 are implemented as far as
certificates and fingerprints go — but **no media session can be keyed by DTLS today, by any
route**: the handshake produces finished SRTP contexts while a media session is configured with
master keys and salts, and the handshake cannot run on the media port §5.1.2 requires it to share.
That is `M-28`, not a switch to flip. What is also left is one transform and no rekeying, which is
why RFC 3711 is marked *partial* rather than *implemented*.

**Some things parse and do nothing.** `Accept-Contact` and others survive the wire intact and
nothing acts on them. That is deliberate — losslessness first — and it is recorded as
*parse-only* rather than counted as support.

## Where it has been tested

Against **two independent peers**, not only against itself — a proxy (Kamailio) and a PBX and
back-to-back user agent on an unrelated SIP library (Asterisk). One peer is one more reading of
the RFCs, not a consensus, which is why there are two.

Every peer runs the same list: registration over UDP, TCP, TLS and WebSocket, plus the refusals
that make the successes mean something — a wrong password, a certificate for another host, an
issuer nobody vouches for. The peer that answers calls also places and answers them with sipx,
with audio flowing and a BYE ending it, and does it again with SDES-keyed SRTP on the media.

The whole RFC 4475 torture corpus is asserted, including the messages that must be *rejected*
and by which layer.
