<img src="docs/assets/logo.svg" alt="" width="132" align="right">

# sipx

**A SIP and VoIP stack in Rust.** Place and answer calls, register against a PBX, carry real
audio — as a library you embed, or as a command you run.

[![docs](https://img.shields.io/badge/docs-codewandler.github.io%2Fsipx-blue)](https://codewandler.github.io/sipx/)
[![RFCs tracked](https://img.shields.io/badge/RFCs%20tracked-61-blue)](docs/compliance.md)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

**📖 [codewandler.github.io/sipx](https://codewandler.github.io/sipx/)** — what it supports,
what it does not, and why.

---

## Can it do what I need?

**Today sipx is a user agent — a phone.** It calls, answers, registers, transfers, bridges and
conferences. It is not a proxy or a registrar: it does not fork requests or hold other people's
registrations.

| | |
|---|---|
| **Calls** | Place and answer, SDP offer/answer, hold and resume, blind and attended transfer |
| **Audio** | G.711 µ-law and A-law, Opus behind a feature, DTMF, play and record WAV |
| **Signalling security** | TLS and secure WebSocket, with certificate verification that **cannot be turned off** |
| **Media security** | **Not yet** — audio goes out unencrypted. It is the next thing on [the roadmap](docs/rfc-roadmap.md). |
| **Transports** | UDP, TCP, TLS, WebSocket, secure WebSocket |
| **Reachability** | NAT via `rport` and symmetric RTP. No Outbound, Path, GRUU or push yet. |
| **Multi-party** | Bridge two calls, or conference several with N−1 mixing |
| **Quality** | Loss, jitter, round-trip time and an estimated MOS, readable mid-call |

Verified against **Kamailio**, not only against itself: registration over UDP, TCP, TLS and
WebSocket — and the refusals that make the successes mean something.

**[→ What sipx supports, RFC by RFC](docs/compliance.md).** 61 RFCs, each marked implemented,
partial, parse-only or not started. The table is generated from a registry and CI fails the
build if it drifts from the code, so it is a measurement rather than a claim.

---

## Use it

### As a phone

```sh
sipx dial sip:bob@192.0.2.1:5060 --play hello.wav --record reply.wav
sipx answer --play greeting.wav --record caller.wav
sipx register sip:alice@example.com --password '…'
```

Every command speaks `--json` and returns a distinct exit code per outcome, so it drops into a
script without having to be parsed out of prose.

### As a library

```rust
use sipx_call::{DialOptions, dial};
use sipx_transport::{Config, Target, bind};

let (endpoint, _incoming) = bind(Config::new("0.0.0.0:5060".parse()?)).await?;

let mut call = dial(
    &endpoint,
    Target::udp("192.0.2.1:5060".parse()?),
    &to_uri,
    &DialOptions::new("<sip:alice@example.net>", local_ip),
)
.await?;

call.media().play(&samples, 160).await;
call.hang_up().await?;
```

The crates are useful on their own, too: take `sipx-sip` for a parser and transaction machines
with no async runtime at all, or `sipx-sdp` for offer/answer as a pure function.

---

## Why it is built this way

**The core does no I/O.** Parsing, the transaction state machines and dialog state are pure
functions over inputs and outputs — no sockets, no tasks, no clock. Time arrives as a
fired-timer input and leaves as a set-timer output.

That is the load-bearing decision. The parts of SIP that are genuinely hard — retransmission
timing, transaction matching, hostile input — are tested deterministically and fuzzed without a
runtime, rather than chased through timing flakes in integration tests.

Three things follow from it:

- **Nothing is lost on the wire.** A parsed message borrows the bytes it arrived in, and headers
  sipx has no behaviour for still survive intact. That is why "parse-only" is a status in the
  compliance table rather than a gap in it.
- **Ownership, not sharing.** A call owns its media pipeline. Bridging moves frames over
  channels; no media session sits behind a mutex.
- **Malformed input is a value, not a panic.** `unsafe` is forbidden across the workspace and
  parse failures are typed errors. The whole RFC 4475 torture corpus is asserted — including the
  messages that must be *rejected*, and by which layer.

---

## Crates

| Crate | What it does |
|---|---|
| `sipx-sip` | Sans-IO SIP core: messages, parser, transactions, dialogs — no async runtime |
| `sipx-transport` | UDP, TCP, TLS, WS, WSS, connection reuse, RFC 3263 resolution |
| `sipx-ua` | User agent: digest authentication, registration as a lease |
| `sipx-sdp` | SDP, and offer/answer as a pure function |
| `sipx-rtp` | RTP, RTCP, an adaptive jitter buffer, quality statistics |
| `sipx-audio` | G.711, Opus, mixing, WAV, RFC 4733 DTMF |
| `sipx-media` | Media sessions, bridging, conferencing |
| `sipx-call` | Calls: playback, recording, DTMF, transfer |
| `sipx-cli` | The `sipx` binary |
| `sipx-testkit` | Torture corpus, fixture CA, load and soak harnesses |

---

## Documentation

**[codewandler.github.io/sipx](https://codewandler.github.io/sipx/)** is the site; the pages
below are its source, readable here too.

- **[RFC compliance](docs/compliance.md)** — what is supported, measured rather than asserted
- **[RFC roadmap](docs/rfc-roadmap.md)** — what comes next, and why in that order
- **[Roadmap and status](docs/roadmap.md)** — the narrative around the board
- **[Specs](docs/specs/)** — an implementable contract per subsystem, written before the code
- **[Board](docs/stories/README.md)** — what is being worked on now

The site is built from `docs/` rather than from a copy of it — `./scripts/build-docs.sh` builds
it locally and fails if a page links to something the site does not publish.

---

## Contributing

```sh
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
./scripts/rfc-report.py --check
./scripts/build-docs.sh
```

Design happens in `docs/specs/` before code, and every behavioural change names the test that
fails without it. [`AGENTS.md`](AGENTS.md) has the full gate and the working rules — it is for
contributors and agents; this file is for people deciding whether sipx fits.

## License

`MIT OR Apache-2.0`, at your option.
