# sipx — roadmap & status

The big picture: what's delivered, what's next, and the epics that group related stories. The
operational detail lives on the [board](https://github.com/codewandler/sipx/blob/main/docs/stories/README.md) (generated from story frontmatter);
this document is the hand-written narrative around it.

Two companions to this one: [**RFC compliance**](compliance.md) is the measured list of what
sipx supports, generated from a registry and checked in CI; [**the RFC roadmap**](rfc-roadmap.md)
is the order the remaining gaps close in and why.

## Status

_As of 2026-07-28:_ **M0 through M5 are complete.** `sipx-sip` is a working sans-IO SIP core:
URIs, headers, an incremental parser for both datagram and stream transports, message
validation, injection-proof builders, and all four transaction state machines with matching
and stores. 157 tests pass; clippy is clean at `-D warnings`; the whole RFC 4475 torture
corpus is green across all four of its layers.

sipx registers against a real Kamailio over both UDP and TCP, answers `OPTIONS`, and places
calls that carry G.711 audio in both directions, and `sipx dial | answer | register` does all
of that from a terminal. Next is **M5**: TLS and WebSocket, bridging, transfer and load
testing.

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

## What to work on next

Ten items, ordered by the [RFC roadmap](rfc-roadmap.md) and grouped so several can run at once.
The grouping is the point: the constraint is not how much there is to do, it is how much of it
touches the same files.

| # | Story | Track | Crates it touches |
|---|---|---|---|
| 1 | **M-14** Encrypt the media | media | `sipx-rtp` `sipx-sdp` `sipx-media` |
| 2 | **X-12** Write the user-facing guides | docs | none |
| 3 | **S-11** Implement session timers | signalling | `sipx-call` `sipx-sip` |
| 4 | **T-14** Implement the Path header | reachability | `sipx-sip` `sipx-ua` |
| 5 | **S-14** Add the modern digest algorithms | auth | `sipx-ua` |
| 6 | **X-13** Publish the API documentation | docs | none |
| 7 | **S-12** Reliable provisional responses | signalling | `sipx-call` `sipx-sip` |
| 8 | **T-11** Specify SIP over QUIC | quic | none (a spec) |
| 9 | **T-15** Outbound — *blocked by T-14* | reachability | `sipx-transport` `sipx-ua` |
| 10 | **M-15** DTLS-SRTP — *blocked by M-14* | media | `sipx-media` |

### Five tracks that do not collide

**Media** (M-14 → M-15) owns `sipx-rtp`, `sipx-sdp` and `sipx-media`. **Signalling** (S-11 → S-12)
owns `sipx-call` and the transaction layer. **Reachability** (T-14 → T-15) owns registration and
`sipx-transport`. **Auth** (S-14) is one crate on its own. **Docs** (X-12, X-13) touches no crate
at all.

Those five can be worked simultaneously. What cannot: S-11 and S-12 are one track rather than
two, because both rewrite the same parts of `sipx-call` and would spend more time merging than
implementing. The same is true of T-14 and T-15, and of M-14 and M-15 — and those two pairs are
strictly ordered anyway, since each second half needs the first.

### Why in this order

**M-14 is first because it is the sentence that disqualifies the stack.** `sips:` and WSS work
today, the TLS policy has no way to disable verification anywhere, and then the audio goes out
in the clear. Nothing else on the list changes as much about where sipx can be deployed.

**X-12 is second because it costs nothing to run alongside** and the README now points at a site
that has a landing page and no guides.

**S-11 before S-12** because a call whose far end vanishes without a BYE currently stays up in
sipx forever, and that is a fault rather than a missing feature.

**T-14 before everything else in its track** because `Path` is not known to the parser at all,
and Outbound, GRUU and push all sit behind it.

**T-11 sits below the RFC work** deliberately. SIP over QUIC is a draft rather than a published
RFC; it is a reasonable bet and not a reasonable thing to do before media is encrypted. That is
a re-ranking of the epic as seeded, and it is easy to reverse if the bet is meant to be taken
sooner.

Not in the ten: **S-13** (the event framework) is large and gates presence and busy-lamp fields,
but nothing currently in the stack is waiting on it; **T-12** and **T-13** are behind T-11.

## Next

Milestones, in order. Each is independently demonstrable.

The gaps M3 left — DTMF, re-INVITE, a DNS backend and RTCP — were closed before M4 began, so
nothing the stack advertises is left unimplemented.

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
