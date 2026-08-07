---
title: Does sipx fit?
description: Where sipx fits today, what is shipped, and which telephony roles and media paths are not available yet.
---

# Does sipx fit?

The short answer: sipx is a programmable **SIP user agent**. It is a strong fit for an endpoint,
test client, dialler, or voice application. It is not a proxy, registrar, or PBX.

## It fits when you need

- **Calls from Rust or a shell.** [Place](place-a-call.md) and [answer](answer-a-call.md) calls,
  [send DTMF](send-and-collect-dtmf.md), [hold and resume](hold-and-resume.md), transfer
  [blind](blind-transfer.md) or [attended](attended-transfer.md), [play audio](play-audio.md),
  [record audio](record-a-call.md), and read quality statistics.
- **A registered endpoint.** Digest authentication, automatic lease refresh, `Path`,
  `Service-Route`, GRUU, RFC 5626 Outbound on a client-opened flow, and push-assisted binding
  refresh are available — see [Register against a PBX](register.md).
- **Telephony audio.** G.711 µ-law and A-law are the default. Mono L16 is selectable at its static
  44.1 kHz assignment or dynamic 8 kHz mapping. Opus is selectable through `sipx-call`'s `opus`
  feature and links a C library.
- **SIP building blocks.** Use the parser, transaction and dialog machines, or SDP offer/answer
  without bringing in an async runtime, socket, or clock — see
  [Use sipx as a library](as-a-library.md).
- **Secure transports.** The transport layer and diagnostic CLI support TLS and secure WebSocket
  with certificate verification. Calls can select plain RTP, SDES-keyed SRTP, or DTLS-SRTP without
  falling back to a weaker mode.
- **A scriptable test endpoint.** The [CLI](../reference/cli.md) emits JSON and distinct exit
  codes, and moves audio through WAV files for repeatable automation. Its optional `device-audio`
  feature also opens an explicitly selected microphone or speaker through a bounded leaf-only
  driver.
- **NAT traversal without a relay.** Calls can gather host candidates and use a configured STUN
  server to select a server-reflexive ICE path.
- **A two-leg call controller.** `sipx-call::EarlyCoupling` and `Coupling` own both dialogs, relay
  offer/answer changes and termination, and can attach the bounded media bridge — see
  [Couple two calls](couple-two-calls.md).
- **Managed transport endpoints.** New TLS and secure-WebSocket handshakes can select an atomically
  replaced server identity while established connections survive. Bounded observation, immutable
  pre-transaction request policy, and live source-prefix admission let a host inspect or constrain
  the endpoint without moving arbitrary mutation into the transport driver.
- **SIP event services in both roles.** A dispatcher can serve bounded `dialog`, `reg`, and
  `presence` subscriptions; a generic authenticated client maintains outbound subscriptions; and
  conditional presence publication is available in both roles. Package bodies, authorization,
  projection and durable state remain application policy.
- **Registration discovery from an existing registrar.** The generic subscriber has a bounded
  RFC 3680 consumer, and `sipx peers --registrar` keeps a current contact view with explicit source
  and age. Enumeration still depends on the registrar authorizing the subscription.
- **Application-owned requests inside a call.** INFO, MESSAGE, and explicitly admitted private
  methods can be sent and answered on an established dialog while the call state machine retains
  ownership of negotiation, transfer, capability and teardown methods.

## Choose something else when you need

Each of these is a boundary of what sipx does, not a judgement about what else exists. For what the
alternatives actually offer against sipx — measured at a pinned version, with the evidence for every
claim and the places sipx loses stated plainly — see [How sipx compares](../reference/comparison.md).

- **Proxy, registrar, or PBX behavior.** sipx does not fork or route other users' requests,
  add itself to a route set, store registrations for other endpoints, or provide dial plans.
- **A desktop phone interface.** The optional device driver can open an exact microphone or speaker,
  but sipx has no graphical call controls, headset integration, or sound-device mixer.
- **A general NAT traversal service.** ICE connectivity checks and STUN-derived server-reflexive
  candidates are available, but TURN and relayed candidates are not. Some NAT pairs therefore have
  no working media path.
- **A general browser media endpoint.** sipx has one named, fail-closed browser-audio composition
  over WSS, ICE, DTLS-SRTP and Opus. It deliberately does not ship browser APIs, video, data
  channels, multiple media sections, incremental candidate trickling, TURN, or the complete WebRTC
  protocol surface. The [native-browser proof](../reference/browser-audio-proof.md) covers that
  exact profile in both SIP roles; selecting the profile alone is not an interoperability claim.
- **Video or additional codecs.** The media stack is for telephony audio. Calls support G.711,
  mono L16 and optional Opus, not arbitrary application-supplied codecs.
- **A ready-made routing product.** The two-dialog coupling primitive is available, but listener
  configuration, routing policy, a location service, and dial plans belong to the application.
- **Automatic event documents from live stack state.** The socket notifier sends valid initial
  `dialog`, `reg`, and PIDF documents, but live calls, registrations and published presence are not
  yet projected into later NOTIFY bodies. A bounded generic subscriber does originate authenticated
  SUBSCRIBE and consume NOTIFY through an injected package parser and origin policy; registration
  discovery has a built-in consumer, while dialog and presence consumers remain application policy.
- **A ready-made instant-messaging service.** The call API can send and answer bounded MESSAGE
  requests inside an established dialog, but sipx does not provide an out-of-dialog messaging
  client, message store, delivery policy, or user-facing chat product.

## Security boundary

TLS protects each signalling hop, not necessarily every intermediary. With SDES, SRTP key
material is carried in SDP, so any intermediary terminating that secure signalling can read it.
DTLS-SRTP keeps that media key out of signalling and is selectable through both the call API and
CLI, but it still has one SRTP transform with no rekeying. The browser-audio profile composes ICE
and DTLS-SRTP under its stricter policy; unsupported combinations are refused rather than silently
downgraded.

See [Security](../reference/security.md) for the CLI-versus-library matrix,
[RFC compliance](../reference/compliance.md) for the checked, protocol-by-protocol status, and
[How sipx compares](../reference/comparison.md) for the same questions asked of other stacks.

## Application host status

The `sipx-host` binary reads configuration, binds listeners, and serves real calls to document-mode
webhooks, authenticated full-duplex sessions, or a configured realtime audio binding. A granted
session can originate a call; a realtime binding carries one routed G.711 call to one authenticated
WebSocket session. The Rust host surfaces are Supported under the pre-1.0 policy, while the
language-neutral `sipx.app.v1` wire contract remains Experimental. The deterministic peer proof is
part of the default test matrix, but the credentialed live-endpoint interoperability proof has not
yet been recorded. There is no embedded runtime or TypeScript SDK, so do not select it when either
is a requirement. The [application host overview](../sdk/overview.md) gives the binding and trust
boundaries.

## Make the decision

If you need a programmable endpoint and the limits above fit, start with
[Getting started](../getting-started.md) or [choose a crate](as-a-library.md). If sipx will join
an existing deployment, first map the user-agent, proxy, registrar, and application roles in
[Integrate with an existing SIP system](integrate-existing-system.md).
