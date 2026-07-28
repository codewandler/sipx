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

## The API reference

Generated from the source with `cargo doc`, and published here rather than only on docs.rs so
that it always matches the guides beside it.

| Crate | Start at |
|---|---|
| `sipx-sip` | [`parser`](../api/sipx_sip/parser/index.html) · [`transaction`](../api/sipx_sip/transaction/index.html) · [`session`](../api/sipx_sip/session/index.html) |
| `sipx-transport` | [`bind`](../api/sipx_transport/endpoint/fn.bind.html) · [`Config`](../api/sipx_transport/endpoint/struct.Config.html) · [`Target`](../api/sipx_transport/target/struct.Target.html) |
| `sipx-ua` | [`UserAgent`](../api/sipx_ua/agent/struct.UserAgent.html) · [`registrar`](../api/sipx_ua/registrar/index.html) · [`auth`](../api/sipx_ua/auth/index.html) |
| `sipx-sdp` | [`answer`](../api/sipx_sdp/answer/fn.answer.html) · [`Capabilities`](../api/sipx_sdp/answer/struct.Capabilities.html) |
| `sipx-rtp` | [`srtp`](../api/sipx_rtp/srtp/index.html) · [`rtcp`](../api/sipx_rtp/rtcp/index.html) |
| `sipx-media` | [`MediaSession`](../api/sipx_media/session/struct.MediaSession.html) |
| `sipx-call` | [`dial`](../api/sipx_call/call/fn.dial.html) · [`answer`](../api/sipx_call/call/fn.answer.html) · [`Call`](../api/sipx_call/call/struct.Call.html) · [`serve`](../api/sipx_call/call/fn.serve.html) |

Every public item in the workspace carries documentation, and the build denies both a missing
one and an intra-doc link that resolves nowhere. That is not a claim about diligence — it is
`RUSTDOCFLAGS="-D warnings"` in `scripts/build-docs.sh`, which is also what CI runs.
