# Changelog

All notable changes to sipx are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

**Milestone M0 — workspace**

- Cargo workspace with the ten `sipx-*` crates, shared lints (`unsafe_code = "forbid"`)
  and `MIT OR Apache-2.0` licensing.
- CI: rustfmt, clippy (`-D warnings`), tests, MSRV check, `cargo-deny`, a fuzz smoke run, and
  a provenance gate that fails rather than passing when unconfigured.

**Milestone M1 — the sans-IO SIP core (`sipx-sip`)**

- Specs first: `docs/specs/sip-message.md`, `sip-parser.md` and `sip-transaction.md`, with
  every normative statement either citing an RFC section or marked as a project decision with
  its rationale.
- The RFC 4475 torture corpus, recovered bit-exactly from that RFC's Appendix A archive by
  `scripts/import-rfc4475-corpus.sh` and classified by which layer must object to each
  message. Green across all four layers.
- `Uri`, `Host`/`HostName`, `HeaderName` and parameter lists, with RFC 3261 §19.1.4
  equivalence — deliberately *not* `PartialEq`, since that relation is not transitive.
- A zero-copy message model: parsed messages borrow their bytes and re-serialize byte for
  byte, including original spelling, compact forms, whitespace and line folding.
- One parser for datagram and stream framing, verified identical by splitting every corpus
  message at every byte offset. Fuzz targets for both, seeded from the corpus.
- Typed headers parsed on demand, distinguishing absent from present-and-malformed.
- Message validation returning a list of findings, with `Max-Forwards` marked repairable.
- Builders in which header injection is unrepresentable rather than validated against.
- All four transaction state machines (RFC 3261 §17, amended by RFC 6026), matching with the
  RFC 2543 fallback, and transaction stores with a leak test.

**Milestone M2 — transports and the user agent**

- `docs/specs/sip-transport.md`, settling the connection-reuse and backpressure decisions.
- One event loop per endpoint owning the transaction layer, the timer queue and the sockets;
  no locks in the signalling path.
- UDP with `received`/`rport` (RFC 3581), and the RFC 3261 §18.1.1 datagram size guard.
- TCP with per-connection stream framing and a pool that distinguishes inbound from outbound
  connections, so a response returns the way it came without an inbound connection becoming a
  route for unrelated outbound requests.
- RFC 3263 resolution — NAPTR, SRV with RFC 2782 weighting, A/AAAA — behind a trait, with a
  seeded RNG so the weighted distribution is asserted rather than assumed. No DNS client is
  wired in yet; see `T-5`.
- Digest authentication (RFC 7616): MD5, MD5-sess, SHA-256, SHA-256-sess, verified against
  the digest RFC 2617 publishes for its own worked example.
- Registration as a lease: the registrar's granted expiry wins, refreshes reuse the `Call-ID`
  and advance the `CSeq`, and a rejected password fails once instead of looping.
- `OPTIONS` answered with a real capability list.
- Verified against a real Kamailio, not only against sipx: `./tests/interop/run.sh`.

**Milestone M3 — media and calls**

- `sipx-sdp`: RFC 8866 parsing that keeps unknown lines, and RFC 3264 offer/answer as a pure
  function. Rejected streams keep their place with port 0, codec order is the offerer's, and
  dynamic payload types are matched by encoding name rather than number.
- `sipx-audio`: G.711 µ-law and A-law checked against the ITU algorithm rather than by round
  trip, and WAV for 8 kHz 16-bit mono.
- `sipx-rtp`: packet encode/decode that rejects rather than guesses, and a jitter buffer that
  extends sequence numbers to 64 bits so the 16-bit wrap is ordinary rather than a cliff.
- `sipx-media`: RTP sessions with symmetric RTP, paced by a single clock.
- `sipx-call`: dialogs, `dial`, `answer` and `hang_up`. Two sipx endpoints establish a call,
  play a WAV and record it bit-exact after G.711.

### Fixed

- **A 2xx was not retransmitted until acknowledged.** The transaction layer absorbs
  retransmitted requests but does not resend the response; over UDP one lost 200 OK left the
  caller giving up while the answering side held an established call.
- **A 2xx the caller could not use was never acknowledged.** A 200 OK carrying an unusable SDP
  answer made `dial` return an error without an ACK, leaving the far end retransmitting for 32
  seconds and then streaming media at a closed port. It now ACKs and BYEs, per RFC 3261 §15.
- **ACK and BYE went to the address the INVITE was sent to** rather than to the peer's
  `Contact`, so with a redirect or a B2BUA in the path they reached the wrong element.
- **The route set was computed and never sent.** No `Route` header was added to in-dialog
  requests, so a call through a Record-Routing proxy could not be ended.
- **`Record-Route` was read one line at a time**, though it is a comma-separated list header —
  so a UAC's reversal reversed lines rather than routes. A malformed first route also silently
  discarded every later one.
- **An inbound BYE reached nothing**, so the far end hanging up did not stop the local media.
- **A URI with its own parameters was truncated** when its header tag was stripped, producing
  an unterminated angle bracket the far end answers with 400.
- **The RTP timestamp advanced by the configured packet size** rather than the samples actually
  sent, so any other frame size built a timeline at the wrong rate.
- **Unknown RTP payload types were decoded as the negotiated codec.** sipx advertises
  `telephone-event` on 101, so a peer's DTMF was decoded as µ-law and heard as a click.
- **A media session could not be stopped while its consumer was not reading**, leaking the task
  and its socket for the life of the process.
- **A forged RTP packet could silence a call.** Any later packet with a different SSRC was
  admitted to the jitter buffer, where a high sequence number made every genuine packet late.
- **`Contact` carried the socket's local address** rather than the endpoint's advertised one,
  so an endpoint bound to `0.0.0.0` published an unroutable contact.
- An endpoint binding to port 0 could fail with `AddrInUse`: UDP and TCP have independent port
  spaces, so a port the OS handed out for UDP could already be held for TCP. Binding now
  retries for a port free on both, while a *named* port that is taken still fails honestly.
