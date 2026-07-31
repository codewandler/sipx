---
title: What sipx is
description: A SIP and VoIP stack in Rust for building programmable phones and call-driven applications.
slug: /
---

# sipx

sipx is a SIP and VoIP stack in Rust for engineers building telephony systems. Use the
`sipx` command for repeatable calls from a shell, or embed the crates for control over
signalling, registration, calls, and media.

The current release is **`1.0.0-alpha.4`**. The API is still allowed to change before 1.0,
and this website documents the latest `main` branch. For a reproducible installation, use
the tagged release in [Getting started](getting-started.md).

## Choose a path

### Try the command line

Place or answer a call, register an address, play and record WAV audio, send DTMF, and emit
machine-readable results:

```bash
cargo install --git https://github.com/codewandler/sipx \
  --tag v1.0.0-alpha.4 --locked sipx-cli
sipx version
```

The CLI is designed for scripts and test calls. It reads and writes WAV files; it does **not**
open a microphone, speaker, or other sound device. `dial`, `answer`, and `register` select UDP,
TCP, TLS, WebSocket, or secure WebSocket, with mandatory certificate verification on secure paths.

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
| Media | RTP/RTCP, jitter buffering, quality statistics, symmetric RTP, SDES-keyed SRTP |
| Transports | UDP, TCP, TLS, WebSocket, secure WebSocket in the libraries |
| Core | Sans-I/O parsing, transactions, dialogs, and SDP offer/answer |

sipx is a **user agent**, not a proxy, registrar, or configuration-driven PBX. It does not
route calls or store registrations for other endpoints. ICE, video, and a complete browser
media path are also outside the current shipped surface. See [Does sipx fit?](guides/does-this-fit.md)
for the boundary and the [RFC compliance table](reference/compliance.md) for protocol-level detail.

## Experimental application host

The workspace includes the `sipx-host` process. It can read a host configuration, bind a SIP
listener, answer a real call, and apply the configured unreachable-app policy. The external and
embedded application callback bindings are not implemented yet, so customer handler code cannot
drive those calls. Treat the host and its `sipx.app.v1` contract as experimental.

## Design guarantees

- `sipx-sip` and `sipx-sdp` do no I/O: bytes and fired timers are explicit inputs.
- Malformed network input returns typed errors; workspace code forbids `unsafe`.
- Parsed messages preserve header fields that sipx does not interpret.
- Examples in these guides are synchronized with source files compiled by CI.

## Continue

- [Getting started](getting-started.md) — install the tagged CLI and make a local call.
- [Does sipx fit?](guides/does-this-fit.md) — supported roles, limitations, and security edges.
- [Use sipx as a library](guides/as-a-library.md) — Git dependencies, features, and crate choices.
- [Integrate with an existing SIP system](guides/integrate-existing-system.md).
- [API reference](https://codewandler.github.io/sipx/api/).

Source is available on [GitHub](https://github.com/codewandler/sipx) under `MIT OR Apache-2.0`.
