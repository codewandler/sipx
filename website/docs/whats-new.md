---
title: What's new
description: Release highlights and adoption notes for the sipx 1.0.0-rc.2 release candidate.
---

# What's new

<!-- BEGIN generated:release-heading -->
## 1.0.0-rc.2 — 2026-08-05
<!-- END generated:release-heading -->

RC.2 is sipx's first published release candidate. It gathers the complete post-beta.7 transport, media,
signalling, observability and distribution work into one immutable version for external review. It
does not move or overwrite an existing tag, package or asset.

The immutable RC.1 cut was not published: its protected gate passed, then the registry rehearsal
found one stale internal version requirement and stopped before any package, archive or release
record was created. RC.2 corrects that manifest edge and adds a live-graph regression test.

- **Endpoints handle deployment edges explicitly.** Oversized UDP requests follow RFC 3261's TCP
  fallback before transaction creation, and applications can drain new-dialog admission while
  established calls and in-dialog work finish under a bounded deadline.
- **Observability stays application-owned.** Redacted signalling can leave the existing bounded
  capture path as non-blocking HEP3 datagrams, and per-stream RTCP loss, jitter and round-trip
  samples reach a callback that survives media replacement and ICE restart. No metrics backend is
  bundled, and collector failure cannot fail a call.
- **The audio boundary is no longer tied to one sample format.** Applications can play and capture
  explicit 8-bit or 16-bit mono PCM at supported caller-selected rates. One bounded streaming
  resampler serves PCM, WAV and device paths; L16 is negotiable at 44.1 kHz or 8 kHz.
- **Routing and privacy edits remain parser-owned.** TEL parameters have a typed allocation-free
  iterator, address presentation and Warning agents can be replaced atomically without normalising
  surrounding bytes, and genuine application provisional responses keep long-ringing server
  transactions answerable without weakening the finite abandonment guard.
- **Three field traps are fixed.** Bodyless re-INVITEs complete delayed offer/answer, dynamic RTP
  payload numbers remain directional, and one transient registration-refresh failure receives a
  bounded retry inside the granted lease.
- **Release evidence is portable and reviewable.** The protected release attaches five native CLI
  archives, per-target SPDX documents and checksums after a native loopback call on each target.
  The retained endpoint-responder comparison now has two compatible runs; their supported
  intervals overlap at the tested ceiling, so the result is inconclusive rather than a ranking.
- **The public architecture page explains the core/driver seam.** It shows where bytes and fired
  timers enter, which crates own I/O, and why the split enables virtual-time and network-free core
  tests.

Install the exact CLI release with:

```bash
cargo install --locked --version =1.0.0-rc.2 sipx-cli
```

The [getting-started guide](getting-started.md#prebuilt-release-binaries) also shows the exact binary
archive, checksum and SPDX path. Those portable executables deliberately omit optional native
features; use Cargo when `device-audio`, `opus` or `dtls` is required.

The one source migration in RC.2 renames `sipx_transport::Config::mtu` to `path_mtu`: pass the path
MTU as `Some(value)`, or `None` for the RFC 3261 unknown-path rule. Supported APIs are still not
frozen before stable 1.0 and receive migration guidance when they change; Experimental APIs may
change or disappear without that guide. The project still has no recorded independent production
application or third-party security audit, so this candidate is an invitation to review, not a
claim that repository evidence can substitute for outside use.

## 1.0.0-beta.7 — 2026-08-05

Beta.7 publishes the routing-integration wave after beta.6. It is a new immutable prerelease and
does not move or overwrite any existing tag or package.

- **One public operation cancels one exact outgoing INVITE transaction.** It is anchored to the
  original response stream, owns CANCEL construction and the provisional/final-response race, and
  returns distinct typed outcomes. The call layer now uses the same operation.
- **Cleartext listener selection is exact.** Endpoints select UDP only, TCP only, both, or no
  cleartext listener when another signalling listener is configured. TCP-only no longer opens an
  undeclared UDP socket.
- **Privacy and identity fields are typed.** Checked Privacy values and strict-send/tolerant-receive
  asserted identity lists keep syntax validation and indexed diagnostics inside the SIP layer.
- **URI editing is parser-owned and lossless.** SIP/SIPS users, TEL subscribers, Request-URIs and
  nested address-field URIs can change without byte searching or rebuilding unchanged wire syntax.
  Generic percent escapes, TEL subscribers and ambiguous bare-address boundaries are validated.

Install the exact CLI release with:

```bash
cargo install --locked --version =1.0.0-beta.7 sipx-cli
```

Public APIs are not frozen before 1.0. Supported APIs receive migration guidance when they break;
Experimental APIs may change or disappear without that guide. Code that configured cleartext
listeners must replace the former TCP boolean with `CleartextTransports`; the default remains UDP
and TCP together.

## 1.0.0-beta.6 — 2026-08-05

Beta.6 publishes the integrated correctness and specification wave after beta.5. It is a new
immutable prerelease and does not move or overwrite any existing tag or package.

- **The bounded endpoint responder has less hot-path contention.** Route sweeping is amortized,
  timer generations remain exact, UDP intake uses a bounded batch and queue, completions are
  dispatched fairly, invalid capacity is a typed refusal, and BYE-before-ACK ordering drains without
  leaking dialog state.
- **Protocol validation now owns successful-response accounting.** Malformed responses cannot
  inflate qualification or headroom totals. The retained load dataset covers the current endpoint
  direction only; the peer direction remains unmeasured, and beta.6 adds no general ranking.
- **The browser SDK boundary is normative before implementation.** Browser-owned signalling,
  timers, entropy, certificate handling and WebRTC resources now have a bounded host/core contract
  with state tables and refusal vectors. The package, adapters and runnable demo remain backlog.
- **Later media and application work is explicitly planning-only.** Local speech, call-audio
  analysis, custom DSP and realtime phone actions are decomposed into constrained designs and
  stories. They are not beta.6 runtime capabilities.

Install the exact CLI release with:

```bash
cargo install --locked --version =1.0.0-beta.6 sipx-cli
```

Public APIs are not frozen before 1.0. Supported APIs receive migration guidance when they break;
Experimental APIs may change or disappear without that guide. Beta.6 preserves beta.5's runtime
surface while hardening the measurement responder and its evidence accounting.

## 1.0.0-beta.5 — 2026-08-05

Beta.5 publishes the endpoint and application wave delivered after beta.4. It is a new immutable
prerelease: it does not move or overwrite beta.2, beta.3 or beta.4.

- **Long-lived endpoints gained explicit operational seams.** A TLS or secure-WebSocket listener can
  atomically replace the certificate identity used by new handshakes without closing established
  connections. A bounded, non-blocking stream exposes parsed messages and connection transitions;
  immutable request policy can allow, reject, or append only application-owned headers; and a live
  bounded IP-prefix set can refuse new sources before parsing or handshake work.
- **SIP event and dialog services now reach the live endpoint.** Applications can serve and originate
  bounded subscriptions, discover current registrations through the registration event package,
  receive and originate conditional presence publication, and handle application-owned INFO,
  MESSAGE, or explicitly admitted private methods inside an established dialog. Confirmed quiescent
  dialogs can also be encoded as bounded versioned protocol state and attached to fresh runtime
  resources under host-owned persistence policy.
- **Testing and operations have executable public surfaces.** The published testkit now includes a
  socket-free call harness, virtual time, a finite RTP/PCMU echo peer, and a deterministic realtime
  peer. The CLI adds a bounded signalling load responder with versioned JSON evidence, while the
  logging reference fixes the library's quiet-by-default level policy.
- **The application host gained a realtime audio binding.** One routed G.711 call can bridge to one
  authenticated realtime WebSocket session with bounded queues, counted loss, barge-in, typed
  terminal outcomes, and joined cleanup. The default suite proves this contract against a
  deterministic loopback peer; the credentialed live-endpoint interoperability proof has not yet
  been recorded.
- **Capability comparison and signalling-load evidence are checked data.** The public comparison and
  compliance pages are generated from evidenced registries. The first bounded UDP dialog-signalling
  run now publishes its correctness qualification, exact revisions and environment, raw hashed
  repetitions, median and spread, unsupported direction, and post-drain zero-state. It makes no
  claim about secure transports, connection churn, audio, or an overall winner; read the
  [comparative signalling-load result](reference/comparison.md#comparative-signalling-load) with
  those limits intact.

Install the exact CLI release with:

```bash
cargo install --locked --version =1.0.0-beta.5 sipx-cli
```

Public APIs are not frozen before 1.0. Supported APIs receive migration guidance when they break;
Experimental APIs may change or disappear without that guide. The credentialed live-endpoint proof
for the realtime binding remains pending, and the comparative-load result remains one bounded UDP
responder-direction measurement rather than a general ranking.

## 1.0.0-beta.4 — 2026-08-04

The first public beta is published and remains immutable.
Beta.4 is published as exact crates.io packages. It comes from a new immutable tag without moving
or overwriting beta.2 or beta.3, and adds a bounded
browser-audio profile, explicit non-ICE deployment addresses, RTCP multiplexing, replay-safe SRTCP,
and stronger hostile-input and entropy invariants.
See [How sipx is built](reference/development-process.md) for the measured process and
[Native-browser audio proof](reference/browser-audio-proof.md) for the executable example, complete
harness command, evidence contract and deliberate boundary.

Install the exact CLI release with:

```bash
cargo install --locked --version =1.0.0-beta.4 sipx-cli
```

The optional browser-audio path needs the Opus and DTLS features:

```bash
cargo install --locked --version =1.0.0-beta.4 --features opus,dtls sipx-cli
```

The adoption surface leads with the modular Rust crates: applications select the protocol,
transport, user-agent, media, call, or host layer they need rather than taking a facade crate. The
`sipx` CLI is the shell-testable proof of those layers. Beta.4's named profile either negotiates
authenticated WSS, one ICE-nominated component, DTLS-SRTP, multiplexed RTP/RTCP and Opus as a unit,
or refuses before falling back to weaker media. A native-browser job exercises both SIP roles and
requires non-silent audio in both directions. Ordinary calls keep their existing defaults.

For deployments that do not use ICE, applications can now bind media locally while advertising a
different address; the CLI exposes that split as `--advertise` and reports both values. SRTCP has
its own authenticated replay window, and malformed SIP and oversized WebSocket input are refused
before response construction or oversized allocation.

Public APIs are not frozen before 1.0. Supported APIs receive a changelog entry and migration
guidance when they break. Experimental APIs may change shape or be removed without a migration
note; that includes the language-neutral `sipx.app.v1` wire contract and SIP over QUIC.

The release intentionally does not provide TURN for networks that require a relayed candidate,
video, data channels, SCTP, browser-facing application APIs, or a general browser/WebRTC engine.
It also does not promise stable `1.0` compatibility: Supported APIs may still change with migration
guidance, and Experimental APIs may change or disappear without that guide. Proxy, registrar, PBX,
routing-product and dial-plan roles remain outside the endpoint library. The
[fit guide](guides/does-this-fit.md) is the maintained deployment boundary.

## 1.0.0-beta.3 — 2026-08-04

Beta.3 preserved beta.2's runtime library and CLI surface while adding the checked public stack
comparison, a demand-led capability roadmap, and checksum-bound recovery for interrupted registry
publication.

```bash
cargo install --locked --version =1.0.0-beta.3 sipx-cli
```

## 1.0.0-beta.2 — 2026-08-04

Beta.2 was the first published public beta. It established the same endpoint, library, transport,
media and application-host surface described above, backed by exact registry packages, the
installed diagnostic CLI, independent transport peers and release-commit documentation.

```bash
cargo install --locked --version =1.0.0-beta.2 sipx-cli
```

Use RC.1 for new installations; beta.2 through beta.7 remain immutable for reproducible
existing consumers.

## 1.0.0-alpha.5 — 2026-08-03

This previous tagged release established a measured alpha baseline for the SIP,
transport, call, and media stack; it is not an API-stability promise. Breaking API changes are
still possible before 1.0.

Install this exact release with:

```bash
cargo install --git https://github.com/codewandler/sipx \
  --tag v1.0.0-alpha.5 --locked sipx-cli
```

### Release highlights

- **Breaking: public error enums are now `#[non_exhaustive]`.** Downstream exhaustive matches need
  a wildcard arm. This is the one-time compatibility cost that lets future diagnostic variants be
  additive instead of breaking every caller. The sole exhaustive exception is a closed set of host
  boundaries, with that reason maintained beside the type.
- **Every published crate has its own landing page.** All eleven packages now ship a README that
  says what the crate is, points to its crate-level stability contract, and names the layer or
  responsibility it deliberately leaves elsewhere.
- **The release surface is measured from five directions per crate.** The guard now compares each
  package README's lead paragraph with the manifest description, crate documentation, and both
  public crate tables: 55 front doors in total. Packaging tests also prove Cargo ships every
  README, rather than merely finding the file in a checkout.

#### Still current from earlier alphas

- **`MediaSession::collect_digits` takes two durations** (breaking in `1.0.0-alpha.3`). It took
  one, and spent it on two different questions — how long to wait for the first keypress, and how
  long a silence means the caller has stopped. Pass the old value as both arguments to keep the
  old behaviour, including the defect. `sipx answer` consequently holds a call for its full
  `--duration` when nobody dials, which is what `--duration` is documented to mean.
- **`sip-tls.md` no longer advertises a minimum-TLS-version setting.** There was no such setting;
  the specification was corrected rather than the setting invented, because the absence of a
  version-selecting API is what currently evidences the TLS floor.
- **A call can use ICE.** An application selects host gathering or a configured STUN server through
  one call-level media policy; the default selects no ICE and is unchanged. A call between two
  endpoints whose advertised addresses do not reach each other completes over a checked candidate
  pair.
- **An ICE session survives a restart.** A re-offer whose credentials have both changed begins a new
  session, and audio keeps flowing on the previously selected path until the new one is chosen.
  Every later offer and answer on a call using ICE now restates its ICE attributes, so a hold or a
  session refresh no longer reads to the far end as ICE having been switched off.
- **The diagnostic phone can select every released signalling transport.** `dial`, `answer` and
  `register` choose UDP, TCP, TLS, WS or WSS through one fail-closed policy; a secure URI scheme
  cannot be served over cleartext, and a certificate failure is reported rather than downgraded.
- **The published compliance and maturity reports are trustworthy again.** Two continuous-integration
  jobs had been reporting a discrepancy in the maturity report that did not exist, because they
  measured a repository history they had not fetched.
- **Media startup is transactional.** A media session or conference is returned only after its
  configuration and codecs have been validated. Startup failures are typed errors, and no worker
  or socket is left behind.
- **Transport resource limits cover live work.** Connection eviction now terminates the evicted
  connection, and unauthenticated TLS and WebSocket handshakes share a finite per-endpoint budget
  and deadline.
- **Invalid runtime settings fail before binding.** Zero channel capacities, connection or
  handshake limits, handshake deadlines, WebSocket keepalives, and media worker intervals are
  rejected as configuration errors rather than reaching a panic or dead worker.
- **Conference shutdown owns its workers.** Removing a participant, closing a conference, or
  dropping it initiates cleanup of participant collectors, media sessions, and sockets.
- **Codec construction cannot change the negotiated codec.** An Opus setup failure is reported
  instead of substituting G.711 bytes under the negotiated Opus payload type.
- **Media statistics are observational.** Reading current statistics no longer resets the RTCP
  reporting interval; sending a report is the operation that closes that interval.
- **CLI recordings retain received audio.** `sipx answer` and `sipx dial` wait separately for the
  first frame and for later idle, and a duration limit preserves samples recorded before it fires.
- **RFC support claims require implementation evidence.** Implemented and partial entries in the
  generated compliance table cite workspace source, so prose alone cannot make a support claim.

The alpha also includes the shipped user-agent surface described throughout this site: calls,
registration, G.711 audio, optional Opus, RTP/RTCP, secure library transports, SDES-keyed SRTP,
and the scriptable WAV-based CLI.

### Not complete in this release

ICE cannot use a relay: host and server-reflexive candidates are gathered, TURN is not implemented,
so the NAT pairs that need a relayed path are not served. A DTLS-keyed call path is still not
available, and the CLI cannot yet select codecs, media security or ICE. The CLI uses WAV files
instead of sound devices.
The experimental `sipx-host` process can bind and
answer calls, but application callback bindings are not implemented.

## Development branch after the tagged release

This website is built from `main`, so a page or API link may describe work newer than the tagged
release. Use the exact crates.io version when reproducibility matters, and consult the
[complete changelog](https://github.com/codewandler/sipx/blob/main/CHANGELOG.md) before updating a
Git revision. Unreleased behavior is not part of `1.0.0-rc.2` merely because it appears on this
site.
