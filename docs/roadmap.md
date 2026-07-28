# sipx — roadmap & status

The big picture: what's delivered, what's next, and the epics that group related stories. The
operational detail lives on the [board](https://github.com/codewandler/sipx/blob/main/docs/stories/README.md) (generated from story frontmatter);
this document is the hand-written narrative around it.

Two companions to this one: [**RFC compliance**](compliance.md) is the measured list of what
sipx supports, generated from a registry and checked in CI; [**the RFC roadmap**](rfc-roadmap.md)
is the order the remaining gaps close in and why.

## Status

_As of 2026-07-28:_ **M0 through M5 are complete**, and **M6 is under way.** `sipx-sip` is a
working sans-IO SIP core: URIs, headers, an incremental parser for both datagram and stream
transports, message validation, injection-proof builders, and all four transaction state
machines with matching and stores. 754 tests pass; clippy is clean at `-D warnings` on both
feature sets; the whole RFC 4475 torture corpus is green across all four of its layers.

sipx registers against a real Kamailio over UDP, TCP and TLS and answers `OPTIONS`. Between two
sipx endpoints it places calls carrying G.711 audio in both directions, encrypted with SRTP when
the offer and answer agree on it — and `sipx dial | answer | register` does all of that from a
terminal.

Next is **M6**: the last few things that decide whether a real deployment can register this
stack and route back to it.

## Delivered

- **M0 — Workspace.** Ten crates, shared lints (`unsafe_code = "forbid"`), `MIT OR
  Apache-2.0`, CI with fmt/clippy/tests/MSRV/`cargo-deny`, and a provenance gate that fails
  the build rather than passing unconfigured.
- **M1 — Wire correctness.** The sans-IO SIP core:
  - RFC 4475 corpus recovered bit-exactly from the RFC's own Appendix A archive and
    classified by which layer must object to each message. 27 messages parse and re-serialize
    byte for byte, 9 are rejected structurally, 7 fail in the header the RFC names, and 6 pass
    parsing and are caught by validation — plus the converse assertion that no valid message
    is rejected.
  - One parser serving datagram and stream framing, asserted identical by splitting every
    corpus message at every byte offset.
  - Header injection made unrepresentable: no public constructor accepts unvalidated bytes,
    and hostnames are a newtype whose interior cannot be forged.
  - All four transaction FSMs, RFC 3261 §17 amended by RFC 6026, driven with no clock and no
    socket.
- **M2 — It talks.** Transports and the user agent:
  - One event loop per endpoint owning the transaction layer, timers and sockets, so nothing
    in the signalling path takes a lock.
  - UDP with `received`/`rport`, and TCP with a pool that distinguishes inbound from outbound
    connections — a response returns the way it came without an inbound connection becoming a
    route for unrelated outbound requests.
  - RFC 3263 selection with RFC 2782 weighting, asserted against a seeded distribution.
  - Digest authentication checked against the digest RFC 2617 publishes for its own example,
    and registration treated as a lease rather than a request.
  - **Verified against a real Kamailio**, not only against sipx: `./tests/interop/run.sh`
    registers over UDP and TCP, refreshes, is refused with a wrong password, and pings.
- **M3 — It calls.** SDP offer/answer as a pure function, G.711 checked against the ITU
  algorithm, RTP with a jitter buffer that treats the 16-bit sequence wrap as ordinary, media
  sessions with symmetric RTP, and dialogs. Two endpoints establish a call, play a WAV and
  record it back bit-exact after the codec.
  - The gaps this milestone left were closed before M4: RTCP (`M-6`), DTMF (`M-7`), re-INVITE
    (`M-8`) and a DNS client (`T-5`).
- **M4 — Phone.** `sipx dial | answer | register`, with WAV playback and recording, DTMF, a
  JSON output mode and an exit code per outcome. Two `sipx` processes place a call to each
  other and the recording contains the audio that was played.

- **M5 — Depth.** TLS, WebSocket and secure WebSocket; two-party bridging and N-party mixing;
  transfer (RFC 3515); an adaptive jitter buffer; RTCP statistics; Opus behind a feature; and a
  load generator.
  - Then the RFC gaps the [RFC roadmap](rfc-roadmap.md) ranked first: SRTP with SDES keying
    (`M-14`), session timers (`S-11`), `Path` (`T-14`), the modern digest algorithms (`S-14`),
    100rel and PRACK (`S-12`), a SIP-over-QUIC specification (`T-11`), the user guides (`X-12`)
    and the published API reference (`X-13`).

## Next

Three milestones, each independently demonstrable, each ordered by the same rule the
[RFC roadmap](rfc-roadmap.md) uses: **a gap that changes what sipx can be deployed as beats a gap
that adds a feature.**

### M6 — Registrable

*What a real deployment needs before it will route to this stack.*

M5 left sipx able to place and receive calls with encrypted media. What it cannot yet do is be
*reached* reliably: a registration that survives NAT, a route set for requests going out, and
keying a browser will accept.

| Story | RFC | Why it is in M6 |
|---|---|---|
| **T-16** Service-Route | 3608 | `Path` (`T-14`) fixed the inbound direction. Requests sipx *sends* still ignore the route set the registrar handed back, so they reach the proxy that has no state for them. |
| **T-15** Outbound | 5626 | `reg-id`, `+sip.instance` and a flow the client opened. Without it a registration behind NAT is only usable until the binding lapses, and re-registering does not fix it. |
| **M-15** DTLS-SRTP | 5764, 8842 | SDES (`M-14`) keys over the signalling path, which means every proxy on it has held the key. DTLS-SRTP does not, and it is the only keying a browser will accept. |

**Done when** sipx registers through a proxy chain behind NAT, is reached back down the flow it
opened, obeys the outbound route set the registrar dictated, and negotiates media keyed without
the signalling path ever carrying the key.

Three tracks, two crates each, no overlap: reachability owns `sipx-ua` and `sipx-transport`,
media owns `sipx-media` and `sipx-rtp`. T-16 before T-15 — both touch registration, and
Service-Route is the smaller half.

### M7 — Forwardable

*Making the API right for something that is not an endpoint.*

Six stories that share one shape: the interface is correct for a user agent and wrong for
anything that forwards. This is the layer the **edge epic** below sits on, and cutting it as its
own milestone is what stops "become a proxy" from being one enormous story.

| Story | What is wrong today |
|---|---|
| **T-19** Stop dropping incoming requests silently | A full channel loses a request with no counter and no log. That is a fault, not a missing feature, which is why it leads. |
| **T-18** Surface unmatched responses | The endpoint logs and drops a response that matched no client transaction — exactly what a forwarding element is required to forward. |
| **T-17** Resolve at proxy throughput | The `Resolver` trait is shaped for one UA: synchronous, one cache per caller. Resolving must not block the loop that is forwarding everything else. |
| **S-15** Header editing operations | `Headers` cannot remove-first, insert-at or retain-by-predicate, so changing one header in flight means rebuilding the collection by hand. |
| **S-16** Server-side digest | sipx can answer a challenge and not issue one. A registrar or proxy has to be the party that authenticates. |
| **X-14** Timer queue + loopback link | The two pieces of scheduling machinery every sans-IO driver needs, still private to the endpoint; and the lossy in-process link the testkit's docs already promise. |

**Done when** a message can be received, rewritten and forwarded with a shared resolver and a
challenge issued by sipx, with nothing silently lost — and the testkit can drive that against a
seeded lossy link with no sockets and no clock.

T-19 is a live fault and goes first; the rest are independent.

### M8 — Subscribable

*The event framework, and the packages that wait behind it.*

sipx implements exactly one subscription today: the implicit one a REFER creates. Everything in
the presence and busy-lamp family needs the general case, and it does not exist yet.

| Story | RFC | What it unlocks |
|---|---|---|
| **S-13** SUBSCRIBE/NOTIFY framework | 6665, 4488 | A subscription store with pluggable packages, refresh, fetch (`Expires: 0`), termination with a reason — and `Refer-Sub: false` to suppress the implicit one REFER creates. |
| **S-17** Dialog and registration event packages | 4235, 3680 | Busy-lamp fields, and watching a registration go stale. Both report state sipx already tracks, which is what makes them the right first packages. |
| **S-18** Presence and PUBLISH | 3856, 3863, 3903 | Presence with PIDF documents, and publishing state into the framework rather than only serving it out of local state. |

**Done when** two sipx endpoints run a subscription through refresh and termination, a watcher
sees a dialog change state and a registration expire, and a published presence document reaches a
subscriber.

S-13 first and alone — the other two are packages *on* it, and writing a package before the
framework would shape the framework around that one package.

### After M8

**QUIC** (`T-12` transport, `T-13` verified against a real peer) is specified in
[`docs/specs/sip-quic.md`](https://github.com/codewandler/sipx/blob/main/docs/specs/sip-quic.md)
and deliberately ranked below the published-RFC work; it is a draft, and a reasonable bet rather
than a prerequisite. **GRUU** (RFC 5627) and
**push** (RFC 8599) come free-ish once M6's Outbound lands. The **edge epic** is what M7 exists
to make possible, and it stays unscheduled until someone wants it.

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
