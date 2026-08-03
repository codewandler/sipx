<div align="center">

<img src="docs/assets/logo.svg" alt="" width="132">

# sipx

**A SIP and VoIP stack in Rust for programmable endpoints.** Place and answer calls, register,
transfer, and carry real audio from a Rust library or a shell command.

<!-- BEGIN generated:badges -->
<a href="https://codewandler.github.io/sipx/"><img alt="docs: codewandler.github.io/sipx" src="https://img.shields.io/static/v1?label=docs&message=codewandler.github.io%2Fsipx&color=blue"></a>
<a href="CHANGELOG.md"><img alt="release: 1.0.0-alpha.5" src="https://img.shields.io/static/v1?label=release&message=1.0.0-alpha.5&color=blue"></a>
<a href="#try-the-cli"><img alt="MSRV: rustc 1.88" src="https://img.shields.io/static/v1?label=MSRV&message=rustc%201.88&color=blue"></a>
<a href="docs/compliance.md"><img alt="RFCs: 32 implemented of 71" src="https://img.shields.io/static/v1?label=RFCs&message=32%20implemented%20of%2071&color=blue"></a>
<a href="docs/compliance.md"><img alt="codecs: G.711 · Opus" src="https://img.shields.io/static/v1?label=codecs&message=G.711%20%C2%B7%20Opus&color=blue"></a>
<a href="#license"><img alt="license: MIT OR Apache-2.0" src="https://img.shields.io/static/v1?label=license&message=MIT%20OR%20Apache-2.0&color=blue"></a>
<!-- END generated:badges -->

</div>

> **Status: <!-- BEGIN generated:workspace-version -->1.0.0-alpha.5<!-- END generated:workspace-version -->.** The public site documents `main`, which can move ahead of the latest
> tag. Public APIs are not frozen. Start with the tagged install below when reproducibility matters.

## Does it fit?

sipx is a **user agent**: the endpoint that places or receives a call. It is not a proxy, registrar,
PBX, browser media engine, or video stack.

| Need | Today |
|---|---|
| Calls | Place and answer, hold and resume, blind and attended transfer, session timers |
| Audio | G.711, DTMF, WAV playback and recording; selectable Opus behind a Cargo feature |
| Security | TLS and secure WebSocket in the library; SRTP with SDES when signalling protects the key |
| Reachability | `rport`, symmetric RTP, Path, Service-Route, Outbound, GRUU and push refresh; no ICE |
| Automation | Single-line JSON reports, distinct outcome exit codes, quality statistics and signalling capture |
| Multi-party | Media-session bridging and N−1 conferencing; connecting two `Call` values is not exposed yet |

The `sipx` CLI is intentionally a scriptable phone, not a desktop softphone: it reads and writes
WAV files and does not open a microphone or speaker. Its `dial` and `register` commands currently
select UDP or TCP only, so encrypted calls require the Rust library. Read
**[Does sipx fit?](https://codewandler.github.io/sipx/docs/guides/does-this-fit)** and the
**[security matrix](https://codewandler.github.io/sipx/docs/reference/security)** before choosing a
deployment shape.

## Try the CLI

The <!-- BEGIN generated:release-tag -->v1.0.0-alpha.5<!-- END generated:release-tag --> release needs
Rust <!-- BEGIN generated:msrv -->1.88<!-- END generated:msrv --> or newer:

```sh
cargo install --locked --git https://github.com/codewandler/sipx --tag v1.0.0-alpha.5 sipx-cli
sipx version
```

Then make a bounded loopback call. Terminal one listens for at most 15 seconds:

```sh
sipx answer --wait 15 --duration 2 --once --json
```

Terminal two calls it and hangs up after two seconds:

```sh
sipx dial sip:you@127.0.0.1:5060 --duration 2 --timeout 5 --json
```

That proves the signalling and media session without needing an account. Add `--play hello.wav` or
`--record reply.wav` to move audio samples; WAV input is 8 kHz, 16-bit, mono. The
**[getting-started guide](https://codewandler.github.io/sipx/docs/getting-started)** continues with
registration, expected output, and installing from `main`.

## Use the Rust libraries

Until the crates are published, pin Git dependencies to the same tag:

```toml
[dependencies]
sipx-call = { git = "https://github.com/codewandler/sipx", tag = "v1.0.0-alpha.5" }
sipx-transport = { git = "https://github.com/codewandler/sipx", tag = "v1.0.0-alpha.5" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

The call guides inline real example files that CI compiles:

- [Place a call](https://codewandler.github.io/sipx/docs/guides/place-a-call)
- [Answer a call](https://codewandler.github.io/sipx/docs/guides/answer-a-call)
- [Register](https://codewandler.github.io/sipx/docs/guides/register)
- [Choose crates and features](https://codewandler.github.io/sipx/docs/guides/as-a-library)

## Why the core is different

`sipx-sip` and `sipx-sdp` do no I/O. Bytes and fired timers enter as data; messages, state changes,
and timer requests leave as outputs. That makes transaction timing deterministic in tests and keeps
sockets, async tasks, and clock reads in the driver crates.

Malformed network input produces typed errors, never panics. `unsafe` is forbidden across the
workspace, parsers are fuzzed, and unknown headers survive a parse/serialize round trip intact.

## Crates

<!-- BEGIN generated:crate-map -->
| Crate | What it does |
|---|---|
| `sipx-app` | Experimental SIP application host, configuration reader, and deterministic contract harness (host process available; callback bindings not yet implemented) |
| `sipx-app-protocol` | The sipx.app.v1 application contract: its types, its JSON wire format, and a sans-IO instruction interpreter (experimental) |
| `sipx-audio` | Telephony audio: G.711 µ-law and A-law, PCM mixing, WAV I/O, and Opus behind the `opus` feature |
| `sipx-call` | Call framework: answer and dial calls with playback, recording, DTMF and transfer |
| `sipx-cli` | sipx — a command line SIP softphone |
| `sipx-media` | Media sessions: RTP/RTCP sockets bound to negotiated SDP with NAT handling, bridging and conferencing |
| `sipx-rtp` | RTP and RTCP packet handling, sequencing, jitter buffering, quality statistics and SRTP (RFC 3550) |
| `sipx-sdp` | SDP session descriptions (RFC 8866) and offer/answer negotiation (RFC 3264) |
| `sipx-sip` | Sans-IO SIP core: messages, parser and transactions (RFC 3261) |
| `sipx-transport` | Async SIP transports: UDP, TCP, TLS, WebSocket, with RFC 3263 resolution |
| `sipx-ua` | SIP user agent: registration, digest authentication, subscriptions and presence |
<!-- END generated:crate-map -->

Each crate states its supported and experimental surface in its crate-level API documentation.
See the [API reference](https://codewandler.github.io/sipx/api/) for the exact contract.

## Documentation

The [public site](https://codewandler.github.io/sipx/) is for users and integrators. It includes:

- [Getting started](https://codewandler.github.io/sipx/docs/getting-started)
- [CLI reference](https://codewandler.github.io/sipx/docs/reference/cli)
- [Troubleshooting](https://codewandler.github.io/sipx/docs/guides/troubleshooting)
- [RFC compliance](https://codewandler.github.io/sipx/docs/reference/compliance)

The compliance registry currently tracks <!-- BEGIN generated:rfc-count -->71<!-- END generated:rfc-count --> RFCs; its public table is generated rather than copied by hand.

Contributor specifications, designs, the roadmap, and the generated work board stay under
[`docs/`](docs/). `./scripts/build-docs.sh` builds the public site, checks every link, verifies the
inlined examples, and builds the API reference with warnings denied.

## Contributing

Read [`AGENTS.md`](AGENTS.md) for the working agreement. Behavioural changes start with a spec and a
failing-first test; `./scripts/gate.py` is the complete local acceptance gate. Do not substitute a
hand-copied command list for it.

## License

`MIT OR Apache-2.0`, at your option.
