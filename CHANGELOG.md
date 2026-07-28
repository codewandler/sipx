# Changelog

All notable changes to sipx are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

**Closing the gaps M3 left**

- RFC 4733 DTMF (`M-7`). sipx had been advertising `telephone-event` in every offer since M3
  with nothing able to encode or decode one. The payload type is read from the negotiation
  rather than assumed, since it is dynamic.
- re-INVITE (`M-8`). A call can be renegotiated from either side, including hold and resume. A
  renegotiation that fails is refused with 488 and **leaves the call running**.
- A real DNS client behind the RFC 3263 resolver trait (`T-5`), with a TTL-respecting cache
  that tells "no such record" from "could not ask".
- RTCP sender and receiver reports (`M-6`), with interarrival jitter computed by RFC 3550's
  own recurrence rather than as a variance.

**Milestone M4 — the phone**

- `sipx dial`, `sipx answer` and `sipx register`, with WAV playback and recording, DTMF, a
  `--json` output mode and a distinct exit code per outcome.
- `sipx dial --timeout` bounds the attempt, and **CANCEL** (RFC 3261 §9), which the stack had
  never implemented. Giving up is not just ceasing to wait: without a CANCEL the callee goes
  on ringing, and someone answering afterwards ends up in a call with a party that has left.
  The bound lives in `sipx-call` rather than around it — dropping the call future would
  abandon the exchange after a 200 OK but before the ACK.

**Milestone M5 — depth** (in progress)

- `docs/specs/sip-tls.md` and **SIP over TLS** (`T-6`, `T-7`), with certificate verification
  that cannot be turned off. Trusting a private CA is an addition to the anchor set, not a
  bypass — there is no `insecure` flag, because every stack that ships one eventually finds it
  in production.
- **SIP over WebSocket** (`T-8`, RFC 7118), which is how a browser reaches a SIP network at
  all. The handshake negotiates the `sip` subprotocol and refuses a peer that does not offer
  it; one SIP message per WebSocket message, with anything else closing the connection rather
  than being patched up; and Ping keeps the path open through intermediaries that would
  otherwise close a registered client's socket.
- **Secure WebSocket** (`T-9`), which is the TLS above with the framing above on top — the same
  acceptor, the same connector, the same policy. Not a third set of security rules.
- **Interop for both against Kamailio** (`T-10`). The harness now issues its own certificate
  and asserts three things a fixture test cannot: that a registration over TLS is accepted by an
  implementation that did not learn TLS from sipx, that a certificate for another name is
  refused, and that an unknown issuer is refused. Both refusals must be *immediate* — a test
  that accepted a timeout would also pass against a stack that had simply hung. WebSocket
  registration is proved the same way, against Kamailio's own WebSocket module.

**Load and stability**

- **A load harness** (`X-4`) in `sipx-testkit`, generic over what a call is so it can be pointed
  at sipx or at somebody else's server — a limit found with sipx on both ends cannot be
  attributed to either half. Failures are reported **by cause**, never aggregated, and latency
  as **percentiles**, never a mean: setup latency is a tight cluster with a tail of
  retransmission timeouts, and a mean sits in the empty space between them.
- **A soak assertion** (`X-5`) that tasks, file descriptors and the transaction store are *flat*
  rather than merely bounded, run nightly in CI rather than on every push.

**Media**

- **Opus** (`M-13`, RFC 6716), behind the `opus` feature. Note the one exception in
  `deny.toml`: the FFI shim under it is unmaintained, there is no maintained alternative that
  encodes, and the advisory is excepted with its reasoning and its exit condition written down.
  — off by default, so the stack stays
  pure Rust unless the codec is asked for. Negotiated as a dynamic payload type matched by
  *encoding name*, so an endpoint that numbers Opus differently is still understood, and G.711
  remains the fallback. Being stateful, unlike G.711, it moved the codec into the send and
  receive loops, one each: a stateful codec that cost a lock in the packet path would not be
  worth having.
- **Conferencing** (`M-12`): every party hears every other party and never themselves. The
  mixer saturates rather than wrapping, because wrapping turns the loudest instant of a call —
  the moment two people talk over each other — into a full-scale discontinuity heard as a bang.
  Participants join and leave without interrupting the others.
- **Bridging two calls** (`M-11`). Audio is passed through without decoding when both legs
  agreed on the same codec, and transcoded — visibly, via `Bridge::is_transcoding` — when they
  did not. Dropping a bridge stops it, rather than leaving two tasks forwarding audio between
  calls nobody holds a handle to.
- **Call quality, readable while the call is running** (`M-10`): loss, jitter, round-trip time
  and an estimated MOS, from `MediaSession::quality()` and from `sipx dial --stats`. The round
  trip is computed from the RTCP exchange (RFC 3550 §6.4.1) and is *absent* rather than zero
  when the far end does not speak RTCP — zero would read as "instantaneous", and a script would
  believe it.
- **The media session now binds a control port** and receives RTCP, where before it could only
  send. It also sends **sender** reports once it has sent anything, rather than only receiver
  reports. Without both, no peer could ever have told sipx its round-trip time.
- **An adaptive jitter buffer** (`M-9`). Depth follows observed jitter between a floor and a
  ceiling, growing at the first packet that arrives too late and shrinking only after five
  seconds of clean network — because being too shallow is audible and being too deep is not.
  The fixed buffer remains, as the control: on a trace with recurring 95 ms spikes the constant
  loses 86 packets to lateness and the adaptive one loses 3, and on a clean trace the two behave
  identically. Used by default in `sipx-media`, bounded at 12 packets.

**Transfer**

- **Blind transfer** (`S-9`, RFC 3515). REFER is sent and received in-dialog, the transferee
  places the call, and the outcome comes back as NOTIFY — because a `202 Accepted` means "I will
  try" and nothing more. A transferor that read it as success would report a completed transfer
  to a user whose call had been refused. The implicit subscription is terminated when the
  transfer finishes, either way, rather than left open on both sides.
- **Attended transfer** (`S-10`, RFC 3891). `Replaces` is matched on `Call-ID` *and both tags*,
  and the check is inside `answer_replacing` rather than left to the caller. A `Call-ID` travels
  in every message of a dialog and is visible to every element on the path; the tags are random
  and known only to the two parties. Matching on the `Call-ID` alone would let anyone who had
  seen one message of a call ask to be put in the middle of it — so every mismatch is refused
  with the same 481, which also tells a guesser nothing about how close they got.
- `Call::handle` does not answer a REFER. Whether to place a call on another party's say-so is
  the application's decision, and `accept_referral`/`refuse_referral` are the two answers. A
  `Refer-To` naming nothing usable is the exception: 400, without asking.

### Changed

- **The connection pool keys connections by `(address, transport, verified identity)`**, where
  it used to key by address alone. Two names that resolve to one address are two connections:
  reusing one for the other would send traffic for `a.example.com` over a connection
  authenticated as `b.example.com`, discarding the check that had just been performed. The
  transport is in the key for a related reason — WebSocket and TCP can share a port, and a
  `sips:` request riding a cleartext socket has silently become what it asked not to be.
- `call::contact_for` takes the transport. Over a WebSocket there is no address to advertise,
  and in-dialog requests ignore `Contact` entirely — see the fix below.
- `TrustAnchors::system()` uses the **platform's** trust store rather than a copy of one
  vendor's root list compiled in — so an operator's corporate CA is honoured, and a root
  distrusted after a compromise stops being trusted when the OS says so.
- **The minimum supported Rust version is now 1.88**, raised from 1.85. The DNS client needed
  to clear RUSTSEC-2026-0119 requires it, and the alternative was shipping a known denial of
  service in a parser that reads untrusted network data.

### Security

- Upgraded the DNS client past **RUSTSEC-2026-0119**, a CPU-exhaustion denial of service in
  `hickory-proto`'s name compression. sipx feeds that parser untrusted network data, so this is
  on the path that matters. Caught by `cargo-deny` in CI on the first push after the dependency
  was added — which is the whole reason the gate exists.

### Fixed

**Conformance defects found by reviewing implemented behaviour against the RFCs** (`X-6`).
Deliberately not a gap analysis — a missing feature is visible, a subtly wrong one is not. Every
fix landed with a failing-first test, and the tests that asserted the old behaviour were
rewritten rather than deleted.

- **Timer B fired from `Proceeding`**, so a callee who took longer than 64·T1 to answer was hung
  up on, and `send_to_uri` then dialled the next RFC 3263 candidate while the first phone was
  still ringing. RFC 3261 §17.1.1.2 fires it from `Calling` only; §16.6 item 11 is explicit that
  the INVITE client transaction no longer times out once a provisional has arrived, which is
  precisely why proxies need Timer C.
- **A `sips:` URI with a `transport` parameter resolved to cleartext.** Table 1 and §26.2.2: in a
  SIPS URI the parameter names the transport carried *under* TLS, so `transport=tcp` asks for TLS
  over TCP. The scheme filter lived in the SRV stage, which an IP literal, an explicit port and
  the bare A-record fallback all skip. `sips` over UDP now yields no candidate rather than a
  downgrade, there being no TLS over UDP to offer.
- **RFC 3581 was broken in both halves.** `received` was omitted when the sent-by matched the
  source, though §4 requires it "even if it is identical to the value of the `sent-by`
  component"; and `rport` was consulted only alongside `received`, so a response went to the
  sent-by port a NAT had rewritten. A client on an ephemeral port never got its answers.
- **In-dialog requests carried the route set but were addressed to the remote target**, bypassing
  the record-routing proxy that inserted itself in the dialog in order to be traversed. Where
  that proxy is the only element that can reach the far end, this is the BYE that never arrives —
  with the media still running. §12.2.1.1, now including strict routing and the parameters
  §19.1.1 bars from a Request-URI.
- **The ACK to a 2xx ran inside a transaction**, earning it the retransmission timers of a
  non-INVITE request aimed at a response that never comes; and a *retransmitted* 2xx was never
  acknowledged again, though §13.2.2.4 requires an ACK for each one received.
- **The 200 to a re-INVITE was sent once.** §13.3.1.4 governs the 2xx to any INVITE, and RFC 6026
  has the server transaction absorb the retransmitted requests without answering them, so a
  single lost packet deadlocked hold and resume until the peer's Timer B.
- **§18.1.1's size limit was applied to responses**, which §18.2.2 gives a UAS no transport to
  escape to. A 200 carrying a full SDP answer was refused outright: the caller timed out while
  the callee believed it had answered.
- **A CRLF before a start-line was a fatal framing error**, so the RFC 5626 keepalives that
  mainstream stacks send routinely closed the connection and every dialog riding it. §7.5 makes
  ignoring them a MUST, and only for stream transports.
- **RTCP named both parties SSRC 0** — the report block never learned the peer's synchronisation
  source and the sender field carried the reportee's — so a conforming peer found no block
  matching itself and discarded every loss and jitter figure. Interarrival jitter also used
  non-modular arithmetic, so a 32-bit timestamp wrap (normal, since §5.1 randomises the starting
  timestamp) injected 2³²/16 into the estimate and poisoned it for hundreds of packets.
- **FQDNs in SDP `o=` and `c=` lines were rejected**, failing the whole description — including
  RFC 3264 §10.1's own example offer, which could therefore never be answered.
- **The digest nonce count was global rather than per-nonce**, so a registrar enforcing the replay
  protection that `nc` exists for rejected every fresh nonce answered with a count above one.
  RFC 7616 §3.4.3 counts requests sent *with the nonce in this request*.
- **`sipx register` advertised the registrar's address in its own `Via`** and, on the default
  `--local`, registered a `sip:user@0.0.0.0` binding — so every inbound call to the
  address-of-record was routed nowhere.
- Smaller ones, each with its citation in the story: comma-separated `Contact`/`Route` rows
  rejected (§7.3), case-sensitive SIP-Version (§7.1), `tel:` URIs compared as opaque bytes,
  escaped parameter names not folded (§19.1.4), no target refresh on a re-INVITE (§12.2.2), the
  ordering check applied only to re-INVITEs so a stale BYE ended a live call, `answer()`
  committing a 2xx before it knew a dialog could be formed, weight-0 SRV records unreachable
  (RFC 2782), TLS advertising the cleartext port in `Via`, session-level SDP direction ignored
  and rtpmap matched without its clock rate (RFC 3264 §6.1), DTMF fed to the jitter estimator and
  saturating instead of segmenting (RFC 4733 §2.5.1.3), and UAS final responses without a To tag
  (§8.2.6.2).

- A call hung up while packets were still in the paced send queue, so every call lost its last
  word — or, for DTMF, its last digit. `MediaSession::flush` now drains the queue first.
- The RTCP report block decoder read cumulative loss from byte 4 instead of byte 5, folding the
  loss fraction into the high byte of the count.
- **A `sips:` URI resolved through DNS had its certificate checked against the resolved
  address.** RFC 3263 turns one name into a list of addresses by way of NAPTR and SRV records
  that may name something else entirely, and resolution never attached the name from the URI to
  what it produced. The handshake still succeeded and the check still appeared to run, which is
  the whole failure mode `docs/specs/sip-tls.md` §3.3 exists to prevent: whoever can influence
  DNS chooses which certificate is acceptable. Found while building WSS on top of it.
- **An in-dialog request over a WebSocket was sent to the peer's `Contact`.** A WebSocket client
  has no listening port, so its `Contact` names something that will never resolve (RFC 7118
  §5.2) — every ACK and BYE went nowhere. In-dialog requests now go over the connection the
  dialog was established on, unconditionally, and sipx writes an unresolvable `Contact` of its
  own when it is the WebSocket client.
- **The crate did not compile with the `tls` feature disabled.** `tokio::select!` cannot compile
  a branch out behind a `#[cfg]`, so each optional listener's branch referred to a field that
  was not there. CI only ever built `--all-features`, so nothing noticed. Every optional
  listener now shares one channel and one branch, and each feature combination is checked.
- **A server transaction the application never answered was held for the life of the process.**
  RFC 3261 §17.2 gives one in `Trying` no timer, because its model is that the transaction user
  always responds; an application that ignores a method it does not implement, or that panics in
  a handler, leaves it there and nothing collects it. Found by the new soak run — 300 of them
  for 300 calls, still present two minutes later. The endpoint now abandons one unanswered after
  three minutes and logs it as the application bug it is.
- **A URI carrying the same header name twice was not equivalent to itself.** Each occurrence
  was compared against the *first* header of that name rather than its counterpart, so
  `sip:a?f=a&f=b` failed reflexivity. Headers are now compared as multisets. Found by a
  property test, which is exactly the kind of bug no example test would have reached.
- **`Handle::respond` returned before the response was sent.** It queued a command for the
  endpoint loop and returned, so a process that answered a call and exited could lose the
  response to its own exit — the caller then saw a timeout for a call that had in fact been
  answered or refused. It now returns once the response is on the wire, which is what every
  caller already assumed. Found by a CI-only failure of the `--busy` test.
- **A received CANCEL was absorbed as an INVITE retransmission.** The transaction key folded
  CANCEL to INVITE, but RFC 3261 §17.2.3 folds the method only for ACK — so a CANCEL matched
  the INVITE's own transaction, was swallowed as a duplicate, and nobody was told. Nothing
  could have stopped a ringing phone.
- The DNS client's own response cache is now disabled. Two caches with different TTL policies
  is a source of confusion rather than speed: sipx's exists to cap TTLs and to distinguish
  "no such record" from "could not ask", and neither survives a second layer underneath doing
  its own thing.

## [0.1.0] — 2026-07-28

The first cut. Not published anywhere: no crate is on crates.io and no tag has been pushed.
What this marks is the point at which the bottom four layers of the stack are complete and
verified — a SIP core, transports, a user agent and calls that carry audio.

sipx registers against a real Kamailio over UDP and TCP, answers `OPTIONS`, and places a call
between two of its own endpoints that carries G.711 in both directions. 349 tests, clippy clean
at `-D warnings`, and the whole RFC 4475 torture corpus green.

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

Defects found and fixed before this release — nothing here ever reached a user. They are
recorded because each one is a mistake worth not repeating, and most of them sat directly
beneath a comment asserting the opposite.

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

### Not in this release

Stated so nobody has to discover it from a stack trace:

- **No TLS, WebSocket or WSS.** The transport enum names them; only UDP and TCP are
  implemented, and a `sips:` URI resolves to no candidate rather than downgrading.
- **No DNS client.** Every RFC 3263 selection rule is implemented and tested, but the only
  `Resolver` implementations are test fixtures, so a URI naming a domain resolves to nothing at
  runtime. IP literals and explicit `host:port` work today (`T-5`).
- **No re-INVITE.** A call can be established and ended, not modified (`M-8`).
- **No RTCP** (`M-6`) and **no RFC 4733 DTMF** (`M-7`) — the latter matters because the SDP
  already advertises `telephone-event`, so that advertisement is currently a promise sipx does
  not keep.
- **No command-line tool.** `sipx-cli` is a scaffold; `dial`, `answer` and `register` are
  library calls only (milestone M4).
- **Interop is verified against Kamailio only.** A second implementation with different
  opinions — Asterisk, as a B2BUA rather than a proxy — has not been tried.

[Unreleased]: https://github.com/codewandler/sipx/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/codewandler/sipx/releases/tag/v0.1.0
