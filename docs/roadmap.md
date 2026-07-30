# sipx — roadmap & status

The big picture: what's delivered, what's next, and the epics that group related stories. The
operational detail lives on the [board](https://github.com/codewandler/sipx/blob/main/docs/stories/README.md) (generated from story frontmatter);
this document is the hand-written narrative around it.

Two companions to this one: [**RFC compliance**](compliance.md) is the measured list of what
sipx supports, generated from a registry and checked in CI; [**the RFC roadmap**](rfc-roadmap.md)
is the order the remaining gaps close in and why.

## Status

_As of 2026-07-30:_ **all seven `1.0.0-alpha` predicates are met**, and `1.0.0-alpha` is cut.
[`maturity.md`](maturity.md) reports them, and it reports them as *computed* rather than asserted —
which is what predicate 7 was for. What this does **not** mean is that v1 is close: its first
predicate requires the alpha's seven to have held **across at least one release** rather than at the
moment one was cut, and the rest each need something outside this repository, chiefly a public API
used from outside it. See [the v1 gate](#the-v1-gate-and-the-alpha-before-it).

_As of 2026-07-29:_ **M0 through M8 are complete.** `sipx-sip` is a working sans-IO SIP core:
URIs, headers, an incremental parser for both datagram and stream transports, message validation,
injection-proof builders, and all four transaction state machines with matching and stores. Clippy
is clean at `-D warnings` on both feature sets, and the whole RFC 4475 torture corpus is green
across all four of its layers.

**This block carries no test count on purpose.** It said "941 tests pass" through four releases
that took the real number past 1300 — the same drift `X-22` fixed in the gate and `X-24` fixed in
the pool-key docs, and for the same reason: a number transcribed by hand into prose has no way to
be wrong out loud. Run `./scripts/gate.py` for the count that is true today. `X-32` generates the
rest of what this section keeps getting wrong.

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
that adds a feature.** M9 to M12 are defined and their stories are cut. M10's are nearly all done and
it is still not declared — [where M10 stands](#m10--reachable) says which clause of its exit criterion
is short of the demonstration it is written as — and M9, M11 and M12 are unstarted. The in-progress
work is the [app-sdk](#application-sdk--app-sdk) and [app-host](#application-host--app-host)
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
two endpoints that symmetric RTP alone cannot connect. **That sentence is M10's only exit criterion**
— if another part of this file appears to state a second one, this is the one that governs.

`T-20` then `T-21`: both are registration work, and push builds on the same instance identity GRUU
needs. `M-16` is in different crates and can run beside them.

ICE is promoted here out of the [RFC roadmap](rfc-roadmap.md)'s last group, where it sat beside
recording. That was a mis-grouping — reaching the far end is not a feature, it is the same class of
gap as the two rows above it.

**The third clause means *some* such endpoints, not *any* of them.** Host and server-reflexive
candidates connect many NAT pairs with no relay anywhere in the path; both ends behind symmetric NAT
are connected by neither, and that case is the one a relay exists for. Read as *any*, the clause puts
RFC 8656 inside M10 and the milestone cannot be reached without a TURN client. Read as *some*, M10 is
the milestone that puts a working ICE media path in the stack and the relayed candidate belongs to
the epic that owns it. **The second reading governs** (`X-50`), on three grounds:

1. **The clause is a demonstration, and the demonstration is of a nominated pair.** `M-27`'s
   `a_call_uses_a_nominated_pair_when_both_host_candidates_are_silent` makes each side's default,
   highest-priority host path a silent socket, so the only usable addresses are the lower-priority
   reflexive candidates and audio arriving proves a nominated pair carried it rather than symmetric
   RTP rescuing the call. That is a pair symmetric RTP alone cannot connect, reached without a relay.
2. **Both places that enumerate M10's content name RFC 8445 and 8839 and not 8656** — the table above,
   and group 2 of the [RFC roadmap](rfc-roadmap.md). The only text that ever put 8656 in M10 was the
   ICE epic's heading, and it got there by grouping every child of `M-16` under one milestone rather
   than by scoping one.
3. **This roadmap orders a deployability gap ahead of a feature.** Before ICE, sipx has no answer to
   NAT beyond symmetric RTP at all; after it, it has one that works for the common pairs and not for
   both-ends-symmetric. A relay widens the coverage of a capability M10 delivers rather than
   delivering a capability M10 lacks.

**So `M-24` is not an M10 story.** RFC 8656 stays in the [ICE epic](#ice--ice) and in no milestone —
work belonging to an epic and to no milestone is a shape this file already carries, in the epics
below and in [After M12](#after-m12) — and it lands in whichever milestone someone asks for it in.
What it buys is exactly the case the clause above excludes: both ends behind symmetric NAT, where no
candidate type but a relayed one reaches. Until it lands, sipx's ICE is host and server-reflexive,
and the [compliance table](compliance.md) says so — 8445 and 8839 are `partial`.

**Where M10 stands, 2026-07-30.** All three mechanisms are built, and the milestone is **not recorded
as reached**, because two of the three clauses are held by mechanism rather than by the demonstration
they are written as:

- **GRUU** — `T-20`, done. `a_request_to_a_gruu_reaches_the_instance_that_registered_it` shows a
  request addressed to one instance's GRUU recognised by that instance, and refused when it names the
  address of record or another instance's GRUU. It is an `OPTIONS` against one agent and a stub
  registrar, not two registrations of the same address of record each taking a call.
- **Push** — `T-21`, done. `a_push_wakes_a_client_that_refreshes_its_binding_before_the_invite` shows
  RFC 8599 §4.1.3's order: push, binding-refresh REGISTER, then the INVITE that could not have
  arrived any earlier. It stops when the INVITE arrives; nothing answers it.
- **ICE** — `M-19`…`M-23` and `M-27`, done; `M-24` out of scope per above. Demonstrated in full by
  the test named in ground 1. The table's `M-16` row is the epic's tracker, and a tracker stays open
  until its last child lands — including `M-24`. **`M-16`'s status is therefore not M10's**, and
  reading it as M10's is the same substitution the ICE heading used to make.

**The distance left is not TURN.** It is the first two clauses demonstrated as they are already
written — nothing added to them: two registrations of one address of record where a call placed at
one instance's GRUU is answered by that instance and not the other, and a pushed client that answers
the call it was woken for. Recording M10 as delivered before that exists would be the defect `X-30`,
`X-35` and `X-42` each found — a claim true of one reading of its evidence and false of the reading
a reader would take.

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

**How a predicate's state is read.** A story declares the predicate it bears on in its own
`predicate:` frontmatter field, and [`maturity.md`](maturity.md) reports a predicate as met when every
story declaring it is `done`. **File a defect against a predicate by setting that field** — there is
deliberately no list of predicate stories anywhere else, because the one that lived in
`scripts/maturity.py` drifted: three defects were filed against predicate 3 in one session, none was
added, and the report was one story from calling the alpha complete (`X-42`). A story may declare two
(`predicate: [3, 7]`) where a defect falsifies both.

1. **No claim outlives its caller.** No entry in [`docs/rfc/registry.toml`](rfc/registry.toml)
   claims a role that nothing above the implementing crate can reach, at *any* layer. `X-30` made
   this mechanical for `media`, `X-33` widened it to `security` and measured and *declined* the rest:
   a path check is satisfied by citing a file whose relevant branch is dead, and a syntactic caller
   check would be fitted to the three rows that motivated it. The honest closure is not a better
   check but a real application — `X-38` — after which the reachable-from-a-call surface is *defined*
   as what that application uses. Rows that cannot be made true are demoted, not explained.
2. **Adversarial input and adversarial timing are both fuzzed.** Four parser targets and the
   transaction-sequence driver (`X-19`), the second with an oracle that can fail without a panic.
   Met, subject to `X-31` closing the harness's own drift holes.
3. **A red gate means a defect**, and a green gate means there is none. No test in the workspace fails
   because the machine was busy, no step is red for something that is not a defect, and no step prints
   a defect and exits 0. **This one is load-bearing for the others** — every predicate here is asserted
   by the gate, so a gate that cries wolf invalidates all of them. Which stories are outstanding is in
   [`maturity.md`](maturity.md), not here: naming them in this prose is the same defect one document
   over, and it had already gone stale.
4. **No known-wrong shipped path.** Every defect the suite or the fuzzer has found is fixed, or is
   an `#[ignore]`d regression test naming the story that will fix it. No silent deviation.
5. **The public API says what it guarantees.** Every published crate marks its surface stable or
   experimental. v1 freezes what "stable" means, so the line has to exist before it can be frozen.
6. **Testable from a shell** (principle 6) for everything the CLI exposes.
7. **The distance to v1 is generated, not asserted** — `X-32`, so this section cannot quietly go
   stale the way the Status block above did.

### `1.0.0` — the predicates

The alpha is the point where a v1 *could* be cut. These are what would make it right to actually cut
one, and they are separate because every one of them needs something this repository cannot supply on
its own. Each is checkable, for the same reason the alpha predicates are: a prose paragraph is not a
definition.

1. **Every alpha predicate above holds**, and has held across at least one release rather than only at
   the moment of measurement.
2. **Reachability is bound to callers at every layer.** No layer in
   [`maturity.md`](maturity.md) carries the "unverified against callers" caveat — that is `X-37`, and
   until it lands `implemented` outside `media` and `security` means "the code exists".
3. **The public API has been used from outside this repository**, by at least one application nobody
   here wrote. This is the one the roadmap has always given as the reason to wait, and it is not
   something a gate can assert: it is recorded when it happens, with what broke.
4. **Every published crate's contract is stated and has survived a breaking change being refused.**
   `A-8` states them; v1 needs at least one instance of a change being shaped by the contract rather
   than the contract being edited to fit the change.
5. **Interop passes against two independent implementations for every transport the README claims.**
   Today it is two peers, and not across every transport — the count is in `tests/interop/`, not here,
   so this is read from there rather than restated.

**What is deliberately absent from that list**: any feature count, any RFC total, any percentage. The
vision makes maximum feature count a non-goal and says a smaller stack whose every path is tested
beats a larger one whose edges are guesswork. A v1 gate built on coverage would contradict the
document it is supposed to serve.

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

### Bounded transport lifetimes — `bounded-transports`

The resource-safety repair for the running transport stack: pool eviction closes the task and socket
it evicts (`T-25`), incomplete unauthenticated handshakes have concurrency and time budgets (`T-26`),
and unusable endpoint capacities or keepalive intervals are rejected before binding (`T-27`). The
common invariant is that a configured limit bounds live resources, not merely entries in a map after
tasks have detached from it. Done when churn and partial-handshake tests observe socket and task
termination across TCP, TLS, WebSocket and secure WebSocket endpoints. See
[design](designs/bounded-transports.md).

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

### Media runtime safety — `media-runtime-safety`

The lifecycle and construction boundary around media workers: dropping a conference stops every
participant collector (`M-35`), zero packet/report/mix intervals are rejected before work starts
(`M-36`), and codec construction can never substitute a different wire codec under a negotiated
payload type (`M-37`). Done when invalid setup starts no worker, conference destruction retains no
participant session, and packet-level tests prove every active payload type uses its negotiated codec.
See [design](designs/media-runtime-safety.md).

### Call framework — `call`

What applications actually program against: answer and dial, playback, recording, DTMF,
two-party bridging, N-party mixing, and transfer (RFC 3515). Done when a bridged call passes
audio and DTMF in both directions with no shared mutable session.
See [design](designs/call.md).

### Phone CLI — `phone`

The `sipx` diagnostic endpoint: dial, answer, register, interactive scenarios and bounded load,
with all five released signalling transports, explicit codec/security/ICE policy, and media sourced
from files, devices or deterministic generators. `P-8` … `P-13` turn lower-layer capability into a
shell-reachable product surface; `A-10` publishes the resulting Linux binaries after `A-9` and the
v1 predicates hold. Done when the [diagnostic-phone specification](specs/diagnostic-phone.md)'s
matrix is reproducible from a shell and records what actually negotiated. See
[design](designs/phone.md).

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

### ICE — `ice`

The media path where symmetric RTP cannot reach: candidate gathering, connectivity checks and
nomination, so two endpoints that never see each other's real addresses still exchange audio.
Cut from `M-16`, which was one story until it was specified and turned out to be three RFCs —
RFC 8445, RFC 8839, and RFC 8656 hiding inside "relayed candidates". The spec
([`specs/ice.md`](specs/ice.md)) was written first and is what the children are measured against.

**The epic is not a milestone, and this heading no longer names one.** It read *(six stories, M10)*,
which scoped [M10](#m10--reachable) to every child of `M-16` — including `M-24`'s relay — while M10's
own **Done when** sentence scoped it to a media path symmetric RTP cannot provide. Two statements of
one exit criterion, disagreeing about whether M10 costs a TURN client (`X-50`). The **Done when**
sentence governs: M10 needs RFC 8445 and 8839, `M-24` (RFC 8656) is in this epic and in no milestone,
and the reasoning is written out under M10 rather than summarised twice.

The order is a dependency chain, not a preference. `M-19` (the RFC 8839 attributes) and `M-20`
(the STUN check codec) are independent and can run together; `M-21` (the sans-IO agent) needs
`M-20`; `M-22` (driving it on the media port) needs `M-19` and `M-21` and owns the test `M-16`
named; `M-23` (restart) follows it, and so does `M-27`, which offers and answers ICE from a call and
is what made everything above it reachable from `sipx-call`. `M-24` (a relayed candidate) is last and
unscheduled. ICE-lite is deferred with its reason recorded — sipx is a UA behind NATs, which is the
case lite does not serve — but *interoperating* with a lite peer is not, because an implementation
that only handles a full peer hangs waiting for checks a lite peer is never required to send.
See [design](designs/media.md).

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

### Commit-stable generated measurements — `commit-snapshot`

The maturity report must describe the commit that carries it and give the same answer in the
originating worktree and a clean checkout. `X-39` owns the full contract: the ordinary all-changes
path already works, while selective staging and stable date attribution remain open after the
2026-07-30 repository review. Done when all-changes, selective, midnight-boundary and retained-date
amend fixtures stay green across the commit boundary without making real report drift invisible. See
[design](designs/commit-snapshot.md).
