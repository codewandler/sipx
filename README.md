# sipx

A SIP and VoIP stack in Rust: signalling, media, and a softphone you can drive from a shell.

> **Status: early.** The workspace is scaffolded and the backlog is planned. See
> [`docs/stories/README.md`](docs/stories/README.md) for what is being built and in what order.

## What it is

`sipx` is a full telephony stack rather than a protocol parser:

- **Signalling** — RFC 3261 SIP over UDP, TCP, TLS and WebSocket, with transactions,
  dialogs, digest authentication and registration.
- **Media** — SDP offer/answer, RTP and RTCP, a jitter buffer, and telephony codecs
  (G.711, G.722; Opus behind a feature flag).
- **Calls** — a framework for answering and placing calls with playback, recording, DTMF,
  bridging, mixing and transfer.
- **A phone** — the `sipx` binary: dial, answer, register and load-test, with
  machine-readable output.

## Design

**The core does no I/O.** Message parsing, the transaction state machines and dialog state
are pure functions over inputs and outputs — no sockets, no tasks, no clock. Time arrives as
a fired-timer input and leaves as a set-timer output. The async layer is a thin driver on
top.

This is the load-bearing decision in the codebase. It means the parts of SIP that are
genuinely hard — retransmission timing, transaction matching, hostile input — are tested
deterministically and fuzzed without a runtime, instead of being chased through timing
flakes in integration tests.

Three consequences follow:

- **Zero-copy messages.** A parsed message borrows the bytes it arrived in; typed header
  access is lazy. Proxy paths forward headers verbatim instead of reparsing and
  reserializing them.
- **Ownership, not sharing.** A call owns its media pipeline. Bridging moves frames over
  channels; there is no media session shared behind a mutex.
- **Malformed input is a value, not a panic.** `unsafe` is forbidden workspace-wide, and
  parse failures are typed errors.

## Layout

| Crate | What it does |
|---|---|
| `sipx-sip` | Sans-IO SIP core: messages, parser, transactions, dialog state |
| `sipx-transport` | Async transports (UDP/TCP/TLS/WS/WSS), connection reuse, RFC 3263 resolution |
| `sipx-ua` | User agent: client, server, dialogs, digest auth, registration |
| `sipx-sdp` | SDP (RFC 8866) and offer/answer (RFC 3264) |
| `sipx-rtp` | RTP/RTCP, sequencing, jitter buffer, statistics |
| `sipx-audio` | G.711, G.722, PCM mixing/resampling, WAV, RFC 4733 DTMF |
| `sipx-media` | Media sessions: sockets bound to negotiated SDP, NAT handling |
| `sipx-call` | Call framework: playback, recording, DTMF, bridging, transfer |
| `sipx-cli` | The `sipx` softphone binary |
| `sipx-testkit` | Loopback transport, RFC 4475 torture corpus, interop fixtures |

`sipx-sip` and `sipx-sdp` depend on no async runtime.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

Design work happens in specs before code: `docs/specs/` holds an implementable contract per
subsystem — normative RFC references, types, state tables, timers and test vectors. Stories
in `docs/stories/` reference them.

## License

`MIT OR Apache-2.0`, at your option.
