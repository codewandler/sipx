---
title: Use sipx as a library
description: The crates are useful separately — a parser and transaction machines with no async runtime, offer/answer as a pure function, or the whole call framework.
---

# Use sipx as a library

The crates are useful separately. Take the pure protocol core, the async transport and user-agent
layers, or the complete call framework. The current public beta is on crates.io; `main` can move
ahead of it.

## Stability policy

Public APIs are not frozen before 1.0. Each crate-level API page labels its surface Supported or
Experimental. Breaking Supported APIs receive a changelog entry and migration guidance;
Experimental APIs may change shape or be removed without a migration note. The labels describe
support intent, not semantic-version stability.

## Add a dependency

Pin the exact public beta in `Cargo.toml` so every sipx crate resolves to the same release:

```toml
[dependencies]
sipx-call = "=1.0.0-beta.4"
```

This website documents `main`, which can be tested explicitly with:

```toml
[dependencies]
sipx-call = { git = "https://github.com/codewandler/sipx", branch = "main" }
```

Commit `Cargo.lock` for applications and binaries so the selected Git revision is reproducible.
For a library, use the crates.io dependency unless you intentionally require unreleased work.

Optional behavior is opt-in. For example, make Opus selectable by `sipx-call` like this:

```toml
[dependencies]
sipx-call = { version = "=1.0.0-beta.4", features = ["opus"] }
```

Opus links a C library. Enabling the feature makes `Codecs::Opus` available; it does not silently
change the codecs a call offers. G.711 remains the default selection.

The named browser-audio call policy needs both optional media boundaries and still has to be
selected explicitly:

```toml
[dependencies]
sipx-call = { version = "=1.0.0-beta.4", features = ["opus", "dtls"] }
```

### Opus packaging policy

Opus is off by default in `sipx-audio`, `sipx-media`, `sipx-call`, and `sipx-cli`; no shipped
application enables it implicitly. Selecting it brings in the native `libopus` boundary through the
optional `opus` and `audiopus_sys` Rust packages, so a deployment needs the native library available
or must deliberately accept building it from source.

The Rust packages carry MIT OR Apache-2.0 licensing and remain inside the workspace's permissive
Cargo licence policy. Native-library distribution is a separate packaging boundary and should be
reviewed for the target platform. The unmaintained `audiopus_sys` advisory RUSTSEC-2026-0150 is the
one narrow exception in `deny.toml`: CI uses a system `libopus` through `pkg-config`, avoiding the
advisory's source-build failure, and the exception ends when a maintained encoding-capable binding
is available or the opt-in codec no longer justifies it. This is why enabling Opus is an explicit
deployment decision rather than a default.

The current implementation uses the mandatory `opus/48000/2` RTP mapping with mono audio. It does
not advertise or apply optional RFC 7587 `fmtp` controls such as bitrate limits, in-band FEC, DTX,
CBR, or stereo preferences.

## Parse without a runtime

`sipx-sip` and `sipx-sdp` depend on **no async runtime at all**. Use them for a parser,
transaction state machines, or offer/answer as pure logic without taking a socket layer.

The example below is a real file that CI compiles
([`crates/sipx-sip/examples/parse_a_message.rs`](https://github.com/codewandler/sipx/blob/main/crates/sipx-sip/examples/parse_a_message.rs)):

<!-- BEGIN generated:example crates/sipx-sip/examples/parse_a_message.rs -->
```rust
//! Parse a SIP message and read its headers, with no runtime and no sockets.
//!
//! `sipx-sip` is usable entirely on its own — this example depends on no async runtime at all.
//!
//! ```text
//! cargo run --example parse_a_message
//! ```

// These samples are read by people before they are run by machines, so they are written for
// readability where the workspace lints would prefer something terser. `clone_into` over
// `to_owned` teaches nothing in a five-line example, and a sine wave has to become an `i16`
// somewhere.
#![allow(clippy::assigning_clones, clippy::cast_possible_truncation)]

use bytes::Bytes;
use sipx_sip::{HeaderName, Limits, Message, parse_datagram};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let wire = b"INVITE sip:bob@example.com SIP/2.0\r\n\
        Via: SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bK776asdhds\r\n\
        Max-Forwards: 70\r\n\
        To: Bob <sip:bob@example.com>\r\n\
        From: Alice <sip:alice@example.net>;tag=1928301774\r\n\
        Call-ID: a84b4c76e66710@example.net\r\n\
        CSeq: 314159 INVITE\r\n\
        X-Vendor-Thing: preserved verbatim\r\n\
        Content-Length: 0\r\n\r\n";

    let message = parse_datagram(Bytes::from_static(wire), &Limits::datagram())?;

    let Message::Request(request) = message else {
        return Err("expected a request".into());
    };
    println!("{:?} {}", request.method, request.uri);

    // Typed access is lazy: the header is parsed when it is asked for, not when the message is.
    let via = request.headers.typed::<sipx_sip::headers::Via>();
    if let Some(Ok(via)) = via {
        println!("came from {}", via.host);
    }

    // A header sipx has no behaviour for still survives intact. That is why "parse-only" is a
    // status in the compliance table rather than a gap in it.
    if let Some(value) = request
        .headers
        .value(&HeaderName::Other("X-Vendor-Thing".into()))
    {
        println!("unknown header kept: {}", String::from_utf8_lossy(&value));
    }

    // And it re-serializes byte for byte.
    assert_eq!(
        sipx_sip::Message::Request(request).to_bytes().as_ref(),
        wire.as_slice()
    );
    println!("round-tripped byte for byte");
    Ok(())
}
```
<!-- END generated:example -->

The message borrows its input bytes, preserves headers it does not interpret, and returns typed
parse errors. Transaction time enters as a fired-timer input and leaves as a set-timer output.
Those properties make the core deterministic to drive from another runtime or a unit test.

## Which crate

| You want | Take |
|---|---|
| Parse and build SIP messages, transactions, dialogs | `sipx-sip` |
| SDP and offer/answer | `sipx-sdp` |
| Sockets, TLS, WebSocket, RFC 3263 resolution | `sipx-transport` |
| Registration and digest authentication | `sipx-ua` |
| RTP, RTCP, jitter buffer, quality statistics, SRTP | `sipx-rtp` |
| G.711 (µ-law and A-law), mixing, WAV, and Opus behind the `opus` feature | `sipx-audio` |
| RTP/RTCP sockets bound to negotiated SDP with NAT handling, bridging, conferencing | `sipx-media` |
| Calls with playback, recording, DTMF, transfer, and confirmed-dialog snapshots | `sipx-call` |
| Socket-free call signalling tests with seeded faults and virtual time | `sipx-testkit` |
| A phone to run rather than embed — the `sipx` binary | `sipx-cli` |
| The `sipx.app.v1` contract: its types, wire format and interpreter | `sipx-app-protocol` |
| The application host, webhook/session bindings, and deterministic contract harness | `sipx-app` |

`sipx-app` includes a `sipx-host` process that serves real calls to document-mode webhooks or
authenticated full-duplex sessions. A granted session can originate calls. The Rust host surfaces
are Supported under the policy above; the `sipx.app.v1` wire line remains Experimental, and no
embedded runtime or TypeScript SDK is shipped.

## Runtime and feature boundaries

- `sipx-sip` and `sipx-sdp` are sans-I/O and have no async runtime.
- `sipx-testkit::call::CallHarness` asynchronously drives the real call API over socket-free SIP
  signalling; `TransactionHarness` is the seeded, nanosecond virtual-time surface whose clock
  advances only when a test asks.
- `sipx-transport`, `sipx-ua`, `sipx-media`, and `sipx-call` use Tokio for I/O-facing work.
- `sipx-transport` enables UDP, TCP, DNS, TLS, WebSocket, secure WebSocket, and the Experimental
  SIP-over-QUIC mapping by default. Use `default-features = false` with an explicit feature list
  for a smaller transport build.
- `sipx-ua` enables its `runtime` feature by default. Disable defaults only when using its
  authentication and other non-runtime primitives without the transport-backed user agent.
- `sipx-media` has no default features. `opus` links the optional codec library; `dtls` links the
  optional handshake backend. Its DTLS components can key a media session through explicit policy.
- `sipx-call` exposes optional `opus` and `dtls` features. The default call codec set is G.711, and
  selecting DTLS-SRTP is always explicit.

The call crate can persist a confirmed, quiescent dialog without serializing its runtime. See
[Persist and restore a confirmed dialog](persist-a-dialog.md) for the format, fresh-driver
attachment contract, and the host responsibilities that deliberately remain outside the library.

## The API reference

Generated from the source with `cargo doc`, and published beside this site rather than only on
docs.rs so that it always matches the guides next to it. Start points:

| Crate | Start at |
|---|---|
| `sipx-sip` | [`parser`](https://codewandler.github.io/sipx/api/sipx_sip/parser/index.html) · [`transaction`](https://codewandler.github.io/sipx/api/sipx_sip/transaction/index.html) |
| `sipx-transport` | [`bind`](https://codewandler.github.io/sipx/api/sipx_transport/endpoint/fn.bind.html) · [`Target`](https://codewandler.github.io/sipx/api/sipx_transport/target/struct.Target.html) |
| `sipx-ua` | [`UserAgent`](https://codewandler.github.io/sipx/api/sipx_ua/agent/struct.UserAgent.html) |
| `sipx-sdp` | [`answer`](https://codewandler.github.io/sipx/api/sipx_sdp/answer/fn.answer.html) |
| `sipx-rtp` | [`srtp`](https://codewandler.github.io/sipx/api/sipx_rtp/srtp/index.html) · [`rtcp`](https://codewandler.github.io/sipx/api/sipx_rtp/rtcp/index.html) |
| `sipx-media` | [`MediaSession`](https://codewandler.github.io/sipx/api/sipx_media/session/struct.MediaSession.html) |
| `sipx-call` | [`dial`](https://codewandler.github.io/sipx/api/sipx_call/call/fn.dial.html) · [`answer`](https://codewandler.github.io/sipx/api/sipx_call/call/fn.answer.html) · [`Call`](https://codewandler.github.io/sipx/api/sipx_call/call/struct.Call.html) |
| `sipx-testkit` | [`CallHarness`](https://codewandler.github.io/sipx/api/sipx_testkit/call/struct.CallHarness.html) · [`TransactionHarness`](https://codewandler.github.io/sipx/api/sipx_testkit/call/struct.TransactionHarness.html) · [`Faults`](https://codewandler.github.io/sipx/api/sipx_testkit/link/struct.Faults.html) |

The API reference is generated from the same `main` branch as this site. When using the tagged
release, consult the checked-out source documentation if an API has changed on `main`.
