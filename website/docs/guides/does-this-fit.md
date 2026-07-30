---
title: Does sipx fit?
description: Where sipx fits today, what is shipped, and which telephony roles and media paths are not available yet.
---

# Does sipx fit?

The short answer: sipx is a programmable **SIP user agent**. It is a strong fit for an endpoint,
test client, dialler, or voice application. It is not a proxy, registrar, or PBX.

## It fits when you need

- **Calls from Rust or a shell.** Place and answer calls, send DTMF, hold, resume, transfer,
  play audio, record audio, and read quality statistics.
- **A registered endpoint.** Digest authentication, automatic lease refresh, `Path`,
  `Service-Route`, GRUU, RFC 5626 Outbound on a client-opened flow, and push-assisted binding
  refresh are available.
- **Telephony audio.** G.711 µ-law and A-law are the default. Opus is selectable through
  `sipx-call`'s `opus` feature and links a C library.
- **SIP building blocks.** Use the parser, transaction and dialog machines, or SDP offer/answer
  without bringing in an async runtime, socket, or clock.
- **Secure transports.** The transport layer and diagnostic CLI support TLS and secure WebSocket
  with certificate verification. Calls over secure signalling can use SDES-keyed SRTP.
- **A scriptable test endpoint.** The CLI emits JSON and distinct exit codes, and moves audio
  through WAV files for repeatable automation.

## Choose something else when you need

- **Proxy, registrar, or PBX behavior.** sipx does not fork or route other users' requests,
  add itself to a route set, store registrations for other endpoints, or provide dial plans.
- **A desktop phone.** The CLI does not access a microphone, speaker, headset, or sound-device
  mixer. It plays and records 8 kHz, 16-bit, mono WAV files.
- **ICE or a general NAT traversal service.** `rport`, symmetric RTP, and Outbound cover many
  registered-endpoint cases, but sipx does not perform ICE connectivity checks or provide a
  relay. Some NAT topologies will have no working media path.
- **A browser media endpoint.** WebSocket signalling alone is insufficient: the current call
  path has neither ICE nor DTLS-keyed media, so browser interoperability is not a shipped use case.
- **Video or additional codecs.** The media stack is for telephony audio. Calls support G.711
  and optional Opus, not arbitrary application-supplied codecs.
- **Bridging two `Call` values.** `sipx-media` can bridge or conference media sessions that you
  own, but a `Call` owns its media session and cannot currently be handed to those operations.
- **Automatic presence from live stack state.** Subscription and event-package components are
  present, but applications must supply the documents they publish; live calls and registrations
  are not automatically projected into presence state.
- **SIP instant messaging.** `MESSAGE` can be parsed but has no user-agent behavior.

## Security boundary

TLS protects each signalling hop, not necessarily every intermediary. With SDES, SRTP key
material is carried in SDP, so any intermediary terminating that secure signalling can read it.
The DTLS fingerprint, certificate-checking, and handshake components exist in the SDP and media
crates, but they are not connected to a media session or call today. There is no supported route
to a DTLS-keyed call, and SRTP currently has one transform with no rekeying.

See [Security](../reference/security.md) for the CLI-versus-library matrix and
[RFC compliance](../reference/compliance.md) for the checked, protocol-by-protocol status.

## Application host status

The experimental `sipx-host` binary exists. It reads configuration, binds a listener, answers
a real call, and follows the configured policy for an unreachable application. None of the
external or embedded application callback bindings is implemented, so handler programs cannot
drive calls yet. Do not select it when application callbacks are a requirement.

## Make the decision

If you need a programmable endpoint and the limits above fit, start with
[Getting started](../getting-started.md) or [choose a crate](as-a-library.md). If sipx will join
an existing deployment, first map the user-agent, proxy, registrar, and application roles in
[Integrate with an existing SIP system](integrate-existing-system.md).
