# sipx — roadmap & status

The big picture: what's delivered, what's next, and the epics that group related stories. The
operational detail lives on the [board](https://github.com/codewandler/sipx/blob/main/docs/stories/README.md) (generated from story frontmatter);
this document is the hand-written narrative around it.

Two companions to this one: [**RFC compliance**](compliance.md) is the measured list of what
sipx supports, generated from a registry and checked in CI; [**the RFC roadmap**](rfc-roadmap.md)
is the order the remaining gaps close in and why.

## Status

_As of 2026-07-29:_ **M0 through M8 are complete.** `sipx-sip` is a working sans-IO SIP core:
URIs, headers, an incremental parser for both datagram and stream transports, message validation,
injection-proof builders, and all four transaction state machines with matching and stores. 941
tests pass; clippy is clean at `-D warnings` on both feature sets; the whole RFC 4475 torture
corpus is green across all four of its layers.

sipx registers against a real Kamailio over UDP, TCP and TLS and answers `OPTIONS`. Between two
sipx endpoints it places calls carrying G.711 audio in both directions, encrypted with SRTP when
the offer and answer agree on it — and `sipx dial | answer | register` does all of that from a
terminal. It is reachable behind NAT through a flow it opened, it can be the party that issues a
challenge rather than only the one that answers, and it serves subscriptions to what its dialogs
and registrations are doing.

Next is not a milestone but an epic: the **application SDK** and the **host** that runs it — the
question of what can be built on this stack without writing Rust. **M9** waits behind it.

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

- **M6 — Registrable.** What a real deployment needs before it will route to this stack: the
  outbound route set the registrar dictates (`T-16`, RFC 3608), a registration that survives NAT
  through a flow the client opened (`T-15`, RFC 5626), and DTLS-SRTP (`M-15`, RFC 5764) — keying
  that never puts the key on the signalling path, and the only keying a browser will accept.

- **M7 — Forwardable.** Six stories sharing one shape: an interface correct for a user agent and
  wrong for anything that forwards. Requests are no longer dropped silently when a channel is full
  (`T-19`), responses matching no client transaction reach the application (`T-18`), resolution is
  async with a shared cache (`T-17`), `Headers` can be edited in flight rather than rebuilt
  (`S-15`), sipx can *issue* a digest challenge and not only answer one (`S-16`), and the testkit
  ships the generic timer queue and seeded lossy loopback link its docs promised (`X-14`).

- **M8 — Subscribable.** The general case behind the one subscription REFER creates: a notifier
  with a subscription store, packages registered by name, refresh, fetch and termination with a
  reason (`S-13`, RFC 6665 and 4488) — and the three packages a desk phone subscribes to. `dialog`
  and `reg` (`S-17`, RFC 4235 and 3680) report state sipx already keeps; `presence` with PIDF and
  PUBLISH (`S-18`, RFC 3856, 3863, 3903) lets somebody who knows something sipx cannot observe
  publish it, held as soft state under an entity tag.
  - Where M8 stops: the packages produce documents, and joining them to a live dialog store or
    registration lease is the application's. A package reaching into the call layer would make
    `sipx-ua` depend on `sipx-call` and reverse the workspace's dependency direction.

## Next

Four milestones, each independently demonstrable, each ordered by the same rule the
[RFC roadmap](rfc-roadmap.md) uses: **a gap that changes what sipx can be deployed as beats a gap
that adds a feature.** M9 to M12 are defined and their stories are cut, not started; the
in-progress work is the [app-sdk](#application-sdk--app-sdk) and [app-host](#application-host--app-host)
epics below, which are not milestones because they are not RFC gaps.

### M9 — Bridgeable

*What has to be true of a session before sipx can sit between two of them.*

M7 makes a *message* forwardable. Nothing makes a *session* forwardable: `sipx-call` owns one
dialog and one media pipeline, and the media bridge `M-11` built has no signalling counterpart.
This is also where the [edge design](designs/edge.md)'s open question gets answered — whether a
B2BUA belongs in this repository. The product does not; the primitive does.

| Story | RFC | Why it is in M9 |
|---|---|---|
| **S-19** UPDATE | 3311 | The only way to change a session that has not been answered — §5.1 permits it "for both early and confirmed dialogs". Also what RFC 4028 §7.4 *recommends* a session refresh use when the peer allows it, which `S-11` could not do. |
| **C-2** Early media | 3960 | `S-12` built the early-dialog offer/answer and stopped there: sipx never puts a session description in a provisional, so a caller hears a locally generated tone where the callee sent audio. |
| **C-1** Two dialogs, one call | 7092 | An offer relayed leg to leg, a re-INVITE and a BYE propagated, and a bridge in between. §3.1.3 names the role sipx should be able to hold — signalling and SDP, off the media path — and §3.2.3 the one whose media half `M-11` already built. |

**Done when** a session is renegotiated before it is answered, a caller hears the callee's early
media rather than a local tone, and two dialogs are driven as one call by a single policy with
audio passing between them.

In that order, and the order *is* the argument. `C-1` has to relay an offer wherever it arrives —
in a provisional, a PRACK, an UPDATE or a re-INVITE. Taken first, it would mean writing the
early-dialog cases twice: once inside the coupling and once beside it.

### M10 — Reachable

*The three ways of being reached that M6 leaves open.*

M6 makes sipx registrable. It does not make one *instance* of a registration addressable, a
*sleeping* client callable, or a media path work where symmetric RTP cannot.

| Story | RFC | What it unlocks |
|---|---|---|
| **T-20** GRUU | 5627 | A URI that reaches one instance of a registered user. It needs the `+sip.instance` value `T-15` introduces, which is why `T-14` recorded it as gated on both. |
| **T-21** Push | 8599 | A client holding no connection at all: `pn-provider`, `pn-prid` and `pn-param` on the registered contact, and the binding-refresh REGISTER §4.1.3 requires when the push arrives. |
| **M-16** ICE | 8445, 8839 | The NAT cases symmetric RTP does not solve — and, with `M-15`'s DTLS-SRTP and M5's WSS, the last piece of a media path a browser can use. |

**Done when** one of two registrations of the same address of record can be called individually, a
push wakes a client that held no connection into an answered call, and a call passes audio between
two endpoints that symmetric RTP alone cannot connect.

`T-20` then `T-21`: both are registration work, and push builds on the same instance identity GRUU
needs. `M-16` is in different crates and can run beside them.

ICE is promoted here out of the [RFC roadmap](rfc-roadmap.md)'s last group, where it sat beside
recording. That was a mis-grouping — reaching the far end is not a feature, it is the same class of
gap as the two rows above it.

### M11 — Attestable

*What a peer network requires before it will carry the traffic.*

Everything up to here makes a call work. None of it makes a call *accountable*: sipx cannot prove
who placed it, say what happened to it on the way, or ask a neighbour to send less.

| Story | RFC | What it unlocks |
|---|---|---|
| **S-20** STIR and PASSporT | 8224, 8225 | A signed `Identity` header field, and a verification service that refuses a bad one with the code §6.2.2 names rather than a generic 400. Without it, a call handed to the public telephone network is unattested traffic. |
| **S-21** History-Info and Reason | 7044, 3326 | Who diverted a call and why. One story, not two: RFC 7044 §10.2 requires the `Reason` inside the `hi-targeted-to-uri`, and RFC 3326 is `syntax only` today precisely because nothing populates it. |
| **T-22** Overload control | 7339, 7415 | `oc`, `oc-algo`, `oc-validity` and `oc-seq` on the `Via`, so a loaded endpoint says how much to send instead of answering 503 — which is what `T-19` will otherwise leave it doing. |

**Done when** an outbound call carries an `Identity` header field an independent verifier accepts,
an inbound one whose signature does not verify is refused with 438, a diverted call arrives
carrying its diversion history with a reason per hop, and an overloaded endpoint publishes a rate
its neighbour honours.

`S-20` first and alone — it is the largest item here and the only one with a credential fetch and a
signature in it. `S-21` next, because the element that has to get a diversion history right is a
re-signing one, and M9 is what creates one. `T-22` is transport work and independent of both.

M11 sits after M10 because a call that cannot be reached has nothing to attest.

### M12 — Provable

*The measurement apparatus, caught up with the stack it measures.*

The north star is "correct under adversarial input and adversarial timing, provably". The apparatus
backing that claim — one torture corpus, four fuzz targets that are all parsers, one interop peer —
has not grown since M1, and the stack under it has.

| Story | What is missing |
|---|---|
| **X-16** The RFC 5118 corpus | The one published corpus sipx has never run: §4's IPv6 messages, nearly all of which must be *accepted* rather than rejected. A stack that classifies RFC 4475 to the layer has no excuse for skipping its IPv6 twin. |
| **X-17** A second interop peer | `tests/interop` proves sipx against exactly one implementation. One peer is a sample of one, and every quirk it happens not to have is a quirk sipx does not know about. |
| **X-18** Count what is discarded, capture what is sent | The stack emits `tracing` and nothing else: no counter leaves the process, and no capture can be attached to a bug report. `T-19` adds the first counter and has nowhere to put it. |
| **X-19** Fuzz the driver, not only the parser | `S-4` fuzzes bytes into the parser. Nothing fuzzes *event sequences* into the transaction layer, which is the half of the north star about adversarial **timing**. |

**Done when** the whole 5118 corpus is classified and green, the interop script runs against two
independent implementations, every discard in the signalling path is counted and exportable next to
a capture of the traffic that caused it, and a fuzzer drives the transaction layer with sequences of
timers and messages rather than with bytes.

Last, and for a reason that is not deprioritisation: each of these measures the stack as it stands
when it is written. Built before M9 to M11 they would certify a stack nobody will ship, and would
be extended three times. `X-16` first — a fixed corpus is the cheapest thing here.

### After M12

**QUIC** (`T-12` transport, `T-13` verified against a real peer) is specified in
[`docs/specs/sip-quic.md`](https://github.com/codewandler/sipx/blob/main/docs/specs/sip-quic.md)
and stays unscheduled on the same argument as before: there is no RFC for SIP over QUIC, so every
choice in that spec is ours, and that makes it a bet rather than a prerequisite. A bet does not get
a milestone — it lands in whichever one someone asks for it in.

What the compliance table still shows red after M12 is a list of features on roles sipx already
holds, and therefore last by this roadmap's own rule: SIPREC (7865, 7866), MESSAGE (3428), INFO
(6086), `tel` URI normalisation (3966), caller preferences (3841), the rest of the SRTP transform
set and rekeying (3711), RFC 5923's `alias` parameter, and the signed `Referred-By` token (3892).

## The v1 gate, and the alpha before it

**v1 is not a feature count.** The vision makes "maximum feature count" a non-goal and says plainly
that *a smaller stack whose every path is tested beats a larger one whose edges are guesswork*. So
the gate to 1.0 is not "M12 is done" and not "the compliance table is all green" — it is the north
star, made checkable: **correct under adversarial input and adversarial timing, provably, and
honest about what it does not do.**

A stack can be short of features and still be worth depending on. It cannot be *wrong about itself*
and be worth depending on, because every consumer's design decision rests on what the table says.

### `1.0.0-alpha` — the predicates

The alpha is the point at which a v1 **could** technically be cut. Each item is checkable by a
person reading the repo, and most are already checked by the gate.

1. **No claim outlives its caller.** No entry in [`docs/rfc/registry.toml`](rfc/registry.toml)
   claims a role that nothing above the implementing crate can reach, at *any* layer — not only
   `media`. `X-30` made this mechanical for media and deliberately went no further; `X-33`
   generalises it. Rows that cannot be made true are demoted, not explained.
2. **Adversarial input and adversarial timing are both fuzzed.** Four parser targets and the
   transaction-sequence driver (`X-19`), the second with an oracle that can fail without a panic.
   Met, subject to `X-31` closing the harness's own drift holes.
3. **A red gate means a defect.** No test in the workspace fails because the machine was busy.
   `X-28` cleared the media path; `X-29` is the rest. **This one is load-bearing for the others** —
   every predicate here is asserted by the gate, so a gate that cries wolf invalidates all of them.
4. **No known-wrong shipped path.** Every defect the suite or the fuzzer has found is fixed, or is
   an `#[ignore]`d regression test naming the story that will fix it. No silent deviation.
5. **The public API says what it guarantees.** Every published crate marks its surface stable or
   experimental. v1 freezes what "stable" means, so the line has to exist before it can be frozen.
6. **Testable from a shell** (principle 6) for everything the CLI exposes.
7. **The distance to v1 is generated, not asserted** — `X-32`, so this section cannot quietly go
   stale the way the Status block above did.

### What the alpha is *not* waiting for

M9–M12, the ICE and discovery epics, QUIC, and the red rows listed under "After M12". Those make
sipx do **more**; the alpha is about sipx being **right**, and about the table being a measurement
rather than a claim. They are v1's content, not its gate — and shipping an alpha is how the API
surface gets exercised before it is frozen.

**We stop at the alpha deliberately.** Cutting `1.0.0` means freezing the public API, and the API
has not yet been used by anyone outside this repository.

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
buffer and reception statistics; G.711, with Opus behind a feature; symmetric-RTP address
learning. Done when two sipx endpoints exchange audio that survives a bit-exactness check.
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

### Application SDK — `app-sdk`

What can be built on sipx **without writing Rust**. The contract layer: `sipx.app.v1`
([spec](specs/app-contract.md)) defines call events and call-control instructions as data, and a
sans-IO interpreter for them lands here (`C-5`) alongside the call-framework surface its effects
need — a typed event stream (`C-3`), multi-call dispatch (`C-4`), playback control (`M-17`),
mute (`M-18`), and the bridge reachable from a `Call` (`C-6`). None of it is scheduled into
M6–M8, whose crates this epic does not touch. Done when the spec's vectors pass sans-IO and an
example binary runs a canned program against a real call from a shell.
See [design](designs/app-sdk.md).

### Application host — `app-host`

The server the SDK's contract exists for, and the consumer that pulls the `app-sdk` stories:
**`crates/sipx-app`**, a leaf crate no other crate depends on. It executes handler programs on
real calls through three bindings of the one vocabulary — webhook documents, full-duplex
sessions, and an embedded TypeScript runtime — with per-app declared failure semantics and
deny-by-default capabilities. Four phases, each shell-demonstrable: one call one webhook
(`A-1` `A-2` `A-7`), session mode and the TypeScript SDK (`A-3` `A-4`), the embedded runtime
(`A-5` `A-6`), then operability. Done for phase 1 when a scripted webhook app answers a real
call placed by the CLI and the absent-app case does what its declaration says.
See [design](designs/app-host.md).

### ICE — `ice` _(six stories, M10)_

The media path where symmetric RTP cannot reach: candidate gathering, connectivity checks and
nomination, so two endpoints that never see each other's real addresses still exchange audio.
Cut from `M-16`, which was one story until it was specified and turned out to be three RFCs —
RFC 8445, RFC 8839, and RFC 8656 hiding inside "relayed candidates". The spec
([`specs/ice.md`](specs/ice.md)) was written first and is what the six children are measured
against.

The order is a dependency chain, not a preference. `M-19` (the RFC 8839 attributes) and `M-20`
(the STUN check codec) are independent and can run together; `M-21` (the sans-IO agent) needs
`M-20`; `M-22` (driving it on the media port) needs `M-19` and `M-21` and owns the test `M-16`
named; `M-23` (restart) and `M-24` (a relayed candidate) follow. ICE-lite is deferred with its
reason recorded — sipx is a UA behind NATs, which is the case lite does not serve — but
*interoperating* with a lite peer is not, because an implementation that only handles a full peer
hangs waiting for checks a lite peer is never required to send. See [design](designs/media.md).

### Endpoint discovery — `discovery` _(four stories)_

sipx can call any endpoint you can already name, and cannot help you name one: `dial` takes a URI,
and nothing answers "who is there to call?". The epic closes the front-door gap with one command
that lists what can be called and a `dial` that accepts what the list prints. Three sources, each
answering a different question — a local peer book (`P-5`, no protocol at all), a registrar's
registration event package (`S-24`, RFC 3680, `partial` today), and the local link (`T-24`,
mDNS/DNS-SD, `blocked` on whether a second protocol earns its attack surface). Note that RFC 3263
is *resolution*, not enumeration: it does the last mile of a call already decided on, and composes
with this rather than competing with it. Done when a shell script runs `sipx peers`, takes a name
from the output and places a call, with no URI written in the script. Discovery stops at naming —
the moment a lookup is consulted while routing someone else's INVITE, that is a dial plan, and the
[vision](vision.md) says those are built *with* sipx. See [design](designs/discovery.md).

### Edge / B2BUA — `edge` _(one story, in M9)_

The design's open question — whether a programmable edge belongs in this repository, or in a
separate product consuming `sipx-call` as a library — is answered: separate. What stays here is the
primitive underneath it, two dialogs driven as one call (`C-1`, M9). Transports, endpoints, routes,
a registrar and session-border policy are a thing built *with* sipx, which is what the
[vision](vision.md) already says about routing engines. See [design](designs/edge.md).
