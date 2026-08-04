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
  with certificate verification. Calls can select plain RTP, SDES-keyed SRTP, or DTLS-SRTP without
  falling back to a weaker mode.
- **A scriptable test endpoint.** The CLI emits JSON and distinct exit codes, and moves audio
  through WAV files for repeatable automation. Its optional `device-audio` feature also opens an
  explicitly selected microphone or speaker through a bounded leaf-only driver.
- **NAT traversal without a relay.** Calls can gather host candidates and use a configured STUN
  server to select a server-reflexive ICE path.
- **A two-leg call controller.** `sipx-call::EarlyCoupling` and `Coupling` own both dialogs, relay
  offer/answer changes and termination, and can attach the bounded media bridge.

## Choose something else when you need

- **Proxy, registrar, or PBX behavior.** sipx does not fork or route other users' requests,
  add itself to a route set, store registrations for other endpoints, or provide dial plans.
- **A desktop phone interface.** The optional device driver can open an exact microphone or speaker,
  but sipx has no graphical call controls, headset integration, or sound-device mixer.
- **A general NAT traversal service.** ICE connectivity checks and STUN-derived server-reflexive
  candidates are available, but TURN and relayed candidates are not. Some NAT pairs therefore have
  no working media path.
- **A browser media endpoint.** Secure WebSocket signalling, ICE, and DTLS-keyed media are useful
  pieces, but sipx deliberately does not ship the complete browser media protocol surface.
- **Video or additional codecs.** The media stack is for telephony audio. Calls support G.711
  and optional Opus, not arbitrary application-supplied codecs.
- **A ready-made routing product.** The two-dialog coupling primitive is available, but listener
  configuration, routing policy, a location service, and dial plans belong to the application.
- **Automatic presence from live stack state.** Subscription and event-package components are
  present, but applications must supply the documents they publish; live calls and registrations
  are not automatically projected into presence state.
- **SIP instant messaging.** `MESSAGE` can be parsed but has no user-agent behavior.

## Security boundary

TLS protects each signalling hop, not necessarily every intermediary. With SDES, SRTP key
material is carried in SDP, so any intermediary terminating that secure signalling can read it.
DTLS-SRTP keeps that media key out of signalling and is selectable through both the call API and
CLI, but it still has one SRTP transform with no rekeying. Reliable early media and ICE combined
with DTLS-SRTP are refused rather than silently downgraded.

See [Security](../reference/security.md) for the CLI-versus-library matrix and
[RFC compliance](../reference/compliance.md) for the checked, protocol-by-protocol status.

## Application host status

The `sipx-host` binary reads configuration, binds listeners, and serves real calls to document-mode
webhooks or authenticated full-duplex sessions. A granted session can originate a call. The Rust
host surfaces are Supported under the pre-1.0 policy, while the language-neutral `sipx.app.v1` wire
contract remains Experimental. There is no embedded runtime or TypeScript SDK, so do not select it
when either is a requirement. The [application host overview](../sdk/overview.md) gives the binding
and trust boundaries.

## Make the decision

If you need a programmable endpoint and the limits above fit, start with
[Getting started](../getting-started.md) or [choose a crate](as-a-library.md). If sipx will join
an existing deployment, first map the user-agent, proxy, registrar, and application roles in
[Integrate with an existing SIP system](integrate-existing-system.md).
