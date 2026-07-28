# Use sipx as a library

The crates are useful separately. `sipx-sip` and `sipx-sdp` depend on **no async runtime at
all** — take them for a parser, transaction state machines, or offer/answer as a pure function,
without taking a socket layer with them.

```rust
{{#include ../../crates/sipx-sip/examples/parse_a_message.rs}}
```

## What is worth noticing

**Nothing is lost on the wire.** A parsed message borrows the bytes it arrived in, and a header
sipx has no behaviour for survives intact and re-serializes byte for byte. That is why
*parse-only* is a status in [the compliance table](../compliance.md) rather than a gap in it —
a message carrying `RAck` passes through unharmed even though nothing sends PRACK.

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
