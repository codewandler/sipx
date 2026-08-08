# sipx — roadmap & status

The big picture: what's delivered, what's next, and the epics that group related stories. The
operational detail lives on the [board](https://github.com/codewandler/sipx/blob/main/docs/stories/README.md) (generated from story frontmatter);
this document is the hand-written narrative around it.

Two companions to this one: [**RFC compliance**](compliance.md) is the measured list of what
sipx supports, generated from a registry and checked in CI; [**the RFC roadmap**](rfc-roadmap.md)
is the order the remaining gaps close in and why.

## Status

_As of 2026-08-08:_ **`1.0.0-rc.3` is the current public prerelease; beta.2 through beta.7 and
`rc.2` remain immutable.** The second release candidate answers two independent external sweeps of
the published `rc.2` artifacts — twenty-five findings across call lifecycle, endpoint reachability,
timeout honesty, refusal signalling, automation contracts and published onboarding — and adds
bounded SIP endpoint resolution, G.722, a typed CLI parser, supervisor-termination handling and the
six-module call split, which moves no public path. Local speech and call-audio analysis are
specified and deliberately unimplemented. Both endpoint-responder directions were re-measured under
the current contract after a schema change invalidated the retained pair; their intervals still
overlap at the tested ceiling, so the comparison stays inconclusive rather than a ranking. Stable
`1.0.0` remains a separate promotion: independent application use is still missing and is not
inferred from repository evidence.

_As of 2026-08-05:_ **`1.0.0-rc.2` was the first published release candidate.** It combined the
post-beta.7 transport fallback and drain, application-owned observability, the explicit PCM/L16
boundary, parser-owned routing and privacy edits, field-trap fixes, portable five-target CLI
artifacts and a direct architecture guide.

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

Beta.5's delivered implementation wave is **M13 — Endpoint-complete**: it closes the measured
sipx-owned endpoint gaps, and M14 records the selected bounded load comparison. The routed realtime
binding is a separate application integration with deterministic default-suite evidence; A-23 keeps
its credentialed live proof visible as backlog. M15 separately tracks the requested
browser-embeddable audio package without turning sipx into a WebRTC engine. M16 tracks local speech
and deterministic call-audio analysis, M17 extends the delivered realtime bridge with understanding
and policy-governed phone actions, and M18 tracks custom call DSP. M9's remaining off-media bridge
work is not pulled into that wave. Beta.6 ships the responder hardening behind the retained M14
evidence and the completed M15 specification gate; the remaining M15 through M18 stories are plans.

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
- **M10 — Reachable.** The three ways of being reached that M6 left open, each recorded against the
  test its clause is *written* as rather than against the mechanism underneath it. `X-50` checked the
  evidence rather than the statuses and found two of the three short; `X-52` closed that distance
  without widening a clause:
  - **One of two registrations of an address of record, called individually** —
    `each_of_two_registrations_of_an_address_of_record_is_called_individually`. Two instances register
    one AOR; a call placed at the first instance's GRUU is answered by that instance and carries audio
    both ways, and the second, whose registration is equally current, never sees the INVITE. The
    contrast is what carries the claim: the same routing applied to the address of record resolves to
    *both* bindings, so being individually callable is a property of the GRUU and not of having
    registered. **What is sipx's and what is the harness's, stated plainly:** RFC 5627 has a
    *registrar* mint the GRUU and a *proxy* resolve it to one binding, and sipx is the UA half of
    that RFC and implements neither — so the registrar and the resolution in that test are doubles,
    and always were. What sipx holds, and what the test falsifies, is per-instance GRUU learning,
    presentation, and recognition of a GRUU as its own. `X-59` closes the last-hop case too: once a
    registered `UserAgent` has learned its own GRUUs, its inbound request decision refuses an
    INVITE carrying another value with `404`, before the application can hand it to `sipx-call`.
  - **A push into an answered call** —
    `a_push_wakes_a_client_that_held_no_connection_into_an_answered_call`. A client holding no
    connection at all is woken, refreshes its binding, answers the call it was woken for, and carries
    audio in both directions — asserted in RFC 8599 §4.1.3's order, because an answered call proves
    nothing about a push if the client was reachable all along.
  - **A nominated pair where symmetric RTP cannot reach** — `M-27`'s
    `a_call_uses_a_nominated_pair_when_both_host_candidates_are_silent`, which already held and is
    untouched by this story.
  - **`M-16`'s open tracker is not M10's status.** It stays open for `M-24`'s relayed candidate,
    which belongs to the ICE epic and to no milestone; reading the tracker as the milestone is the
    substitution [the M10 section below](#m10--reachable) exists to refuse.
- **M12 — Provable.** What lets someone outside the project check the claims rather than take them:
  the whole RFC 5118 corpus classified and green and now tamper-evident (`X-16`, `S-31`, `X-56`), two
  independent peers each with their own CI job over one shared test list (`X-17`), a fuzzer driving
  the transaction layer with programs rather than bytes and a seed corpus proven unmodified after the
  campaign (`X-19`, `S-26`), and **every discard in the signalling path counted and exportable beside
  the capture that explains it** (`X-18`, `X-54`). The last of those took a rework: the counter was
  first taken where a request was handed over rather than where the wire was missed, which is a
  measurement that would have been trusted to rule a cause out. See
  [where M12 stands](#m12--provable).

## Milestone sequence

Four milestones, each independently demonstrable, each ordered by the same rule the
[RFC roadmap](rfc-roadmap.md) uses: **a gap that changes what sipx can be deployed as beats a gap
that adds a feature.** M9 to M12 are defined and their stories are cut. **M10 is delivered** as of
`X-52` — its section stays below because the scoping argument in it is still load-bearing, and
[where M10 stands](#m10--reachable) now records which test carries each of its three clauses.
**M12 is delivered** as of `X-54`, and its section stays below for the same reason M10's does. `X-51`
checked its four **Done when** clauses against the tests and CI jobs meant to demonstrate them and
found three held and the fourth short in the same way M10's two were; `X-54` closed that fourth —
[where M12 stands](#m12--provable) records which evidence carries each clause. M9 is two thirds
done: `S-19` and `C-2` closed, with `C-1` substantially implemented but still open on delayed
reliable-provisional relay and a true off-media signalling mode. **M11 is delivered**: `S-20`,
`S-21`, `T-22` and `S-34` provide the identity, diversion-history, overload and live-call evidence.
Application-host phase 1 is delivered by `A-2`; the remaining in-progress work is the later
[app-sdk](#application-sdk--app-sdk) and [app-host](#application-host--app-host) phases below, which
are not milestones because they are not RFC gaps.

### M13 — Endpoint-complete (delivered)

*Closed every known capability owned by a sipx endpoint before measuring it under load.*

The `parity-wave-1` area tag is the machine-queryable delivered set: **fifteen stories across nine
epics**. Its implementation is integrated and all 36 full-gate steps are green. M13 is not a release
name or stable-1.0 promise. M14 is now the next selected measurement wave.

| Order | Delivery lane | Stories | Outcome | Dependency order |
|---:|---|---|---|---|
| 1 | [Stack comparison](designs/stack-comparison.md) | **X-97** | Leaf-level, generated capability ownership; newly found sipx gaps joined M13 | first |
| 2 | [Event reachability](designs/event-reachability.md) | **S-24, S-35, S-37, S-38, S-39** | Inbound notifier, reusable subscriber, registration-event consumer and live publication paths | S-35/S-37 before S-24/S-38/S-39 |
| 3 | [Dialog extensions](designs/dialog-extensions.md) | **S-40** | Authenticated application-owned INFO, MESSAGE and extension requests | independent first batch |
| 4 | [Live endpoint policy](designs/live-endpoint-policy.md) | **T-31, T-32** | Atomic TLS identity rotation, then bounded typed observation and policy | T-31 before T-32 |
| 5 | [Supported test surfaces](designs/test-surfaces.md) | **X-75, M-53** | A quiet library, real call harness and runnable RTP echo proof | X-75 before M-53 |
| 6 | Registration observation | **S-42** | Typed public address learned from a registration response | second batch |
| 7 | [Dialog persistence](designs/dialog-persistence.md) | **S-43** | Versioned, bounded dialog snapshot and safe restoration | after dialog extensions |
| 8 | [Comparative load](designs/comparative-load.md) | **X-98, P-15** | Freeze the bounded workload and ship its finite answering endpoint | X-98/X-75 before P-15 |

**Delivery threshold:** the pinned capability ledger has no unclassified row and no open sipx-owned
row; all fifteen selected stories plus any sipx story discovered by X-97 are done; every
cluster-owned row links to a revision-pinned story in that repository; every new live path is
bounded, cancellation-safe and represented in the RFC registry; and the full gate is green.
Proxying, registrar/location service, routing, trunks,
allowlists, deployment and cluster failover do not become sipx work merely because the comparison
subject packages them beside its endpoint.

The dependency-closed implementation order put X-97 and S-37 first; S-35, S-40, T-31 and X-75 ran
beside them; S-38 and S-39 followed the event-client contract; T-32 followed the live-update review;
S-42 and M-53 followed the first batch; S-43 followed the dialog-extension review; and X-98 froze
the load contract before P-15 implemented its responder. X-97 served as a discovery gate, expanding
the wave when it found another real sipx-owned leaf instead of declaring parity against a fixed
initial list.

### M14 — Pressure-proved

*Run a fair, bounded endpoint comparison only after the comparable surface exists.*

| Order | Story | Outcome | Starts when |
|---:|---|---|---|
| 1 | [**X-99 — Run and publish the comparative load result**](stories/X-99-run-and-publish-the-comparative-load-result.md) | Both directions, immutable builds, raw evidence and a generated, non-ranking summary | X-98, P-15 and M13 |

**Done when** low-rate correctness qualifies each supported direction, the neutral driver proves
headroom, five bounded repetitions identify sustainable UDP dialog-signalling capacity under the
predeclared threshold, every process and dialog drains, and exact inputs plus raw results generate a
refreshable public summary. Media, secure transports and connection churn are not inferred from the
first signalling-only result.

### M15 — Browser-embeddable audio

*Ship sipx as a browser-consumable audio endpoint without implementing a WebRTC engine.*

| Order | Story | Outcome | Starts when |
|---:|---|---|---|
| 1 | [**A-16 — Specify the browser SDK contract**](stories/A-16-specify-the-browser-sdk-contract.md) | ABI, lifecycle, security, package and browser-support contract | unscheduled admission gate |
| 2 | [**S-41 — Export the sans-I/O session kernel to WebAssembly**](stories/S-41-export-the-sans-io-session-kernel-to-wasm.md) | Deterministic SIP/SDP/dialog kernel with host bytes, timers and entropy | A-16 |
| 3 | [**T-33 — Bind browser WebSocket signalling**](stories/T-33-bind-browser-websocket-signalling.md) | Bounded WSS adapter using browser-owned I/O | A-16 and S-41 |
| 4 | [**M-52 — Adapt browser-native WebRTC audio**](stories/M-52-adapt-browser-native-webrtc-audio.md) | Audio-only `RTCPeerConnection` adapter reusing the beta.4 profile | A-16 |
| 5 | [**A-17 — Generate and package the browser SDK**](stories/A-17-generate-and-package-the-browser-sdk.md) | Installable JavaScript, checked TypeScript and WASM package | S-41, T-33 and M-52 |
| 6 | [**A-18 — Publish a runnable browser-audio demo**](stories/A-18-publish-a-runnable-browser-audio-demo.md) | Public static demo for register, dial, answer and non-silent audio | A-17 |
| 7 | [**X-100 — Prove the packaged browser SDK**](stories/X-100-prove-the-packaged-browser-sdk.md) | Clean consumer and supported-browser matrix in both SIP roles | A-18 |

**Done when** a clean JavaScript consumer installs the exact package, the public demo registers over
WSS and completes non-silent audio in both SIP roles across the supported browser matrix, and all
socket, timer and media resources are observed closed after cancellation. The browser owns
`RTCPeerConnection`, ICE, DTLS-SRTP, capture and render; video, data channels and a Rust WebRTC engine
remain out of scope. M15 is tracked but is **not** part of the selected M13 wave.

### M16 — Local call intelligence

*Make live-call speech and small real-time audio facts useful on a local machine without coupling
the two capabilities or hiding provider choice.*

M16 contains two separate epics. [Local speech](designs/local-speech.md) defines interchangeable
recognition and synthesis providers, ships practical local/offline implementations with declared
accelerator and CPU behavior, and carries their lifecycle through the application SDK.
[Call-audio analysis](designs/call-audio-analysis.md) supplies deterministic voice activity and
signal metrics without loading a speech model. They share M-54's bounded PCM attachment and no
other implementation state.

| Order | Epic | Stories | Outcome | Starts when |
|---:|---|---|---|---|
| 1 | Local speech | **A-25** | Recognition/synthesis provider contracts, discovery and selection | spec gate |
| 1 | Call-audio analysis | **M-57** | Sans-I/O frame-processor contract and sample vectors | spec gate, parallel with A-25 |
| 2 | Shared seam and policy | **M-54, A-28** | Bounded PCM/resampling attachment plus privacy and isolation | A-25 and M-43 |
| 3 | Local speech | **M-55, M-56** | Local/offline recognition and synthesis on measured accelerator/CPU paths | A-25, A-28 and M-54 |
| 3 | Call-audio analysis | **M-58, M-59** | Voice activity and signal metrics through typed events | M-57 and M-54 |
| 4 | Call-audio analysis | **M-60, M-61** | Bounded adaptation and hostile-input hardening | analysis implementation |
| 5 | Local speech | **A-26, A-27** | Ordered recognition and bounded synthesis/cancellation lifecycle | providers; A-27 also uses M-58 |
| 6 | Local speech | **X-105, X-104** | Provider conformance plus runnable measured example | implementation and SDK stories |
| 6 | Call-audio analysis | **X-106, A-29** | Accuracy/resource corpus plus runnable model-free example | analysis implementation |

**Done when** endpoint and per-call provider selection use one public contract; bundled and external
providers pass one conformance suite; accelerator and CPU limits are measured; deterministic voice
activity and signal metrics meet declared accuracy and resource budgets; concurrent calls share no
data or state; default runs retain no audio or text; and clean packaged examples prove both epics.

### M17 — Realtime phone understanding and actions

*Extend the delivered routed-agent bridge without giving the model authority over the phone.*

The [Realtime phone extension](designs/openai-realtime-phone.md) reuses A-22's one audio bridge,
configuration and credential boundary. A-21 remains the deterministic peer and A-23 remains the
single guarded live-proof authority.

| Order | Story | Outcome | Starts when |
|---:|---|---|---|
| 1 | [**A-30 — Adapt Realtime session events**](stories/A-30-adapt-openai-realtime-session-events.md) | Correlated finite lifecycle and deliberate session replacement | A-22 |
| 2 | [**A-31 — Emit understanding events**](stories/A-31-emit-realtime-understanding-events.md) | Bounded typed transcripts and untrusted-model provenance | A-30 and C-3 |
| 2 | [**A-33 — Enforce action policy**](stories/A-33-enforce-realtime-action-policy.md) | Closed schemas, idempotency, deadlines and confirmation | A-22 |
| 3 | [**A-32 — Allowlist phone actions**](stories/A-32-allowlist-realtime-phone-actions.md) | Exhaustive deny-by-default action registry and correlated outcomes | A-30 and A-33 |
| 4 | [**X-107 — Prove deterministic and live paths**](stories/X-107-prove-openai-realtime-test-service.md) | A-21 CI plus A-23's guarded live evidence for the extension | A-30…A-33 |
| 5 | [**X-108 — Publish the measured phone**](stories/X-108-publish-openai-realtime-testkit-phone.md) | Runnable policy UI with bounded rate, cost and latency evidence | X-107 |

**Done when** typed session, speech, transcript, response, rate and action state reaches the SDK;
only schema-valid, idempotent, policy-accepted phone actions execute; consequential actions require
independent confirmation; every request has a correlated terminal event; deterministic CI covers
all failure paths; and the packaged example records redacted evidence with zero residual work.

### M18 — Custom call-audio DSP

*Let applications compose bounded filters, effects and noise reduction on live call audio through
one deterministic processor contract.*

The [custom call-DSP](designs/custom-call-dsp.md) epic consumes M-54's call-local PCM seam. Built-in
effects and external processors use the same frame contract; supervised processors never block the
media worker; noise reduction is interchangeable and measured; and SDK code controls registered
graphs without running callbacks on the media worker.

| Order | Story | Outcome | Starts when |
|---:|---|---|---|
| 1 | [**M-63 — Specify the DSP contract**](stories/M-63-specify-custom-call-dsp-contract.md) | Frames, execution profiles, bounds and minimum failure policy | M-54; spec gate |
| 2 | [**M-64 — Attach bounded DSP graphs**](stories/M-64-attach-bounded-dsp-graphs-to-calls.md) | Ordered per-direction graphs, atomic replacement and teardown | M-54 and M-63 |
| 3 | [**M-65 — Ship effects and filters**](stories/M-65-ship-deterministic-audio-effects-and-filters.md) | Deterministic gain, filters, distortion, bit-crush and stutter | M-63 |
| 3 | [**M-66 — Ship local noise reduction**](stories/M-66-ship-interchangeable-noise-reduction.md) | Replaceable measured baseline with explicit latency | M-63 |
| 4 | [**M-67 — Control graphs through the SDK**](stories/M-67-control-dsp-graphs-through-the-sdk.md) | Typed registry, parameters and lifecycle events | M-64 |
| 4 | [**M-68 — Harden failure isolation**](stories/M-68-harden-dsp-realtime-failure-isolation.md) | Prove budgets, bypass/termination and worker containment | M-63 and M-64 |
| 5 | [**X-109 — Measure DSP quality and cost**](stories/X-109-measure-custom-dsp-quality-and-cost.md) | Exact effects plus noise, latency, CPU and drop evidence | M-65, M-66 and M-68 |
| 6 | [**A-34 — Publish the DSP example**](stories/A-34-publish-custom-call-dsp-example.md) | Runnable live graph with bypass and teardown | M-67 and X-109 |

**Done when** built-in and external processors compose on either call direction; changes occur at
deterministic sample boundaries; intentional effects remain distinct from overload defects; the
noise reducer meets declared quality and real-time thresholds; proven execution profiles cannot
stall RTP or cross call boundaries; every transition is observable; and a clean consumer runs the
packaged example with zero residual work.

**Most recent release cut.** Beta.4 is published. Its boundary is a deliberately coherent
feature-and-security wave rather than a bag of the ten shortest stories. The
product claim is narrow: audio connects securely through real networks, including one independently
proven browser-compatible path. Publication still requires the complete clean release gate,
exact-SHA main CI and Pages, the protected registry workflow and one immutable annotated tag. The
six generated predicates below measure that boundary; they do not authorize broader publicity.

### Beta.4 — secure audio through real networks

The `beta4` area tag is the machine-queryable selection. Exactly ten stories carry it; `M-38`
remains their epic tracker and is deliberately not counted as an eleventh implementation story.

| Order | Story | Outcome | Starts when |
|---:|---|---|---|
| 1 | [**M-48 — Specify the browser-audio profile and state machine**](stories/M-48-specify-browser-audio-profile.md) | Normative SDP, ordering, downgrade, resource and byte-vector contract before code | now |
| 2 | [**X-64 — Pin malformed-input refusals**](stories/X-64-pin-the-malformed-input-refusals-with-named-tests.md) | Named mutation-proven bounds across UDP, TCP, TLS, WS and WSS | now, parallel |
| 3 | [**X-65 — Assert branch and tag RNG is cryptographic**](stories/X-65-assert-the-branch-and-tag-rng-is-cryptographic.md) | The off-path response-injection invariant fails loudly if construction regresses | now, parallel |
| 4 | [**M-47 — Reject replayed SRTCP with a separate replay window**](stories/M-47-reject-replayed-srtcp.md) | Closes the known shipped replay gap before multiplexed control traffic becomes browser-reachable | now, parallel |
| 5 | [**M-42 — Advertise chosen addresses and latch RTP without ICE**](stories/M-42-advertise-a-chosen-address-and-latch-rtp-without-ice.md) | Highest-demand real-network path, with explicit precedence when ICE is selected | verify-first now |
| 6 | [**M-46 — Multiplex RTCP and negotiate the DTLS setup role**](stories/M-46-multiplex-rtcp-and-negotiate-the-dtls-setup-role.md) | `rtcp-mux` and `actpass` substrate required by browser media | after M-48 |
| 7 | [**M-49 — Negotiate a fail-closed browser-audio profile**](stories/M-49-negotiate-browser-audio-profile.md) | One named call profile either negotiates the complete secure path or refuses before I/O | after M-48 and M-46 |
| 8 | [**M-50 — Run ICE, DTLS, SRTP and SRTCP on one nominated component**](stories/M-50-run-browser-media-on-one-component.md) | One bounded, cancellation-safe owner with no bind/drop/rebind race | after M-42, M-46, M-47 and M-49 |
| 9 | [**M-51 — Prove browser audio against an independent endpoint**](stories/M-51-prove-browser-audio.md) | Non-silent Opus in both SIP roles plus fingerprint, nomination and downgrade negatives | after M-50 |
| 10 | [**A-15 — Publish and verify `1.0.0-beta.4`**](stories/A-15-publish-beta4.md) | Immutable tag, registry consumer, installed CLI, exact-SHA Pages and GitHub prerelease | after the other nine and M-38 |

The critical path is `M-48 → M-46/M-49 → M-50 → M-51 → A-15`. `X-64`, `X-65`, `M-47` and the
verify-first half of `M-42` can proceed in parallel. The selection intentionally defers the second
audio-runtime lane (`M-43` PCM, `M-44` G.722 and `M-45` jitter quality), TURN (`M-24`), portable
artifacts (`P-14`) and graceful drain (`T-29`) rather than placing two unrelated critical paths in
one cut. Video remains the post-beta admission decision `M-40`; the current vision still excludes
it, and beta.4 does not silently reverse that decision.

### M9 — Bridgeable

*What has to be true of a session before sipx can sit between two of them.*

M7 makes a *message* forwardable. Nothing makes a *session* forwardable: `sipx-call` owns one
dialog and one media pipeline, and the media bridge `M-11` built has no signalling counterpart.
This is also where the [edge design](designs/edge.md)'s open question gets answered — whether a
B2BUA belongs in this repository. The product does not; the primitive does.

| Story | RFC | Why it is in M9 |
|---|---|---|
| **S-19** UPDATE | 3311 | The only way to change a session that has not been answered — §5.1 permits it "for both early and confirmed dialogs". Also what RFC 4028 §7.4 *recommends* a session refresh use when the peer allows it, which `S-11` could not do. |
| **C-2** Early media | 3960 | A reliable provisional starts one negotiated session, signals the application to stop its local tone, and hands the same live stream to the confirmed call without rebinding. |
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

**Where M10 stands, 2026-07-31.** **Delivered.** All three clauses are now demonstrated by a test
written as the clause is written, and the milestone is recorded in [Delivered](#delivered) against
those three tests rather than against the statuses of the stories that built the mechanisms:

- **GRUU** — `X-52`'s `each_of_two_registrations_of_an_address_of_record_is_called_individually`. Two
  instances of one address of record, a call placed at one instance's GRUU, answered by that instance
  with audio both ways, and the other instance never sees it — **because the test's routing double
  sends it to one flow, which is the role a proxy plays and sipx does not.** The stack's own half is
  that each instance learns, presents and recognises its own GRUU, and that is the half the
  falsification attacks. `X-59` additionally delivers the first instance's GRUU to the second
  flow, bypassing that routing double, and proves the registered UA refuses it `404` rather than
  establishing a call. `T-20`'s
  `a_request_to_a_gruu_reaches_the_instance_that_registered_it` remains what it always was — an
  `OPTIONS` against one agent and a stub registrar, the mechanism this composes on top of — and is
  not reopened.
- **Push** — `X-52`'s `a_push_wakes_a_client_that_held_no_connection_into_an_answered_call`, which
  carries `T-21`'s ordering through to the answered call and the audio on it. `T-21`'s
  `a_push_wakes_a_client_that_refreshes_its_binding_before_the_invite` proved §4.1.3's order and
  stopped at the INVITE, which was its whole Acceptance and is not reopened either.
- **ICE** — `M-19`…`M-23` and `M-27`, done; `M-24` out of scope per above. Demonstrated in full by
  the test named in ground 1. The table's `M-16` row is the epic's tracker, and a tracker stays open
  until its last child lands — including `M-24`. **`M-16`'s status is therefore not M10's**, and
  reading it as M10's is the same substitution the ICE heading used to make.

**What was demonstrated, and what was not repaired.** Both new tests passed the first time they ran:
nothing in the stack was broken, and this milestone was short of *evidence* rather than of behaviour.
That is a weaker claim than a red-then-green fix, so each test was falsified against a real mutation
of the library instead of being trusted for passing. Making `Gruus::from_response` select a binding by
position rather than by `+sip.instance` — the failure its own doc comment names — has both instances
adopt the same GRUU and the first test says so by name; discarding the PURR that RFC 8599 §8.2 assigns
the binding fails the second. Recording M10 as delivered on tests that had never been shown capable
of failing would have been the defect `X-30`, `X-35` and `X-42` each found, one layer further out.

### M11 — Attestable

*What a peer network requires before it will carry the traffic.*

Everything up to here makes a call work. None of it makes a call *accountable*: sipx cannot prove
who placed it, say what happened to it on the way, or ask a neighbour to send less.

| Story | RFC | What it unlocks |
|---|---|---|
| **S-20** STIR and PASSporT | 8224, 8225 | A signed `Identity` header field, and a verification service that refuses a bad one with the code §6.2.2 names rather than a generic 400. Without it, a call handed to the public telephone network is unattested traffic. |
| **S-34** Live-call STIR evidence | 8224, 8225 | Select the service from a real call, have an independent verifier accept the outbound field, and refuse an invalid inbound signature before the application answers. |
| **S-21** History-Info and Reason | 7044, 3326 | Who diverted a call and why. One story, not two: RFC 7044 §10.2 requires the `Reason` inside the `hi-targeted-to-uri`, and RFC 3326 is `syntax only` today precisely because nothing populates it. |
| **T-22** Overload control | 7339, 7415 | `oc`, `oc-algo`, `oc-validity` and `oc-seq` on the `Via`, so a loaded endpoint says how much to send instead of answering 503 — which is what `T-19` will otherwise leave it doing. |

**Done when** an outbound call carries an `Identity` header field an independent verifier accepts,
an inbound one whose signature does not verify is refused with 438, a diverted call arrives
carrying its diversion history with a reason per hop, and an overloaded endpoint publishes a rate
its neighbour honours.

`S-20`'s pure service lands first and alone — it is the largest item here and the only one with a
credential fetch and a signature in it. `S-34` then carries it through a real call and an independent
verifier. `S-21` supplies the history a re-signing element consumes; M9 is what creates that element.
`T-22` is transport work and independent of all three.

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

**Where M12 stands, 2026-07-31. M12 is delivered.** All four stories are closed and all four clauses
now hold against tests and CI jobs rather than against statuses. `X-51` checked the first three and
they are unchanged; `X-54` closed the fourth, which was short in exactly two of its words.

- **The corpus** — `X-16`, done, and the clause holds. `crates/sipx-testkit/src/rfc5118.rs` classifies
  all twelve of Appendix A's messages across §4.1 to §4.10, none left unreferenced, and its
  `DEVIATIONS` list is empty since `S-31` taught the parser to tolerate §4.10's three-colon reference.
  Nineteen tests are green under the gate's own `cargo test --workspace`. `X-56` then made the
  fixtures tamper-evident from a gate step and a CI job, so the classification cannot be quietly
  weakened by editing the bytes it classifies.
- **Two peers** — `X-17`, done, and the clause holds. `tests/interop/run.sh --list` reports two peer
  profiles and CI builds its matrix from that list, so one `interop (<peer>)` job runs per profile.
  Both play the `server` role over the identical nine-test list, and neither declares a
  `PEER_DIVERGES_ON` any more.
- **The fuzzer** — `X-19`, done, and the clause holds. `transaction_sequence` drives `TransactionLayer`
  with a decoded program of messages, application requests and fired timers, built rather than parsed.
  CI's `fuzz` job runs it sixty seconds a push against seventeen committed seeds, `KNOWN_DEFECTS` is
  empty, and `the_campaign_suppresses_nothing_and_run_agrees_with_run_strict` keeps that honest.
- **Counted, and next to a capture** — `X-18` and `X-54`, done, and the clause holds.

  1. **"Every" now covers the crates the phrase covers.** `no_discard_in_the_signalling_path_is_silent`
     scans `CRATES` — `sipx-transport` **and** `sipx-call` — from one list beside the one copy of the
     detector, with `docs/specs/sip-transport.md` §12.3 stating why those two and not the sans-IO core
     or the media path. Widening it exposed **sixteen** unexplained sites where the hand census had
     found seven, which is the argument for an enumeration over a sweep, made by the enumeration
     itself. All sixteen carry a counter or a `// discard:` reason. `sipx_transport::UnsentCounts`
     counts by method every request the endpoint **tried to put on the wire and could not** — taken at
     the transmit rather than at the hand-off, so a refused connection, an unreachable peer and an
     over-MTU datagram are all in it. A failed BYE on a teardown path is finally the number an
     operator asking "why did that call linger" can read. `CallEvents::dropped` counts events a
     consumer was too far behind to be given, per call, reported to the only party who can act.
  2. **"Next to" is now true outside the process.** `sipx_call::SignallingCounts` is one reading of
     both crates' snapshots, embedding each unaltered rather than recounting, with `dispatch` an
     `Option` because an endpoint with no dispatcher has not dispatched nothing — it has not been
     asked. `sipx --counters <FILE>` exports it and `--capture <FILE>` implies it as
     `<capture>.counters.json`, written on **every** path out of the command rather than only the
     successful one, since the run that fails is the run the bug report is about.

**What that cost to get right, recorded because the counter would have been believed.** The first
version incremented at `Handle::send`, which returns `Ok` as soon as a transaction exists rather than
when bytes leave — so the counter could fire only on a closed endpoint, never on the network failures
it advertised, while the spec and seven call-site comments told a reader otherwise. An independent
review found it with an over-MTU BYE. §12.3 now states the rule `M-32` inherits: **count where the
loss happens, not where it is reported.**

**Media is not what holds this open, and never was.** The clause says the *signalling* path, so the
media counters `X-18` split out to `M-32` fall outside it — read off the clause's own word rather than
assumed. `M-32` extends §12.3 rather than editing it: a crate joins the path by being added to
`CRATES` and by growing a member on `SignallingCounts`, never by adding fields to another crate's
struct and never by a second tally of an event already counted.

### After M12

**QUIC** (`T-12` transport, `T-13` verified against a real peer) is specified in
[`docs/specs/sip-quic.md`](https://github.com/codewandler/sipx/blob/main/docs/specs/sip-quic.md)
and the transport is now implemented. There is no RFC for SIP over QUIC, so every choice in that
spec is ours; that makes `T-13`'s independent peer evidence more important, not optional proof that
can be inferred from two sipx endpoints. QUIC remains outside a milestone, while `T-13` is the open
evidence story for the public transport claim.

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

### `1.0.0-beta.7` — the hypothetical public-announcement predicates

This is the threshold at which the prerelease could responsibly receive broader publicity: outside
Rust users can install exact registry
versions and exercise the whole advertised endpoint surface, while supported APIs are still allowed
to break with a changelog entry. Meeting it does not authorize an announcement. The actual beta cut
creates its GitHub prerelease only; later publicity requires separate explicit authorization. Only
`1.0.0` freezes supported APIs.

Stories declare the predicates they bear on through `announcement:` frontmatter, using the same
single-source rule as the alpha's `predicate:` field. [`maturity.md`](maturity.md) generates the
state and names blockers. All six are required; they are not a weighted score.

1. **Every alpha integrity predicate still holds.** This is derived from the complete alpha table,
   not from another story list.
2. **Hostile-input, entropy and SRTCP replay invariants are executable.** Named tests and recorded
   mutations hold framing/allocation bounds, cryptographic branch/tag construction and the separate
   authenticated SRTCP replay window.
3. **Browser-audio negotiation is complete and fail-closed.** The normative profile and its call
   policy agree on one SDP vocabulary; missing features, incompatible roles and weaker media are
   typed refusals rather than downgrades.
4. **One nominated component carries every browser-media protocol safely.** STUN, DTLS, SRTP and
   SRTCP have one bounded owner, nomination binds the peer, fingerprint verification precedes keys,
   and RTCP multiplexing is real behavior rather than an SDP claim.
5. **An independent browser endpoint carries Opus in both roles.** The bounded proof asserts
   non-silent audio, negotiated facts and non-vacuous fingerprint, nomination and downgrade
   negatives.
6. **Exact registry, CLI, Pages and GitHub release evidence agrees.** Every public package is
   rehearsed and checksum-verified, an exact registry consumer and installed Opus CLI run, Pages is
   bound to the release SHA, and one immutable annotated tag owns the GitHub prerelease.

No RFC total or completion percentage belongs here. The compliance table measures protocol scope;
the hypothetical announcement gate measures whether the scope being offered is truthful and usable.

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

### Capability ownership — `stack-comparison`

Generated comparison data records evidence and confidence for chooser-facing claims. `X-97` extends
it to a leaf-level inventory with one owner and disposition per public capability, turning comparison
into M13's discovery gate rather than a one-time prose audit. Subject-specific material remains in the
comparison data directory. Done when no row is stale, unowned, unevidenced or linked to an already
closed gap. See [design](designs/stack-comparison.md).

### Event reachability — `event-reachability`

Make the existing event machinery usable from a live endpoint: inbound SUBSCRIBE and mandatory
initial NOTIFY (`S-35`), a specified reusable subscriber (`S-37`, `S-38`), and inbound/outbound
PUBLISH using the existing compositor (`S-39`). Event-package policy stays with its consumer; `S-24`
uses the generic subscriber for `reg`. See [design](designs/event-reachability.md).

### Application-owned dialog extensions — `dialog-extensions`

One constrained path for INFO, MESSAGE and admitted extension methods in both directions. Dialog
routing, sequencing, authentication and transaction lifetime remain owned by the stack; re-INVITE,
UPDATE, REFER, NOTIFY, BYE and OPTIONS keep their specialized semantics. See
[design](designs/dialog-extensions.md).

### Live endpoint policy — `live-endpoint-policy`

Security-sensitive changes to a running endpoint: validate and atomically replace the server TLS
identity for new handshakes (`T-31`), then expose bounded typed observation and a narrow pre-transaction
policy seam (`T-32`). No arbitrary post-key message mutator, file watcher or competing resolver is
introduced. See [design](designs/live-endpoint-policy.md).

### Supported test surfaces — `test-surfaces`

Turn the internal deterministic call tools into a public downstream test contract while keeping every
library crate quiet unless its host installs tracing. The in-process surface is deliberately separate
from wall-clock benchmark orchestration. See [design](designs/test-surfaces.md).

### Comparative signalling load — `comparative-load`

A subject-neutral, signalling-only dialog workload with bounded process supervision, a machine-ready
answerer and immutable raw evidence. It reuses the existing scheduler and soak measurements rather
than reopening them. M14 reports correctness-qualified capacity and uncertainty, not a winner. See
[design](designs/comparative-load.md).

### Browser audio SDK — `browser-sdk`

An installable JavaScript/TypeScript package generated around a sans-I/O WebAssembly SIP/session
kernel, browser WebSocket signalling, browser-native WebRTC audio and a runnable public demo. This is
not the server-side application SDK and not a WebRTC implementation: the browser owns network and
media engines. M15 is audio-only and unscheduled behind its specification gate. See
[design](designs/browser-sdk.md).

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

### Opus product support — `opus`

The complete downstream path for Opus rather than the codec in isolation. `M-13` implements RFC
6716 encode/decode and RFC 7587 payload mapping; `M-30` makes both call roles select it; `P-9` and
`P-13` expose it through the diagnostic phone and prove that an Opus negotiation carries media
between command processes; `M-37` makes construction failure typed instead of putting G.711 under
the negotiated Opus payload type. Those are complete and are reused, not reopened.

**Delivered:** `M-39` makes both CLI directions follow the negotiated media clock and packet size,
proves distinct 48 kHz signals rather than non-empty one-way media, exercises the normalized
Opus-only package graph, and passes Opus-only calls against an independent peer in both SIP roles.
The peer proof asserts the dynamic payload number, 48 kHz RTP clock, non-silence and signal identity,
so G.711 cannot satisfy it. Exact crates.io installation remains part of the beta distribution
story, not missing codec behavior. Optional RFC 7587 `fmtp` controls remain a documented non-goal
unless separately requested. See [design](designs/opus.md).

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

### OpenAI realtime bridge — `openai`

The first proof whose far end is a *service*: one sipx call leg held together with one OpenAI
realtime session over a WebSocket, caller audio up as G.711 passthrough, the agent's voice back
into the call's RTP — so "dial in, an agent answers" is demonstrable with one command. The seam
is application-side and the epic builds only what the workspace lacks: a spec with vectors
(`A-19`), a general WSS client over the one TLS policy (`A-20`), a deterministic loopback peer
so the loop runs in the default CI matrix with no account (`A-21`), the bridge and its product
path (`A-22`), and the repo's first credentialed opt-in live proof under disclaim-don't-skip
(`A-23`). Deliberately not here: the vendor's SIP connector (sipx stays the SIP endpoint),
transcoding, reconnection, and any claim the stand-in peer could satisfy vacuously. Done when
the stand-in-backed loop is green in CI and one live bridged call's evidence is recorded.
See [design](designs/openai.md).

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

### Browser-compatible WebRTC audio — `webrtc-audio`

One browser audio path, not a full WebRTC stack. It composes the already delivered SIP-over-WSS,
Opus/G.711/telephone-event, ICE and DTLS-SRTP work into the profile a browser can actually answer:
one audio media section, `RTP/SAVPF`, RTCP multiplexing, ICE nomination followed by DTLS on the
selected component, and fail-closed secure media. Video, browser APIs, data channels, SCTP,
simulcast, multiple bundled media sections and a general browser media engine remain outside the
repository's scope. `M-24`'s TURN client widens NAT coverage but is not permission to call a
host/server-reflexive path universally reachable.

RFC 8834 supplies the RTP/SAVPF and RTP/RTCP multiplex requirements rather than a local profile.

**Current status:** the beta.4 tree now has the named fail-closed profile, `RTP/SAVPF`, RTCP
multiplexing, one ICE-nominated component carrying DTLS-SRTP/SRTP/SRTCP, and the bounded native-browser
CI proof in both SIP roles. That proof requires non-silent Opus in both directions and reverses the
fingerprint, nomination and downgrade conditions independently. `M-38` is done: the hosted
native-browser job and complete release gate passed before beta.4 was published. See
[design](designs/webrtc-audio.md).

### Video admission — `video`

Video remains a non-goal of the current [vision](vision.md), and this post-beta epic does not reverse
that decision by appearing on the roadmap. `M-40` is an admission gate: measure the cost of one
bounded send-and-receive profile, then either keep video outside sipx with the evidence recorded or
explicitly amend the vision and write a normative spec before implementation. Until that decision,
there is no video codec, SDP profile, packetizer, runtime or public support claim in scope.

The proposed maturity ladder is deliberately separate from release maturity: **0 proposed** (the
current state), **1 admitted** (vision, scope, budgets and spec agreed), **2 negotiable** (one
feature-gated codec and offer/answer profile), **3 runnable** (bounded frame pipeline and RTCP
feedback with no audio regression), **4 interoperable** (independent peer in both call roles under
clean and impaired transport), and **5 public** (packaged-consumer proof plus measured docs). A
decision not to admit video closes `M-40` at 0 rather than pretending the missing four levels are
backlog debt.

The browser-compatible audio epic is a prerequisite only for a future **browser** video claim. Its
WSS, ICE, DTLS-SRTP, RTP/SAVPF and RTCP-mux composition can be reused, but video adds its own codec,
packetization, feedback, congestion, timing and resource-safety obligations under RFC 3264, RFC
3550, RFC 4585, RFC 5104, RFC 6184, RFC 7741, RFC 7742, RFC 8834 and RFC 9429. `M-40` must resolve
that complete cost before any implementation child becomes ready.

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
