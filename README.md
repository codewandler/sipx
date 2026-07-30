<img src="docs/assets/logo.svg" alt="" width="132" align="right">

# sipx

**A SIP and VoIP stack in Rust.** Place and answer calls, register against a PBX, carry real
audio — as a library you embed, or as a command you run.

[![docs](https://img.shields.io/badge/docs-codewandler.github.io%2Fsipx-blue)](https://codewandler.github.io/sipx/)
[![RFCs tracked](https://img.shields.io/badge/RFCs%20tracked-70-blue)](docs/compliance.md)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

**📖 [codewandler.github.io/sipx](https://codewandler.github.io/sipx/)** — what it supports,
what it does not, and why.

---

## Can it do what I need?

**Today sipx is a user agent — a phone.** It calls, answers, registers and transfers. It is not a
proxy or a registrar: it does not fork requests or hold other people's registrations.

| | |
|---|---|
| **Calls** | Place and answer, SDP offer/answer, hold and resume, blind and attended transfer |
| **Audio** | G.711 µ-law and A-law, DTMF, play and record WAV. Opus too, behind `sipx-call`'s `opus` feature: a call offers G.711 unless it selects Opus, and the default never moves |
| **Signalling security** | TLS and secure WebSocket, with certificate verification that **cannot be turned off** |
| **Media security** | SRTP with SDES keying, negotiated automatically when the signalling is secure |
| **Transports** | UDP, TCP, TLS, WebSocket, secure WebSocket |
| **Reachability** | NAT via `rport` and symmetric RTP; `Path` and `Service-Route` honoured; RFC 5626 Outbound down a client-opened flow, GRUU, and a binding refreshed on a push. No ICE yet. |
| **Multi-party** | Bridging two media sessions and conferencing several with N−1 mixing live in `sipx-media`; reaching them from a `Call` is being finished (`C-6`) |
| **Liveness** | RFC 4028 session timers, so a far end that loses power is noticed rather than billed for |
| **Quality** | Loss, jitter, round-trip time and an estimated MOS, readable mid-call |

Verified against **two independent peers**, not only against itself — a proxy (Kamailio) and a PBX
and back-to-back user agent on an unrelated SIP library (Asterisk). Every peer runs the same list:
registration over UDP, TCP, TLS and WebSocket, and the refusals that make the successes mean
something. The one that answers calls also places and answers them with sipx, with SDES-keyed
SRTP on the media.

**[→ What sipx supports, RFC by RFC](docs/compliance.md).** 70 RFCs, each marked implemented,
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
| `sipx-sip` | Sans-IO SIP core: messages, parser, transactions — no async runtime |
| `sipx-transport` | UDP, TCP, TLS, WS, WSS, connection reuse, RFC 3263 resolution |
| `sipx-ua` | User agent: digest authentication, registration as a lease, subscriptions and presence |
| `sipx-sdp` | SDP, and offer/answer as a pure function |
| `sipx-rtp` | RTP, RTCP, an adaptive jitter buffer, quality statistics, SRTP |
| `sipx-audio` | G.711 (µ-law and A-law), mixing, WAV, and Opus behind the `opus` feature |
| `sipx-media` | Media sessions, bridging, conferencing |
| `sipx-call` | Calls: playback, recording, DTMF, transfer |
| `sipx-cli` | The `sipx` binary |
| `sipx-app-protocol` | The `sipx.app.v1` contract: its types, wire format and interpreter |
| `sipx-app` | The application host — in development |

**Each crate says what it guarantees in its own crate-level documentation**, under a `# Stability`
heading: *Supported* means meant to be depended on, with a changelog entry and a migration note if it
breaks; *Experimental* means it may change shape or be removed without one. Neither means frozen — 1.0
is what freezes an API, and its predicates are in [`docs/roadmap.md`](docs/roadmap.md). Several crates
mark part of their surface experimental for the same reason: it is implemented and tested and nothing
above it selects it yet, so no caller has ever constrained its shape.

**Which surface is which is not a judgement, it is a measurement.** The reachable-from-a-call surface
is *defined* as what the shipped application in [`crates/sipx-app`](crates/sipx-app) uses, and
`./scripts/check-app-surface.py --check` fails the build when a crate claims *Supported* surface that
no path from that application reaches, or when the application starts selecting something still marked
*Experimental*. Three earlier attempts checked this by reading evidence paths and each recorded the
same limit: a path is satisfied by citing a file whose relevant branch is dead. An application has no
dead branch to cite.

**Two applications ship, and a claim says which one backs it.** The call-reachable surface is what
[`crates/sipx-app`](crates/sipx-app) uses; `sipx-cli`'s promise is its command-line surface, documented
in [the CLI reference](website/docs/reference/cli.md) and asserted by its own tests. Most of
[`sipx-ua`](crates/sipx-ua)'s supported surface is the second kind: `sipx register --outbound` calls it
and the host does not, because registration happens before and outside any call. That is why the crate
says which application backs its claim, and why `check-app-surface.py` verifies the citation rather
than trusting it. A claim measured by the wrong instrument is a bug; a claim measured by **nothing** is
what these checks exist to find.

**A Cargo feature is part of being selectable.** A capability behind a feature that no shipped binary
*enables* is *Experimental*, however thoroughly it is implemented and tested and whatever
`--all-features` compiles. Opus is the worked example: it is complete, it has vectors, RFC 6716 and
7587 are cited against it, and it sits behind `sipx-audio/opus`, which links libopus. `sipx-cli` has
no flag for it and no `[features]` table to forward one, and the host does not enable it, because
linking a C library is a deployment decision rather than something a default should make for you. So
Opus is reachable from the library and from no application, and it says so on its own page. This is
the distinction three earlier attempts could not draw, because every *path* to Opus is real.

*Enables*, not *can enable*: the check resolves each application with the features it ships with, so
turning a feature on at the command line is not what widens the surface — changing what the shipped
binary enables is, and that comes with a `CHANGELOG.md` entry. Building with a non-default feature is
opting into the experimental half knowingly, which is exactly what the word is there to tell you.

**The rule runs in both directions.**

- **Graduation.** If something outside this repository depends on an experimental item, that is not a
  mistake to be corrected at the caller — a second caller is exactly what constrains a shape, and
  nothing else can. The item moves to *Supported* with a `CHANGELOG.md` entry, and the surface is
  wider than it was. **Please open an issue saying what you depend on**: that is the mechanism
  working, not a complaint.
- **Demotion.** If the application stops using a capability — a feature switched off, a dependency
  dropped, a call path removed — it returns to *Experimental*, with a `CHANGELOG.md` entry saying so.
  The same sentence has to be sayable in both directions or the measurement is a ratchet: a surface
  that can only grow is a freeze arriving one item at a time. A row of
  [`docs/rfc/registry.toml`](docs/rfc/registry.toml) that claimed a role on the strength of the
  removed path is demoted in the same commit, exactly as `X-30` and `X-33` demoted theirs.

Without these two clauses the definition above would freeze the stack at whatever one application
happens to need, instead of measuring it.

The table is exactly the crates that publish, and `./scripts/check-audio-claims.py --check` holds
it to that: a published crate no table describes has no front door anyone can be held to. The
workspace also contains `sipx-testkit` — the torture corpus, the fixture CA and the load and soak
harnesses — which is `publish = false`, so it is not a dependency you can take.

---

## Documentation

**[codewandler.github.io/sipx](https://codewandler.github.io/sipx/)** is the public site — for
users and integrators, hand-written under [`website/`](website/). The internal contributor
material stays in [`docs/`](docs/):

- **[Does sipx fit?](https://codewandler.github.io/sipx/docs/guides/does-this-fit)** — what it
  is for and what it is not
- **Guides** — [getting started](https://codewandler.github.io/sipx/docs/getting-started),
  [place a call](https://codewandler.github.io/sipx/docs/guides/place-a-call),
  [answer one](https://codewandler.github.io/sipx/docs/guides/answer-a-call),
  [use it as a library](https://codewandler.github.io/sipx/docs/guides/as-a-library)
- **[The SDK preview](https://codewandler.github.io/sipx/docs/sdk/overview)** — call control
  without Rust: where it is headed, what is real
- **[Migrating](https://codewandler.github.io/sipx/docs/migrate/from-kamailio)** — from
  Kamailio or Asterisk, what maps where
- **[API reference](https://codewandler.github.io/sipx/api/)** — `cargo doc` for every crate,
  built with warnings denied so a missing doc or a dead link fails the build
- **[RFC compliance](docs/compliance.md)** — what is supported, measured rather than asserted
- **[RFC roadmap](docs/rfc-roadmap.md)** — what comes next, and why in that order
- **[Roadmap and status](docs/roadmap.md)** — the narrative around the board
- **[Specs](docs/specs/)** — an implementable contract per subsystem, written before the code
- **[Board](docs/stories/README.md)** — what is being worked on now

`./scripts/build-docs.sh` builds the site locally: it compiles the example files the guides
inline, refuses a stale inlined sample, and fails on any link that goes nowhere — on the site
and inside `docs/`.

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
