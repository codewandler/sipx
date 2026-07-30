# Conformance, capability and release-readiness review — 2026-07-30

## Executive assessment

**sipx has a stronger conformance-accounting and verification story than its product readiness.**
The reviewed checkout has a complete sans-I/O SIP transaction core, all five conventional SIP
transports in the Rust library, a real call framework, encrypted and unencrypted media, a
scriptable file-oriented phone, five fuzz targets, two independent interop profiles, and a
machine-checked registry tracking 70 RFCs. The registry check reports every claim backed, and the
workspace carries 1,520 Rust test attributes.

That is enough to call sipx a substantial SIP user-agent and media stack. It is **not enough to say
that it clears the broader combined bar of a general SIP service library, a high-level VoIP
framework, and a full test-phone binary.** The decisive gaps are reachable product paths, not
missing low-level code:

- an outbound INVITE cannot answer a 401 or 407 challenge (`S-28`);
- DTLS-SRTP and ICE exist below the call layer but a call cannot select them (`M-28`, `M-27`);
- early-dialog signalling exists but early media is not carried (`C-2`);
- media can bridge and mix, but applications cannot yet couple two calls or reach those primitives
  from a call (`C-1`, `C-6`);
- the CLI exposes only UDP and TCP, WAV files rather than microphone/speaker devices, no Opus
  selection, no interactive scenario language, and no call-load command;
- the project is an alpha, its public APIs are not frozen, the crates are not yet published, and
  the reviewed local branch is 196 commits ahead of its public remote-tracking branch.

The honest verdict is therefore **conformance depth: strong; endpoint-library capability: broadly
competitive but incomplete; general SIP-service capability: intentionally incomplete; test-phone
parity: not reached; adoption maturity: not reached.**

## Review identity and method

- Review time: `2026-07-30T11:32:06+02:00` (`Europe/Berlin`)
- Base commit: `87f4dfad1f8ea8edd6a8877c5cb8ab57b0c054a8`
- Branch: `main`, 196 commits ahead of `origin/main`
- Snapshot: the base commit plus this review story and document
- Workspace size: 12 crates, 594 tracked files, approximately 104,470 lines of Rust
- Evidence used:
  - [`vision.md`](../vision.md), the scope and design tie-breaker;
  - [`compliance.md`](../compliance.md), generated from the RFC registry;
  - [`maturity.md`](../maturity.md) and the generated [story board](../stories/README.md);
  - public crate contracts, CLI help, source, tests, fuzz targets and interop profiles;
  - `./scripts/rfc-report.py --check` and `./scripts/gate.py --list`.

The capability threshold is deliberately evaluated at three layers: protocol/service library,
high-level call and media framework, and executable phone. A feature implemented in a lower crate
does not satisfy a higher-layer row until that layer can select and exercise it. This is the same
reachability distinction the RFC registry and application-surface checks are intended to enforce.

This is a source and verification review, not a fresh multi-vendor interoperability campaign or a
performance benchmark. Existing interop evidence is credited, but no latency, throughput or calls
per second claim is made.

## Capability threshold

| Dimension | Shipped evidence | Assessment |
|---|---|---|
| SIP parsing and transactions | Incremental datagram/stream parser, typed failures, all four RFC 3261 transaction machines amended for RFC 6026, RFC 4475 and RFC 5118 corpora | **Exceeds the threshold for evidence and testability.** RFC 3261 remains correctly marked partial because there is no proxy role. |
| Conventional transports and resolution | UDP, TCP, TLS, WS, WSS; RFC 3263 NAPTR/SRV/A/AAAA selection; connection pooling and NAT response routing | **Meets at the Rust-library layer.** The CLI exposes only UDP/TCP, so the executable does not meet the same row. |
| Generic SIP services | UAC/UAS endpoint, mutable messages, stateless and transaction inputs, registration client, subscription and publication models | **Does not meet a general service-library threshold.** sipx is explicitly not a proxy, registrar or PBX; it does not fork or write Record-Route, and the B2BUA primitive is backlog work. |
| Registration and digest authentication | Registration leases, refresh, unregister, RFC 3263 routing, modern digest algorithms, Outbound, Path, Service-Route, GRUU and push-refresh support | **Strong for registration.** It is not complete call authentication: an outbound INVITE fails on 401/407 because credentials cannot enter `sipx-call`. |
| Dialog and call control | Dial, answer, cancel, ACK/BYE, re-INVITE, UPDATE, hold/resume, session timers, reliable provisional responses, blind and attended transfer, Replaces, typed event stream | **Broadly capable.** Early offer/answer is present, but the media in an early dialog is not carried. |
| Audio and RTP | G.711, selectable Opus in the Rust call library, RTP/RTCP, adaptive jitter buffer, RFC 4733 DTMF, WAV playback/recording, quality and MOS estimates | **Meets at the library layer.** The CLI is narrower: fixed WAV I/O and no codec-selection surface. |
| Secure media | SRTP, SDES negotiation on protected signalling, DTLS-SRTP implementation and tests | **Partial at the product layer.** A call hard-codes SDES and cannot offer DTLS-SRTP; the CLI cannot select an encrypted signalling transport, so no CLI invocation produces encrypted media. |
| NAT traversal | `rport`, symmetric RTP, Outbound flows, STUN binding, a full ICE agent and SDP procedures | **Implemented below the call, not shipped as a call capability.** No INVITE sent or answered by `sipx-call` carries candidates. ICE restart and relayed candidates also remain open. |
| Bridging and conferencing | Two-session media bridge and N−1 conference mixer with lifecycle tests | **Primitive exists; framework parity is not reached.** A `Call` cannot select the bridge/conference, and no object drives two dialogs as one session. |
| Phone automation | `dial`, `answer`, `register`, `peers`; WAV playback/recording; DTMF; bounded timeouts; JSON; distinct exit codes; quality stats; redacted signalling capture | **Strong shell contract, narrow phone surface.** No microphone/speaker, transcription, interactive stdin actions, codec choice, secure transport choice, arbitrary-header flag or load-test command. |
| Application SDK | Typed call events, multi-call endpoint, versioned JSON contract, sans-I/O interpreter, deterministic host harness | **Promising but experimental.** Callback bindings and the usable host/SDK path remain incomplete, so this cannot compensate for missing Rust or CLI reachability. |
| Packaging and compatibility | `1.0.0-alpha`, MSRV checked, feature matrix checked, API contracts documented | **Not adoption-ready at a stable-library threshold.** APIs are not frozen, crates are not published, and `A-9` still tracks mechanical freeze safety and per-crate READMEs. |

## Conformance status

The registry tracks **70 RFCs**: 32 implemented, 22 partial, 9 not started, 6 syntax-only and one
not applicable/superseded. That distribution must not be converted into a percentage: a partial
row can contain most of an RFC or only one reachable role, and the layers have very different
deployment weight.

The strongest evidence is concentrated where sipx's architecture is strongest:

- the whole RFC 4475 corpus and all twelve RFC 5118 messages are classified and tested;
- transaction logic takes messages and fired timers as inputs, permitting deterministic state-table
  tests and transaction-sequence fuzzing;
- malformed input is represented by typed errors, `unsafe` is forbidden, and no network parser
  needs an async runtime, socket or clock;
- five fuzz targets cover datagram parsing, stream parsing, URI parsing, round-trip invariants and
  transaction event sequences;
- interop profiles exercise two independent peers rather than treating self-agreement as proof;
- `rfc-report.py` verifies that implemented/partial rows cite Rust evidence and that claimed parser
  vocabulary exists.

The important limits are also recorded rather than hidden:

- RFC 3261 is partial because sipx supplies UAC/UAS behavior, not the proxy role;
- presence, registration and dialog event packages have usable library models, but nothing in the
  shipped binary receives SUBSCRIBE or PUBLISH from a socket;
- ICE is partial because the call layer cannot negotiate it;
- DTLS-SRTP reachability is currently overstated by lower-layer evidence and is explicitly tracked
  for correction by `M-28`;
- MESSAGE and INFO are syntax-only; SIPREC, STIR/PASSporT, History-Info and overload control remain
  absent or open work.

Three open conformance-process defects reduce confidence at the margin without invalidating the
whole registry: `X-44` says the no-fixed-sleep testing rule is not mechanically guarded, `X-45`
identifies a capture test that cannot observe the behavior in its name, and `X-46` identifies a TLS
specification claim with no configuration surface. They should remain visible beside any statement
that the alpha predicates are met.

## Verification and operational readiness

The gate is materially stronger than a single workspace test command. Its 22 local steps check its
own parity with CI, generated RFC and maturity data, pool-key and audio claims, application-surface
reachability, provenance, formatting, clippy at `-D warnings`, all-feature tests and examples, the
application contract end to end, the advertised MSRV, optional-feature combinations, and the public
documentation build. CI additionally runs bounded fuzzing, soak work, dependency policy and live
interop jobs where local prerequisites would make them misleading.

That supports confidence in the behavior that is claimed. It does not yet support a stable adoption
claim:

- the README labels the release `1.0.0-alpha` and says public APIs are not frozen;
- Rust consumers must use Git dependencies because the crates are not published;
- the local checkout is 196 commits ahead of `origin/main`, so public review and downstream use do
  not necessarily see the code assessed here;
- one shipped application defines reachability at crate granularity, which cannot prove that every
  public capability inside a reached crate is itself exercised;
- the maturity report correctly says that an absence of filed defects is not proof that no defect
  exists, and recent discovery volume remains high.

## Readiness verdict by intended use

| Intended use | Verdict | Reason |
|---|---|---|
| Sans-I/O SIP parser and transaction core | **Ready for serious evaluation** | Strong RFC corpus, fuzz and deterministic state-machine evidence; typed failures and no unsafe code. |
| Rust SIP user-agent endpoint | **Usable alpha with explicit gaps** | Calls, registration, transfers, subscriptions and all transports exist; outbound call auth, early media and selected secure/NAT paths remain missing. |
| General SIP proxy/registrar/service framework | **Not a fit** | The vision excludes proxy, registrar and PBX roles; RFC 3261 proxy behavior and forking are not implemented. |
| High-level programmable call/media framework | **Promising, not parity-complete** | Rich single-call control and media exist, but call-to-call coupling, call-level bridge/conference access, early media, ICE and DTLS selection remain open. |
| Scriptable file-based SIP test endpoint | **Usable alpha** | Deterministic WAV/DTMF/JSON/exit-code/capture workflows are strong and shell-friendly. |
| Full test phone with live audio and load generation | **Not parity-complete** | No sound-device I/O, interactive scenario input, transcription, secure/codec CLI selection or call-load command. |
| Stable dependency for external products | **Not yet** | Alpha API, unpublished crates, no external stability window, and public remote lag. |

## Prioritized closure list

The following order closes misleading or ordinary-call gaps before adding breadth:

1. **Outbound INVITE authentication (`S-28`).** Registration credentials working while ordinary
   authenticated calls fail is the clearest endpoint-level parity blocker.
2. **Make secure paths selectable (`M-28` plus a CLI transport/codec surface).** A tested DTLS-SRTP
   implementation and WSS transport do not satisfy a phone or call framework until callers can
   select them. The missing CLI surface is not currently represented by one dedicated story.
3. **Carry early media (`C-2`).** The signalling state exists; leaving the media behind produces a
   visible interoperability failure on ordinary carrier and IVR calls.
4. **Expose ICE from calls (`M-27`).** This turns substantial existing agent and SDP work into a
   deployable NAT capability. Restart and relay support can remain separately scoped and honest.
5. **Couple calls and reach bridge/conference (`C-1`, `C-6`).** This closes the largest difference
   between media primitives and a programmable VoIP framework.
6. **Decide the CLI product bar.** If parity means a full test phone, file and deliver secure
   transports, codec selection, sound-device I/O, interactive actions and bounded load generation.
   If sipx intentionally remains a WAV-oriented shell endpoint, document that as a deliberate
   non-goal and stop using a full-phone comparison as the release bar.
7. **Close adoption mechanics (`A-9` and publication).** Freeze-compatible public enums, per-crate
   READMEs, published crates and one release of observed API stability are prerequisites for a
   stable-library claim.
8. **Close the open evidence defects (`X-44` through `X-46`).** These are smaller than the product
   gaps but directly qualify statements about the gate and specifications.

STIR/PASSporT, History-Info/Reason and overload control are important deployment extensions, but
they rank after the ordinary call paths above for a general library-and-phone parity claim. They
should stay prominent on the conformance roadmap without displacing a call that cannot authenticate
or select already-implemented secure media.

## Bottom line

sipx already has the harder-to-add foundation: deterministic protocol machinery, unusually explicit
RFC accounting, adversarial testing, strong failure rules and a coherent separation between core
logic and I/O. What it lacks is the last-mile reachability and product surface that turns those
primitives into a broad library and phone.

Consequently, the correct status is **not “at least as capable across the combined stack” today**.
The defensible claim is narrower and still valuable: **sipx is a deeply verified Rust SIP user-agent
and media alpha whose protocol evidence is ahead of its deployable surface.** Closing the first five
items above would materially change that verdict; adding more registry rows before those paths are
reachable would not.
