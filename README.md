<div align="center">

<img src="docs/assets/logo.svg" alt="" width="132">

# sipx

**A SIP and VoIP stack in Rust for programmable endpoints.** Place and answer calls, register,
transfer, and carry real audio from a Rust library or a shell command.

<!-- BEGIN generated:badges -->
<a href="https://codewandler.github.io/sipx/"><img alt="docs: codewandler.github.io/sipx" src="https://img.shields.io/static/v1?label=docs&message=codewandler.github.io%2Fsipx&color=blue"></a>
<a href="CHANGELOG.md"><img alt="release: 1.0.0-beta.5" src="https://img.shields.io/static/v1?label=release&message=1.0.0-beta.5&color=blue"></a>
<a href="#try-the-cli"><img alt="MSRV: rustc 1.88" src="https://img.shields.io/static/v1?label=MSRV&message=rustc%201.88&color=blue"></a>
<a href="docs/compliance.md"><img alt="RFCs: 36 implemented of 78" src="https://img.shields.io/static/v1?label=RFCs&message=36%20implemented%20of%2078&color=blue"></a>
<a href="docs/compliance.md"><img alt="codecs: G.711 · Opus" src="https://img.shields.io/static/v1?label=codecs&message=G.711%20%C2%B7%20Opus&color=blue"></a>
<a href="#license"><img alt="license: MIT OR Apache-2.0" src="https://img.shields.io/static/v1?label=license&message=MIT%20OR%20Apache-2.0&color=blue"></a>
<!-- END generated:badges -->

</div>

> **Status: <!-- BEGIN generated:workspace-version -->1.0.0-beta.5<!-- END generated:workspace-version -->.** This is the current public-beta release. `main` can move ahead of
> the release tag. Public APIs are not frozen;
> Supported APIs receive migration notes when they break, while Experimental APIs may change or be
> removed without one. Start with the exact registry install below when reproducibility matters.

## Does it fit?

sipx is a **user agent**: the endpoint that places or receives a call. It is not a proxy, registrar,
PBX, browser media engine, or video stack.

| Need | Today |
|---|---|
| Calls | Place and answer, hold and resume, blind and attended transfer, session timers, bounded confirmed-dialog snapshots |
| Audio | G.711, DTMF, WAV playback and recording; optional Opus and explicitly selected live devices behind Cargo features |
| Security | TLS and secure WebSocket; selectable plain RTP, SDES-keyed SRTP, optional DTLS-SRTP, and a fail-closed browser-audio composition profile |
| Reachability | `rport`, symmetric RTP, Path, Service-Route, Outbound, GRUU and push refresh; host and STUN-derived ICE candidates, but no TURN relay |
| SIP events | Bounded inbound notifier, package-generic authenticated subscriber, live registration discovery, and conditional presence publication in both roles |
| Automation | Single-line JSON reports, distinct outcome exit codes, interactive scenarios, bounded load, quality statistics and signalling capture |
| Two-leg calls | Public early and confirmed coupling of two dialogs, with optional media bridging; the off-media relay role remains unfinished |

The `sipx` CLI is intentionally a scriptable phone, not a desktop softphone. WAV files are the
reproducible default; builds with the optional `device-audio` feature can open an exact microphone
or speaker identifier. `dial`, `answer`, and `register` select UDP, TCP, TLS, WebSocket, or secure
WebSocket, and call commands expose codec, media-security, ICE policy, and the named
`browser-audio` composition when the CLI is built with the optional `opus` and `dtls` features.
That profile is intentionally one secure audio stream, not a general browser API or general WebRTC
stack. Its exact host/server-reflexive boundary is exercised in both SIP roles by the
**[native-browser audio proof](https://codewandler.github.io/sipx/docs/reference/browser-audio-proof)**. Read
**[Does sipx fit?](https://codewandler.github.io/sipx/docs/guides/does-this-fit)** and the
**[security matrix](https://codewandler.github.io/sipx/docs/reference/security)** before choosing a
deployment shape.

## Try the CLI

The <!-- BEGIN generated:release-tag -->v1.0.0-beta.5<!-- END generated:release-tag --> beta release needs
Rust <!-- BEGIN generated:msrv -->1.88<!-- END generated:msrv --> or newer:

```sh
cargo install --locked --version =1.0.0-beta.5 sipx-cli
sipx version
```

To use the bounded browser-audio profile, install that same exact release with its two native
media features:

```sh
cargo install --locked --version =1.0.0-beta.5 --features opus,dtls sipx-cli
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
`--record reply.wav` to move audio samples; WAV input is 16-bit mono at the negotiated clock
(8 kHz for G.711, or 48 kHz for an Opus-only call), and recordings preserve that rate. The
**[getting-started guide](https://codewandler.github.io/sipx/docs/getting-started)** continues with
registration, expected output, and installing from `main`.

## Use the Rust libraries

The workspace deliberately publishes modular crates rather than one facade crate. Pin every sipx
dependency to the same exact beta while the API remains pre-1.0:

```toml
[dependencies]
sipx-call = "=1.0.0-beta.5"
sipx-transport = "=1.0.0-beta.5"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

The call guides inline real example files that CI compiles:

- [Place a call](https://codewandler.github.io/sipx/docs/guides/place-a-call)
- [Answer a call](https://codewandler.github.io/sipx/docs/guides/answer-a-call)
- [Register](https://codewandler.github.io/sipx/docs/guides/register)
- [Choose crates and features](https://codewandler.github.io/sipx/docs/guides/as-a-library)

The beta release is for programmable SIP endpoints, not a promise of every telephony role. It
does not provide proxy, registrar, PBX, TURN for relay-required networks, video, data channels,
browser-facing APIs, or a general browser-media engine. It remains a prerelease rather than stable
`1.0`, and the language-neutral application contract remains Experimental. The public
**[fit guide](https://codewandler.github.io/sipx/docs/guides/does-this-fit)** is the canonical list
of shipped boundaries and intentional omissions.

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
| `sipx-app` | SIP application host with webhook, session, and realtime audio bindings |
| `sipx-app-protocol` | The sipx.app.v1 application contract: its types, JSON wire format, and sans-IO instruction interpreter |
| `sipx-audio` | Telephony audio: G.711 µ-law and A-law, PCM mixing, WAV I/O, and Opus behind the `opus` feature |
| `sipx-call` | Call framework: dial, answer, couple dialogs, play, record, send DTMF, and transfer |
| `sipx-cli` | sipx — a command line SIP softphone |
| `sipx-media` | Media sessions: RTP/RTCP sockets bound to negotiated SDP with NAT handling, bridging and conferencing |
| `sipx-rtp` | RTP and RTCP packet handling, sequencing, jitter buffering, quality statistics and SRTP (RFC 3550) |
| `sipx-sdp` | SDP session descriptions (RFC 8866) and offer/answer negotiation (RFC 3264) |
| `sipx-sip` | Sans-IO SIP core: messages, parser and transactions (RFC 3261) |
| `sipx-testkit` | Deterministic SIP and RTP tests with bounded realtime peers, virtual time, and RFC corpora |
| `sipx-transport` | Async SIP transports: UDP, TCP, TLS, WebSocket, experimental QUIC, and RFC 3263 resolution |
| `sipx-ua` | SIP user agent: registration, digest authentication, event subscriptions, and presence |
<!-- END generated:crate-map -->

Each crate states its supported and experimental surface in its crate-level API documentation.
See the [API reference](https://codewandler.github.io/sipx/api/) for the exact contract.
Production reachability is measured from `sipx-app`. The Supported `sipx-testkit` harness is a
separate test-product surface: a manifest-declared Cargo example is compiled as its caller and the
release rehearsal rebuilds that archived example in a clean consumer; it does not promote the
testkit's dependencies into the production surface.

## Documentation

The [public site](https://codewandler.github.io/sipx/) is for users and integrators. It includes:

- [Getting started](https://codewandler.github.io/sipx/docs/getting-started)
- [What's new](https://codewandler.github.io/sipx/docs/whats-new)
- [CLI reference](https://codewandler.github.io/sipx/docs/reference/cli)
- [Troubleshooting](https://codewandler.github.io/sipx/docs/guides/troubleshooting)
- [How sipx is built](https://codewandler.github.io/sipx/docs/reference/development-process)
- [Diagnostic-phone proof](https://codewandler.github.io/sipx/docs/reference/diagnostic-phone-proof)
- [Native-browser audio proof](https://codewandler.github.io/sipx/docs/reference/browser-audio-proof)
- [RFC compliance](https://codewandler.github.io/sipx/docs/reference/compliance)
- [How sipx compares](https://codewandler.github.io/sipx/docs/reference/comparison)

The compliance registry currently tracks <!-- BEGIN generated:rfc-count -->78<!-- END generated:rfc-count --> RFCs; its public table is generated rather than copied by hand.

Contributor specifications, designs, the roadmap, and the generated work board stay under
[`docs/`](docs/). `./scripts/build-docs.sh` builds the public site, checks every link, verifies the
inlined examples, and builds the API reference with warnings denied.

## Contributing

Read [`AGENTS.md`](AGENTS.md) for the working agreement. Behavioural changes start with a spec and a
failing-first test; `./scripts/gate.py` is the complete local acceptance gate. Do not substitute a
hand-copied command list for it.

## License

`MIT OR Apache-2.0`, at your option.
