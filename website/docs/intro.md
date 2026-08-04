---
title: What sipx is
description: A SIP and VoIP stack in Rust for building programmable phones and call-driven applications.
slug: /
---

# sipx

sipx is a SIP and VoIP stack in Rust for engineers building telephony systems. Use the
`sipx` command for repeatable calls from a shell, or embed the crates for control over
signalling, registration, calls, and media.

The current public-beta release is **`1.0.0-beta.4`**. The latest `main` branch can move ahead of
that release, and this website documents the branch. Public APIs are not frozen before 1.0:
Supported APIs receive migration notes when they break, while Experimental APIs may change or be
removed without one. For a reproducible installation, use the tagged release in
[Getting started](getting-started.md).

## Choose a path

### Try the command line

Place or answer a call, register an address, play and record WAV audio, send DTMF, and emit
machine-readable results:

```bash
cargo install --locked --version =1.0.0-beta.4 sipx-cli
sipx version
```

The CLI is designed for scripts and test calls. It reads and writes WAV files by default; builds
with the optional `device-audio` feature can also open an exact microphone or speaker identifier.
`dial`, `answer`, and `register` select UDP, TCP, TLS, WebSocket, or secure WebSocket, with mandatory
certificate verification on secure paths.

[Make a local call →](getting-started.md)

### Use the Rust libraries

Take only the layer your application needs: the sans-I/O SIP and SDP cores, async transports,
registration, RTP and media, or the complete call framework. The library transport layer
supports UDP, TCP, TLS, WebSocket, and secure WebSocket.

[Choose a crate →](guides/as-a-library.md)

## What ships today

| Area | Available |
|---|---|
| Calls | Place and answer, hold and resume, blind and attended transfer, session timers |
| Registration | Digest authentication, lease refresh, Outbound flows, `Path`, `Service-Route`, GRUU, push-assisted refresh |
| Audio | G.711, DTMF, WAV playback and recording; selectable Opus behind a Cargo feature |
| Media | RTP/RTCP, jitter buffering, quality statistics, ICE, SDES-keyed SRTP, optional DTLS-SRTP |
| Transports | UDP, TCP, TLS, WebSocket, secure WebSocket in the libraries |
| Core | Sans-I/O parsing, transactions, dialogs, and SDP offer/answer |

sipx is a **user agent**, not a proxy, registrar, or configuration-driven PBX. It does not
route calls or store registrations for other endpoints. One narrow
[browser-audio profile](reference/browser-audio-proof.md) composes WSS, ICE, DTLS-SRTP and Opus;
TURN relay, video, and a general browser media stack remain outside the shipped surface. See
[Does sipx fit?](guides/does-this-fit.md) for the boundary and the
[RFC compliance table](reference/compliance.md) for protocol-level detail.

## Application host and contract

The workspace includes the `sipx-host` process. Document-mode webhooks can drive real calls, and
authenticated full-duplex sessions can replace call programs and originate calls. The Rust host
surfaces are Supported under the pre-1.0 policy; the language-neutral `sipx.app.v1` wire contract
remains Experimental. There is no embedded runtime or TypeScript SDK. See the
[Application host overview](sdk/overview.md) for that boundary.

## Design guarantees

- `sipx-sip` and `sipx-sdp` do no I/O: bytes and fired timers are explicit inputs.
- Malformed network input returns typed errors; workspace code forbids `unsafe`.
- Parsed messages preserve header fields that sipx does not interpret.
- Examples in these guides are synchronized with source files compiled by CI.

## Continue

- [Getting started](getting-started.md) — install the tagged CLI and make a local call.
- [Does sipx fit?](guides/does-this-fit.md) — supported roles, limitations, and security edges.
- [How sipx compares](reference/comparison.md) — the same questions asked of other stacks, with the
  evidence for each answer and where sipx loses.
- [Use sipx as a library](guides/as-a-library.md) — Git dependencies, features, and crate choices.
- [Integrate with an existing SIP system](guides/integrate-existing-system.md).
- [How sipx is built](reference/development-process.md) — specifications, evidence, and the release gate.
- [API reference](https://codewandler.github.io/sipx/api/).

Source is available on [GitHub](https://github.com/codewandler/sipx) under `MIT OR Apache-2.0`.
