---
title: Use sipx as a library
description: The crates are useful separately — a parser and transaction machines with no async runtime, offer/answer as a pure function, or the whole call framework.
---

# Use sipx as a library

The crates are useful separately. `sipx-sip` and `sipx-sdp` depend on **no async runtime at
all** — take them for a parser, transaction state machines, or offer/answer as a pure function,
without taking a socket layer with them.

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

## What is worth noticing

**Nothing is lost on the wire.** A parsed message borrows the bytes it arrived in, and a header
sipx has no behaviour for survives intact and re-serializes byte for byte. That is why
*parse-only* is a status in [the compliance table](../reference/compliance.md) rather than a
gap in it — a message carrying an exotic header field passes through unharmed even though
nothing acts on it.

**The core does no I/O.** The transaction machines take inputs and return outputs: time arrives
as a fired-timer input and leaves as a set-timer output. They can be driven with no clock and no
socket, which is how the retransmission behaviour is tested deterministically rather than
chased through timing flakes.

**Malformed input is a value.** `unsafe` is forbidden across the workspace and parse failures
are typed errors, with the whole RFC 4475 torture corpus asserted — including which layer must
object to each message that has to be rejected.

## Which crate

| You want | Take |
|---|---|
| Parse and build SIP messages, transactions, dialogs | `sipx-sip` |
| SDP and offer/answer | `sipx-sdp` |
| Sockets, TLS, WebSocket, RFC 3263 resolution | `sipx-transport` |
| Registration and digest authentication | `sipx-ua` |
| RTP, RTCP, jitter buffer, SRTP | `sipx-rtp` |
| Calls with playback, recording, transfer | `sipx-call` |

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

Every public item in the workspace carries documentation, and the build denies both a missing
one and an intra-doc link that resolves nowhere. That is not a claim about diligence — it is
`RUSTDOCFLAGS="-D warnings"` in the build script CI runs.
