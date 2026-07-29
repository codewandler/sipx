---
title: Does sipx fit?
description: The honest answer — sipx is a phone, not an exchange. What it does, what it deliberately does not, and where every claim is measured.
---

# Does sipx fit?

The shortest honest answer: **sipx is a phone, not an exchange.** It places and answers calls,
registers, transfers, bridges and conferences. It does not route other people's calls.

## It fits if you want to

- **Place or answer calls from a program** — a dialler, an alerting system, a test harness, a
  voice application.
- **Register against a PBX or carrier** and be reachable — including from behind NAT, down a
  flow the client opened (RFC 5626 Outbound), with `Path` and `Service-Route` honoured.
- **Carry real audio**: G.711 both ways, Opus behind a feature, DTMF, playback and recording.
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
  connectivity checks would fix are not fixed. No GRUU and no push either — one instance of a
  registration cannot be addressed individually, and a sleeping client cannot be woken.
- **Browser interoperability.** WebSocket transport works and DTLS-SRTP keys the media, so the
  two pieces browsers insist on are there — but ICE is not, and without it a browser and sipx
  will agree on a session and then fail to find a media path in most networks.
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

**Media encryption is real but has edges.** SRTP's default transform, keyed either by SDES or by
DTLS-SRTP. SDES puts the key in the SDP body, so an intermediary that terminates the TLS can
read it; DTLS-SRTP keys on the media path and does not have that property, with its handshake
behind the off-by-default `dtls` feature. What is left is one transform and no rekeying, which
is why RFC 3711 is marked *partial* rather than *implemented*.

**Some things parse and do nothing.** `Accept-Contact` and others survive the wire intact and
nothing acts on them. That is deliberate — losslessness first — and it is recorded as
*parse-only* rather than counted as support.

## Where it has been tested

Against **Kamailio**, not only against itself: registration over UDP, TCP, TLS and WebSocket,
plus the refusals that make the successes mean something — a wrong password, a certificate for
another host, an issuer nobody vouches for.

The whole RFC 4475 torture corpus is asserted, including the messages that must be *rejected*
and by which layer.
