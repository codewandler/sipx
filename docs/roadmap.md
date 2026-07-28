# sipx — roadmap & status

The big picture: what's delivered, what's next, and the epics that group related stories. The
operational detail lives on the [board](stories/README.md) (generated from story frontmatter);
this document is the hand-written narrative around it.

## Status

_As of 2026-07-28:_ pre-release, nothing shipped. The Cargo workspace is scaffolded with all
ten crates compiling, the lint and licensing policy is set (`unsafe_code = "forbid"`,
`MIT OR Apache-2.0`), and CI runs fmt, clippy, tests, MSRV, `cargo-deny` and the provenance
gate. Work now begins at the bottom of the stack: the sans-IO SIP core.

## Delivered

- Nothing yet. Milestone **M0 (workspace)** is in progress.

## Next

Milestones, in order. Each is independently demonstrable:

- **M0 — Workspace.** Scaffold, lints, licensing, CI, backlog, first specs.
- **M1 — Wire correctness.** `sipx-sip` round-trips every RFC 4475 valid message and rejects
  every invalid one; transaction FSMs pass exhaustive table tests; the parser survives fuzzing.
- **M2 — It talks.** Registers against Kamailio and Asterisk over UDP and TCP; answers
  `OPTIONS`.
- **M3 — It calls.** INVITE with SDP offer/answer and G.711 audio both ways: play a WAV into
  a call, record the far end, assert on the samples.
- **M4 — Phone.** `sipx dial | answer | register` with file/log media, recording, DTMF and
  machine-readable output.
- **M5 — Depth.** TLS/WS/WSS, bridging and mixing, transfer, jitter-buffer tuning, RTCP
  statistics, Opus, load testing.

## Epics

An **epic** is a themed group of stories with a shared design doc. Stories join an epic via the
`epic: <slug>` frontmatter field, where `<slug>` matches a design doc at `docs/designs/<slug>.md`.

### SIP core — `sip-core`

The sans-IO heart of the stack: URIs, headers, messages, an incremental parser, and the four
transaction state machines. No async, no sockets, no clock. Done when every RFC 4475 case
behaves as the RFC says, the FSMs are exhaustively table-tested, and the fuzzers run clean.
See [design](designs/sip-core.md).

### Transport — `sip-transport`

The driver that turns the sans-IO core into a running stack: UDP, TCP, TLS, WebSocket and
secure WebSocket; connection pooling and reuse; RFC 3263 target resolution; and the
practical necessities of NAT — `rport`, sent-by rewriting. Done when a message can be sent
and received over each transport, with a loopback harness proving the core is driven
correctly. See [design](designs/sip-transport.md).

### User agent — `sip-ua`

The roles applications use: a client that issues requests, a server that dispatches by
method, dialogs as typed state machines, digest authentication, and registration with
re-registration. Done when sipx registers with and is called by a third-party proxy.
See [design](designs/sip-ua.md).

### Media — `media`

SDP (RFC 8866) with offer/answer (RFC 3264) as a pure function; RTP and RTCP with a jitter
buffer and reception statistics; G.711 and G.722; symmetric-RTP address learning. Done when
two sipx endpoints exchange audio that survives a bit-exactness check.
See [design](designs/media.md).

### Call framework — `call`

What applications actually program against: answer and dial, playback, recording, DTMF,
two-party bridging, N-party mixing, and transfer (RFC 3515). Done when a bridged call passes
audio and DTMF in both directions with no shared mutable session.
See [design](designs/call.md).

### Phone CLI — `phone`

The `sipx` binary: dial, answer, register and load-test, with media sourced from files,
devices or generators, and results emitted in a form a test can assert on. Done when a shell
script can place a call, send DTMF, record the answer and verify it.
See [design](designs/phone.md).

### Edge / B2BUA — `edge` _(backlog, not scheduled)_

A programmable SIP and media edge: transports, endpoints and routes, with dialog bridging
and selected session-border behaviour. Deliberately deferred until the layers beneath it are
proven. See [design](designs/edge.md).
