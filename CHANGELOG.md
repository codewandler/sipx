# Changelog

All notable changes to sipx are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **A call reports what happens to it as a typed event stream (`C-3`).** A `Call` was only visible
  by calling methods on it at the right moment — `is_on_hold`, `is_ended`, `transfer` — which meant
  a host had to know when to look. Every state change is now also pushed once onto a channel the
  call owns: ringing (with whether the provisional was reliable), answered, a DTMF digit and how
  long it was held, playback and recording finishing, an inbound REFER with its target, transfer
  progress, hold and resume by the far end, and ended with a cause.
  - **`Call::events` hands out the receiver exactly once** and returns `None` after — one consumer
    by construction rather than by convention.
  - **The overflow policy is the part worth knowing.** The channel holds 32, and one slot is
    reserved for `Ended` at construction, before any ordinary event can claim it. Ordinary events
    are dropped rather than queued when the consumer is behind — each carries a snapshot, so a
    consumer that missed one resynchronises from the next. `Ended` is not like that: it is a call's
    last word, and a consumer that never learns a call ended waits forever. So it gets a reserved
    slot rather than a policy, and nothing on the ending path awaits the channel having room. A
    consumer that never reads at all cannot stall a call's teardown, which is tested directly.
  - Events are emitted where the state changes rather than reconstructed afterwards, so the stream
    cannot disagree with the call, and `dial` and `serve` go through the same path. No clock reads
    were added to `sipx-call`.
- **`Call::play` and `Call::record_until_idle`**, which report completion on that stream.
  `PlaybackFinished` carries whether the clip ran to the end or the call cut it off — "the
  announcement finished" and "the caller hung up during it" lead somewhere different, and one flag
  is what keeps them apart. `RecordingFinished` carries how much audio was captured, measured from
  the samples and the negotiated clock rate rather than from how long this side waited: counting
  the idle timeout would describe our own patience rather than the recording.

### Changed

- **`MediaSession::play` returns whether the clip reached the end** instead of `()`. A playback cut
  off by the session stopping was previously indistinguishable from one that finished.
- **`MediaSession::samples_per_packet()` is public.** Callers were passing a literal `160`, which is
  only right for an 8 kHz codec; `Call::play` uses the session's own packet size instead.
- `MediaSession::recv_digit` yields the digit **and how long it was held**, taken from the RFC 4733
  event's own duration field rather than from timing its arrival — the event carries the sender's
  clock, and measuring anything else would make the number depend on jitter rather than on how long
  the key was down. `Call::recv_digit` still yields just the digit.

## [0.5.0] — 2026-07-29

### Added

- **The event notification framework (`S-13`, RFC 6665).** sipx had exactly one subscription: the
  implicit one a REFER creates. Now there is a notifier with a subscription store — establish,
  refresh, unsubscribe, expire, terminate — and packages that register by name.
  - **A terminated subscription stays terminated.** It produces no further notification and cannot
    be refreshed back to life; a subscriber that wants another one starts a new dialog. Terminating
    is not forgetting, either: it stays findable until swept, so a NOTIFY crossing it finds a
    subscription that is *over* rather than one that never existed.
  - The identity is the dialog **and** the package, because §4.4.1 allows several subscriptions in
    one dialog when their `Event` differs — keying on the dialog alone lets the second silently
    replace the first.
  - An unserved package is refused `489 Bad Event` by name, rather than accepted and never
    notified, which a subscriber cannot tell from a slow notifier.
- **`Refer-Sub: false` (RFC 4488)** suppresses the implicit subscription — and needs *both* sides
  to say so. §3 makes it a request and an agreement; a transferor that assumed agreement would stop
  watching for notifications the transferee is still sending.
- **The dialog and registration event packages (`S-17`, RFC 4235 and RFC 3680).** What a busy-lamp
  field on a desk phone actually subscribes to: `dialog-info` documents carrying the five states of
  RFC 4235 §3.7.1, and `reginfo` documents carrying per-contact state with the event that changed
  it.
  - **The first document is `full` and the rest are `partial`.** A watcher that joined mid-call is
    given the whole picture once and told about changes after that; sending only changes from the
    start leaves it inferring a state nobody ever described.
  - **The version counter is scoped per subscription, not per resource.** Two watchers of the same
    dialogs each count from zero — sharing a counter would make one of them see gaps it cannot
    explain. It saturates rather than wraps, because a counter returning to zero looks like a new
    subscription.
  - `expired` and `unregistered` are kept apart, and so are `early` and `confirmed`. Both pairs mean
    roughly one thing to a state machine and two different things to a display: "lost its
    connection" reads differently from "logged out", and a lamp that lights on `early` lights while
    the phone is still ringing.
  - XML metacharacters are escaped. A SIP URI can carry `&` in its parameters, and one unescaped
    makes the whole document unparseable — a watcher then sees nothing at all rather than a
    slightly wrong dialog.
- **Presence, and publishing it (`S-18`, RFC 3856, RFC 3863, RFC 3903).** Nothing in a SIP stack
  knows whether a person is at their desk, so this is the half that lets somebody who does know put
  it in: PUBLISH creates soft state, an entity tag identifies it, and a subscriber to the `presence`
  package is told when it changes. PIDF is a typed document rather than a string template.
  - **A fresh `SIP-ETag` on every acceptance, a refresh included** (RFC 3903 §6 step 6) — which is
    what makes the tag mean anything: a publisher that kept its old one is refused next time.
    Without tags at all, two publishers for one resource overwrite each other and neither can tell.
  - **412 for a tag the compositor does not hold**, including one whose state expired while the
    publisher was not looking. Accepting that refresh as a new publication would resurrect a
    document the server had already forgotten and that nothing has re-sent. Expiry is judged on the
    clock rather than on whether a sweep has run, so the answer is not a race.
  - The three operations are read from what is present (§4.1) rather than dispatched by the caller:
    a tag with no body is a refresh, with a body a modify, with `Expires: 0` a removal.
  - Presence is `open` or `closed` and nothing else (RFC 3863 §4.1.3). The vocabulary people expect
    — busy, away, on the phone — is RFC 4480's, a different document; inventing tokens here would
    put values in a namespace that does not define them.
  - Composition policy is deliberately absent: a second publication for one presentity replaces the
    first. Merging several publishers' documents is a policy question, and a policy belongs to
    whoever has one.

  Both stories stop at the same line, and on purpose: the packages produce documents, and wiring
  them to sipx's *live* dialog store and registration lease is the application's join. A package
  that reached into the call layer would make `sipx-ua` depend on `sipx-call` and reverse the
  dependency direction the workspace is built on.

### Changed

- The implicit REFER subscription reads `Subscription-State` through the event framework instead of
  parsing it a second time. Two parsers for one header eventually disagree about whether a transfer
  has finished.

## [0.4.0] — 2026-07-29

### Added

- **The loopback link the testkit has always promised (`X-14`).** Two full stacks talk in one
  process with no sockets, over a link with seeded loss, duplication, latency and jitter. The same
  seed replays the same trace, so a failure found by varying the loss rate is one that can be
  re-run.
  - **Reordering is not a knob.** Packets overtake because one took longer than another, not
    because a path chose to shuffle them — so jitter produces reordering, and a separate
    probability would model the symptom and permit orderings no real path can produce.
- **The timer queue is generic over its key and no longer reads the clock** — `now` is an argument
  to `set`. It called `Instant::now()` internally, which made it unusable by any driver but the one
  it was written for, and made "when was this scheduled?" a question you could only answer by
  sleeping. Together with the link, a dropped INVITE and the Timer A retransmission that recovers
  from it now cost no wall-clock time at all.

- **A genuine negative DNS answer is cached (`T-17`, RFC 2308 §5).** It was not: an SOA-backed
  NXDOMAIN returned early and was re-queried every time. For a user agent that is one extra lookup
  per call; for a forwarding element resolving for every call it forwards, a domain with no
  `_sips._tcp` record was asked about thousands of times a minute. "Could not ask" is still
  deliberately *not* cached — remembering a network blip as a routing decision keeps a domain
  unreachable long after it has come back.
- `_sip._ws` and `_sips._wss` join the RFC 3263 prefetch, so a WebSocket destination no longer pays
  a serial lookup the other transports avoid.
- `dns::resolve_uri` resolves a URI to a candidate list in one await, for a caller that is not the
  endpoint loop.

### Changed

- A single-flight layer for concurrent identical DNS lookups was written and then **removed**:
  `hickory-resolver` already coalesces them, and the layer was measured to change nothing. The test
  that proves the property stays, so it is a checked fact about the dependency rather than an
  assumption.

- **sipx can issue a digest challenge, not only answer one (`S-16`, RFC 7616 / 8760).**
  `Authenticator` mints a nonce, emits `WWW-Authenticate` or `Proxy-Authenticate`, and verifies the
  credentials that come back. The credential store stays out: `verify` takes the password as an
  argument, so which credential a username maps to is the caller's business.
  - **Nonces are self-describing** — issue time plus an HMAC over it and the realm — so a server
    recognises its own nonce and its expiry without a table of every nonce it ever issued.
  - **A replay and a retransmission are told apart** by the response digest: the same count with
    the same digest is one request seen twice, which is ordinary over UDP and must still
    authenticate; the same count with a different digest is a captured credential.
  - The digest is checked before the clock, so a wrong password on an expired nonce is a rejection
    rather than `stale=true` — which would tell an attacker the only thing wrong with their guess
    was its timing.
  - SHA-256 by default. A server is the only party that can make that choice.

- **The digest primitives can be taken without a runtime (`X-20`).** `sipx-ua` depended on `tokio`
  and `sipx-transport` unconditionally, though only `agent`, `flows` and `error` need either — so
  the caller `S-16` was written for, a proxy or registrar whose decision logic touches no IO, could
  not use the authenticator without linking an async runtime into its core. Its alternative was to
  write digest a second time, and two implementations of one algorithm eventually disagree about
  who is authenticated. A default-on `runtime` feature now carries the two dependencies;
  `default-features = false` leaves `auth`, `challenge`, `outbound` and `registrar` with neither in
  the resolved graph. Nothing changes for anyone who does not ask.
  - The gate asserts on the **resolved dependency graph**, not on whether the build succeeds. A
    runtime-free `sipx-ua` that still pulled `tokio` would compile perfectly and deliver nothing,
    which is precisely the outcome a build check calls success.
  - `outbound::Flow` moved there from `agent`, where it had been sitting for no reason but history:
    it is a pair of the two identifiers `outbound` defines and needs no runtime to be one. `agent`
    re-exports it, so `agent::Flow` still resolves.

- **`Headers` can be edited, not only read (`S-15`).** `remove_first`, `insert` and `retain` — the
  three operations rewriting a message in flight needs. `Via`, `Route`, `Record-Route` and `Path`
  order *is* the routing, so these are exact positions rather than set operations.
  - `insert` past the end appends rather than panicking: this crate parses hostile input, and a
    panic on an index derived from it would be a remote denial of service reachable by arithmetic.
  - The transport's top-`Via` rewrite used to allocate a fresh `Headers` and clone every header to
    change one, on the received-path. It is now two operations that clone nothing.

- **Unmatched responses can be watched (`T-18`).** A response that matches no client transaction was
  logged and dropped — right for a user agent, wrong for anything that forwards: RFC 3261 §16.7
  step 1 requires a stateful proxy that finds no response context to forward the response
  statelessly, which it cannot do if it never sees one. `Handle::watch_unmatched` delivers them.
  - **Opt-in, and that is the design.** Widening `Incoming` into an enum would make every user agent
    handle a case it has no answer for; a second channel out of `bind` would change the signature
    for everyone. An endpoint nobody is watching allocates no channel and behaves exactly as before.

- **Backpressure is visible now (`T-19`).** The endpoint's delivery path ended in
  `let _ = try_send(…)`: a request the application could not take was gone, with nothing logged and
  no counter moved. `Handle::shed()` now reports what was dropped, and both paths log it.
  - **The counter is shared state, not a question asked of the event loop.** The loop is busy in
    exactly the situation this counts, so a metric readable only by asking it would be unavailable
    precisely when it is interesting.
  - **ACKs are counted apart**, because their consequence is different in kind. An ACK cannot be
    refused — SIP has no response to one, and an ACK for a 2xx is a transaction of its own with
    nothing to answer — so nothing retransmits it after Timer H and both ends are left in a dialog
    no timer reaps. A non-zero `ShedCounts::acks` means calls are leaking.

- **DTLS-SRTP (`M-15`, RFC 5764 / 5763 / 8122).** SDES (`M-14`) keys over the signalling path,
  which means every proxy on it has held the key. This keys on the *media* path: the two endpoints
  handshake there, derive SRTP keys from the DTLS master secret, and the SDP carries only a hash of
  the certificate that will appear. It is also the only keying a browser accepts.
  - **The fingerprint check is mandatory and happens where the TLS stack cannot see it.** RFC 8122
    §6.2 requires an endpoint whose peer's certificate does not match to stop; the certificate is
    self-signed, so there is no chain to validate, and what authenticates it arrived in the
    *signalling*. A mismatch yields an error rather than keys, and a peer that sent no fingerprint
    is refused before the handshake runs at all.
  - **Everything the RFC decides is compiled always** — `a=fingerprint`/`a=setup` negotiation,
    §5.1.2's demultiplexing of DTLS from RTP and STUN on one port, §4.2's key derivation. Only the
    handshake sits behind the new **off-by-default `dtls` feature**, which is where OpenSSL lives.
    The default build stays pure Rust.
  - MD5 and MD2 fingerprints are refused at the parser, which is where §5's prohibition on acting
    on them belongs; a digest whose length disagrees with the hash it names is refused too.
  - A session-level `a=fingerprint` is honoured as well as a media-level one — a browser sends only
    the former, and reading just the media level declines a perfectly good offer.

### Fixed

- Three specs linked to `designs/host.md`, which is named `app-host.md`, and the board's epic blurb
  carried a link relative to `docs/designs/` into `docs/stories/`. Both broke the docs build.

### Changed

- **The application host is a workspace crate — `crates/sipx-app` — not a separate product.**
  Reverses the placement 0.3.0 recorded: the contract, its interpreter and its host iterate
  together in one repository with one gate, and the separation's benefits are kept as ground
  rules instead (the host is a leaf no kernel crate depends on; its HTTP stack, serialization
  and future engine stop at its own `Cargo.toml`). The host's planning — designs
  (`app-host`, `embedded-runtime`, `ts-sdk`), four binding specs, and stories `A-1` … `A-7`
  under the new `app-host` epic — moves into `docs/`, and the crate exists as a documented
  stub so the name has its home from day one.

## [0.3.0] — 2026-07-28

### Added

- **The application contract, specified — `sipx.app.v1`** (`docs/specs/app-contract.md`), the
  epic behind it (`app-sdk`), and the six kernel stories it pulls: a call-level event stream
  (`C-3`), multi-call dispatch (`C-4`), the contract crate with its sans-IO interpreter (`C-5`),
  the bridge reachable from a `Call` (`C-6`), playback control (`M-17`) and mute (`M-18`).
  Events carry full call snapshots; instructions are ordered programs with correlated
  completion ids; a response replaces the pending program, which is what makes barge-in
  compose. Experimental until an inbound IVR and an outbound notifier both run against it.
- **Migration guides** — from Kamailio and from Asterisk — written as honest concept maps,
  each opening with a maps-today/not-yet table.
- **Outbound, the client half (`T-15`, RFC 5626).** A `Contact` naming an address behind a NAT is
  unroutable the moment the mapping lapses. Outbound routes down a *flow* the client opened
  instead: `+sip.instance` and `reg-id` on every REGISTER, `outbound` offered, `ob` on a
  dialog-forming `Contact`, and one registration per outbound proxy so that a proxy going away is
  survivable.
  - **`Flows::register` and `Flows::keepalive` return one outcome per flow and no aggregate
    `Result`.** Registering to several proxies exists so one failing is survivable, and a function
    returning a single `Result` cannot help but let one failure stand for all of them. The type is
    the guarantee.
  - **Keep-alives, both techniques.** CRLFCRLF/CRLF for connection-oriented flows (§4.4.1) and STUN
    Binding for UDP (§4.4.2), each over the flow it is testing — a ping on a second connection
    proves a flow nobody is using.
  - **A changed reflexive address is a failed flow**, even when every ping is answered (§4.4.2).
    That is the reason STUN is the UDP technique rather than an `OPTIONS`: the socket still works,
    but the mapping the registrar holds no longer reaches the UA, so a call routed down the flow
    would silently never arrive.
  - §4.5's backoff, with its asymmetric base — 30 seconds when every flow is down, 90 when one is
    still up. A UA that is reachable already has nothing to gain by hurrying.
- **A STUN Binding client (RFC 5389)**, scoped to what the keep-alive needs and no further.
  Decoding is checked against the vectors RFC 5769 publishes — including the 11-byte attribute whose
  padding a decoder must skip to find `XOR-MAPPED-ADDRESS` at all.
- `StreamParser::take_keepalives` counts the CRLFs RFC 3261 §7.5 tells a parser to ignore. It still
  ignores them; RFC 5626 §4.4.1 gives them a meaning, and a transport waiting for a pong has to be
  able to tell one arrived.

- **The registrar's outbound route set, obeyed — `Service-Route` (`T-16`, RFC 3608).** `Path`
  (`T-14`) fixed routing *toward* a UA. This is the other direction: a registration can dictate
  which proxies the UA's own requests must traverse, and a UA that ignores it sends every call
  straight at the destination — arriving at a proxy holding no state for the registration the call
  belongs to.
  - **An absent `Service-Route` clears the stored one.** §6.1's two sentences are one rule, and
    "nothing to say, keep what you had" is the natural mis-implementation: it leaves a UA routing
    through a proxy the registrar has stopped naming.
  - It is **not** attached behind the caller's back. `UserAgent::service_route()` hands it over and
    `DialOptions::with_service_route` takes it, because a `Route` header silently added to every
    request is close to undebuggable from outside.
  - A hop missing the `;lr` that §5 requires is *reported*, not dropped — the registrar is the
    offending party, and a UA that discarded a route set over a missing parameter would be
    unroutable for an invisible reason.

### Changed

- **The documentation site is customer-facing now** (`website/`, Docusaurus), and the internal
  tree under `docs/` is no longer published at all. The book's guarantee survives the move:
  every code sample is a compiled example file, inlined as a generated region the gate refuses
  to let drift (`scripts/sync-website.py --check`). The API reference stays at `/api`.
- `Outcome::Registered` carries a `Registered` struct rather than positional fields. `PathSet` and
  `ServiceRoute` are the same shape and opposite directions, and two interchangeable positions of
  identical type is how they would eventually get swapped.

- **Reliable provisional responses — 100rel and PRACK (`S-12`, RFC 3262).** A `180 Ringing` is
  fire-and-forget over UDP, and some carriers will not accept a call without the option tag at
  all. `100rel` is offered on every INVITE, honoured when a peer requires it, and refused with
  `420 Bad Extension` + `Unsupported: 100rel` when it is switched off locally — refusing plainly,
  because a caller waiting for an `RSeq` that never comes cannot tell that from a dead network.
  - The retransmission schedule doubles from T1 and **deliberately does not cap at T2**, which
    every other retransmission in SIP does. §3 gives the reason: an ACK is resent because a 2xx
    arrived again, but a PRACK is sent once and is not re-triggered by a further 1xx.
  - The `To` tag is chosen when the provisional is sent and reused by the answer. A reliable
    provisional establishes a dialog, so a fresh tag on the 200 would create a *second* one — the
    caller ACKs the dialog it knows while this side retransmits the 200 into a working call.
  - `RSeq` is chosen uniformly in `1..2^31-1` rather than sequentially: it is the only thing an
    off-path attacker would need to forge a PRACK and silence the retransmissions.

## [0.2.1] — 2026-07-28

Documentation and tooling only. **No crate changed**, so the libraries are byte-identical to
0.2.0; this release exists to mark the point where what sipx supports became something you can
check rather than something you have to take on trust.

### Added

- **[A documentation site](https://codewandler.github.io/sipx/)**, built from `docs/` rather
  than from a copy of it — a site with its own content tree is a second copy of the truth, and
  the second copy is the one that rots. `./scripts/build-docs.sh` builds it locally and fails if
  a published page links to something the site does not publish; it found eight such links on
  its first run.
- **An RFC compliance table**, generated from `docs/rfc/registry.toml` and verified in CI: a
  header an entry names must be known to the parser, a cited file must exist, and an entry
  claiming implementation must cite something. 61 RFCs — 22 implemented, 7 partial, 10
  parse-only, 21 not started, 1 superseded.
  *Parse-only is its own status.* sipx parses `RAck` and `RSeq` and does nothing with them, so
  "supports RFC 3262" and "rejects it" are both false, and a three-state table could not say so.
- **An [RFC roadmap](https://codewandler.github.io/sipx/rfc-roadmap.html)** ordering the
  remaining gaps by dependency and by what each changes about where sipx can be deployed.
- A logo: a crab holding a telephone handset.

### Changed

- **The README is for people deciding whether sipx fits**, not for contributors. It had been
  claiming the workspace was still being scaffolded. It now leads with what sipx can and cannot
  do — media is not encrypted, stated in the first table rather than buried — and `AGENTS.md`
  stays the file for contributors and agents.

## [0.2.0] — 2026-07-28

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
