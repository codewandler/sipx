# Does sipx fit?

The shortest honest answer: **sipx is a phone, not an exchange.** It places and answers calls,
registers, transfers, bridges and conferences. It does not route other people's calls.

## It fits if you want to

- **Place or answer calls from a program** — a dialler, an alerting system, a test harness, a
  voice application.
- **Register against a PBX or carrier** and be reachable.
- **Carry real audio**: G.711 both ways, Opus behind a feature, DTMF, playback and recording.
- **Build on the pieces**: a SIP parser and transaction machines with no async runtime at all,
  or SDP offer/answer as a pure function.
- **Encrypt a call end to end**, signalling and media, without a way to accidentally turn the
  verification off.
- **Notice a far end that vanishes.** RFC 4028 session timers turn "the other phone lost power"
  from a call that stays up forever into one that ends — see [placing a call](place-a-call.md).

## It does not fit if you want

- **A proxy or a registrar.** sipx does not fork requests, insert `Record-Route`, or hold
  registrations for other people. That is a different kind of program.
- **To be reachable through hard NAT.** `rport` and symmetric RTP handle the common case; there
  is no ICE, no Outbound, no Path, no GRUU. A phone behind a symmetric NAT that a server must
  call *into* will not work reliably yet.
- **Browser interoperability.** WebSocket transport works, but browsers require DTLS-SRTP and
  sipx keys with SDES today.
- **Presence, messaging or busy-lamp fields.** The event framework is not built.

## The state of it, precisely

Every claim above is backed by [the compliance table](../compliance.md), which is generated from
a registry and checked in CI — a header it says sipx parses must actually be known to the
parser, and a file it cites must exist. It marks 64 RFCs as implemented, partial, parse-only or
not started, and *partial* entries say which part is missing.

Three things are worth reading there before committing to sipx:

**Media encryption is real but has edges.** SRTP's default transform, keyed by SDES. SDES puts
the key in the SDP body, so an intermediary that terminates the TLS can read it — DTLS-SRTP,
which does not have that property, is not built yet. One transform, no rekeying. RFC 3711 is
marked *partial* for those reasons rather than *implemented*.

**A call with no BYE stays up.** Session timers (RFC 4028) are not implemented, so a far end
that loses power leaves the call established on this side until something else notices.

**Some things parse and do nothing.** `RAck`, `Session-Expires`, `Accept-Contact` and others
survive the wire intact and nothing acts on them. That is deliberate — losslessness first — and
it is recorded as *parse-only* rather than counted as support.

## Where it has been tested

Against **Kamailio**, not only against itself: registration over UDP, TCP, TLS and WebSocket,
plus the refusals that make the successes mean something — a wrong password, a certificate for
another host, an issuer nobody vouches for.

The whole RFC 4475 torture corpus is asserted, including the messages that must be *rejected*
and by which layer.
