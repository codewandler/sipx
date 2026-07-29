---
title: What's new
description: The current state of sipx, release by release — and the full engineering changelog for the detail.
---

# What's new

sipx is **pre-1.0**: this site tracks `main`, and the
[changelog](https://github.com/codewandler/sipx/blob/main/CHANGELOG.md) carries the full
engineering history, story by story.

## Where it stands

The stack places and receives calls with encrypted media between its own endpoints, and
registers against a real third-party registrar (Kamailio) over UDP, TCP, TLS and WebSocket.
The CLI dials, answers and registers from a shell with JSON output and per-outcome exit codes.

Recently landed:

- **Outbound (RFC 5626)** — registration down a flow the client opened, with `reg-id`,
  `+sip.instance`, and keep-alives on the flow being tested, so NAT bindings that lapse stop
  being fatal.
- **Service-Route (RFC 3608)** — requests sipx sends now follow the route set the registrar
  handed back.
- **Path (RFC 3608's inbound twin, RFC 3327)** — registration through a proxy chain that needs
  to route back down the way it came.
- **Reliable provisionals (RFC 3262)** — 100rel and PRACK, so "it is ringing" survives a lossy
  network.
- **Session timers (RFC 4028)** — a far end that vanishes ends the call instead of leaving it
  up forever.
- **SRTP with SDES keying (RFC 3711, RFC 4568)** — media encrypted when the signalling
  protects the key, with the [edges documented honestly](reference/compliance.md).
- **DTLS-SRTP (RFC 5763, RFC 5764)** — the keying that never touches the signalling path: the
  handshake runs over the media path and the certificate is checked against the
  `a=fingerprint` the SDP carried, or no keys are returned. Everything the two RFCs decide is
  compiled always; only the handshake sits behind the off-by-default `dtls` feature.
- **The event notification framework (RFC 6665)** — a notifier with a subscription store and
  packages registered by name, plus the `dialog`, `reg` and `presence` packages and PUBLISH
  behind an entity tag. The packages produce documents; joining them to sipx's live dialogs and
  registrations is still yours to write.
- **A call reports itself as a typed event stream** — ringing, answered, a DTMF digit and how
  long it was held, playback and recording finishing, transfer progress, hold and resume, and
  ended with a cause, pushed onto a channel the call owns instead of found by polling it.

Not there yet: ICE. The two pieces a browser insists on — WebSocket transport and DTLS-SRTP —
are in place, but without connectivity checks a browser and sipx will agree on a session and
then fail to find a media path in most networks.

## The SDK direction

The `sipx.app.v1` contract — call events out, instructions in, so call behaviour can be built
without writing Rust — is now [specified](sdk/overview.md) and experimental. The kernel work it
needs is designed and tracked, and the host has its home in the workspace: `crates/sipx-app`.
