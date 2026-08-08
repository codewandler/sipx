---
title: What's new
description: Release highlights and adoption notes for the sipx 1.0.0-rc.8 release candidate.
---

# What's new

<!-- BEGIN generated:release-heading -->
## 1.0.0-rc.8 — 2026-08-08
<!-- END generated:release-heading -->

RC.8 repairs five checks and contracts that were quieter than they claimed.

```bash
cargo install --locked --version =1.0.0-rc.8 sipx-cli
```

- **The discard guard covers the whole tree.** It had been listing one directory, so the largest
  module in the workspace went unscanned; the widened scan found seven unexplained discards.
- **A re-encoded RTP packet keeps its header extension**, which a forwarding path used to strip.
- **`MediaProfile`, `IcePolicy` and `Keying` are `#[non_exhaustive]`** — matching them exhaustively
  now needs a fallback arm.
- **The CLI reference no longer leaves a wrong-featured binary** for the next test run to spawn.
- **`to_string_sdp` says what it does not preserve**, so a forwarder learns it from the docs rather
  than from a peer.

## 1.0.0-rc.7 — 2026-08-08

RC.7 fixes the added-delay reports at their real source and repairs three checks that were measuring
less than they claimed.

Install the exact CLI release with:

```bash
cargo install --locked --version =1.0.0-rc.7 sipx-cli
```

- **Inbound audio is bounded in time.** The queue between a call and the application held 5.12
  seconds of audio at 20 ms packets, with no bound, counter or shed policy — an application reading
  slightly slower than real time settled at the far end and stayed there. It now holds 200 ms by
  default, measured as audio rather than frames so the bound means the same thing whatever the far
  end sends, and every shed frame is counted.
- **A refused analysis frame no longer hides a voice transition.** The gap it leaves restarts the
  epoch rather than vanishing.
- **A connection failure reports how many addresses it tried**, from the library as well as the
  phone, so "this name resolves to one dead host" reads differently from "everything behind this
  name is unreachable".
- **Process tests refuse a binary built with the wrong features**, instead of failing as though
  audio were broken.

### Upgrade edits

- An application that reads call audio slower than real time now loses the oldest audio rather than
  receiving all of it late. Raise `Config::inbound_queue` if that is not what you want, and watch
  `MediaDiscardCounts::inbound_frames_shed` to see whether it is biting.

## 1.0.0-rc.6 — 2026-08-08

RC.6 is largely about this project's own evidence — what the coverage number counts, whether a
generated report can be found, whether a story's status agrees with its acceptance, and whether a
protection profile sipx negotiates has been agreed by something that is not sipx.

Install the exact CLI release with:

```bash
cargo install --locked --version =1.0.0-rc.6 sipx-cli
```

- **Applications can name a destination instead of addressing one.** The bounded resolver moved into
  the transport crate, so a library consumer gets the same deadlines, ordering and typed failures the
  diagnostic phone has had, with the URI host still the TLS verification identity.
- **AEAD-GCM has been agreed with an independent implementation.** A native browser and sipx settle
  on `AEAD_AES_256_GCM` and carry non-silent audio both ways, with the browser deriving its own keys.
  A deliberately wrong salt offset is proved to fail that run while every published vector still
  passes. The 128-bit suite and the SDES keying path are not yet covered.
- **`register` finishes its work before it reports it.** Every exit now joins the endpoint, not just
  an expired deadline, and `--keep-alive` no longer sends a redundant second REGISTER.
- **The coverage figure stopped counting the tests.** Inline test modules are excluded by rule, and
  the published number moved from 90.13% to 86.76%.
- **The DTLS profiles are named as the registry names them.**

## 1.0.0-rc.5 — 2026-08-08

RC.5 makes a call's audio observable and its speech contract runnable, exports the signalling kernel
to WebAssembly, and adds a coupling role that stays entirely off the media path. It remains a
prerelease on the same terms as RC.4.

Install the exact CLI release with:

```bash
cargo install --locked --version =1.0.0-rc.5 sipx-cli
```

- **Calls report voice activity and signal metrics as typed events.** Both are deterministic signal
  analysis over the bounded PCM seam — **no speech model is loaded** — and both carry direction, call
  identity, an observation sequence and sample time. Delivery is bounded and cannot delay RTP; a slow
  consumer sees a sequence gap rather than a stalled call.
- **The speech contract runs.** An asynchronous driver pumps recognition and synthesis sessions off
  the seam, bounds unconsumed output, and aborts a wedged session on its drain deadline. Speech data
  is now isolated by default: nothing is retained, four opt-ins cover debug capture, persistent
  derived data, off-host processing and network egress, logs redact content while keeping identity
  and lifecycle, and cancellation erases a call's audio buffers. **Still no speech recognition or
  synthesis implementation ships** — the contract is what exists.
- **The signalling kernel runs in WebAssembly.** The sans-I/O SIP and SDP core builds without
  sockets, an async runtime, a filesystem, an OS clock or an entropy source; the host supplies bytes,
  fired timers, monotonic time and entropy across a versioned ABI. Native and wasm32 produce
  byte-identical wire output and identical events, pinned by a digest over the full RFC 4475 and
  5118 replay.
- **Two dialogs can be coupled without touching their media.** The RFC 7092 §3.1.3 role drives both
  dialogs and maps SDP transparently while binding no RTP and advertising no address of its own, so
  audio flows endpoint to endpoint. Reliable provisionals, PRACK and offerless INVITE are
  deliberately refused rather than half-relayed: mapping them would mean describing a media endpoint
  this role does not have.
- **Every command's stated deadline now bounds target resolution.** A slow name can no longer consume
  the resolver's own budget before the command's clock starts.
- **The gate records what each step costs.** Timings go to a machine-readable record alongside the
  commit, host CPU count and cache state. Nothing gates on a duration — a slow run is not a failed
  run.

### Video

Video was considered and **not admitted**. The decision is recorded with the cost measured across
every seam it would touch, and with the evidence that would reverse it, rather than left open. The
vision's telephony-audio focus is unchanged.

## 1.0.0-rc.4 — 2026-08-08

RC.4 continues the release-candidate line. It attaches application audio processing to a live call
through one bounded seam, negotiates the RFC 7714 AEAD-GCM protection profiles, gives registration a
deadline it previously lacked, completes named-target resolution with the proofs that tell its three
failures apart, and publishes a generated coverage figure that nothing gates on. It remains a
prerelease on the same terms as RC.3.

Install the exact CLI release with:

```bash
cargo install --locked --version =1.0.0-rc.4 sipx-cli
```

- **Applications can observe live call audio through one bounded seam.** A processor attaches to a
  direction of a call, receives PCM in a format it asks for, and converts through the same linear
  boundary WAV and device audio already use. Its queue is finite with a documented loss policy: a
  processor that stops reading loses its own oldest frames and is told so by a discontinuity, and it
  can never delay RTP decode, encode, playback or capture. Attachments survive a re-INVITE.
- **Media negotiates the AEAD-GCM protection profiles.** Both the 128- and 256-bit profiles are
  offered strongest-first over SDES and DTLS, selection is by strength rather than by the order a
  peer listed them, and the counter-mode profile remains the floor. The transform is verified
  bit-exactly against RFC 7714's own published vectors. That RFC publishes no key-derivation vector,
  so interoperation with an independent implementation is not yet claimed.
- **Registration can be bounded.** `register` now takes a deadline covering resolution, the initial
  transaction and any authentication retry, and a refused connection is reported as a transport
  failure rather than as a timeout.
- **Named targets are documented and their failures are distinguishable.** Resolution failure,
  resolution timeout and connection failure are told apart in text, JSON and exit status, and the
  CLI reference describes the resolution contract instead of showing literal addresses. A new
  `SIPX_NAMESERVER` names a specific resolver when the host's own is not the one to ask.
- **Interchangeable local speech providers have an executable contract.** Registry, discovery
  descriptor, selection precedence with typed refusals, session lifecycle and per-call privacy
  admission all exist and are exercised by conformance vectors against a deliberately inert
  provider. **No speech recognition or synthesis implementation ships, and none should be inferred
  from this** — the contract is what shipped.
- **Lost audio is concealed rather than spliced.** Packets either side of a gap are no longer handed
  over back to back, which previously produced an audible step and permanent drift; discards and
  concealment are counted where every other media discard already is.
- **The test suite's reach is measured and published.** The figure is generated, never transcribed,
  and nothing gates on it — see the coverage page for what the measurement deliberately excludes and
  why the headline number reads high.

### Upgrade edits

- `register` previously had no completion deadline; it now defaults to 20 seconds. A registrar that
  legitimately takes longer than that will now fail where it previously succeeded — pass
  `--timeout 0` to restore the old behaviour.
- Constructing an SRTP context now takes the negotiated protection profile, and the keying type
  carries it. Code that built either directly needs the profile threaded through.

## 1.0.0-rc.3 — 2026-08-08

RC.3 is the second published release candidate. It answers two independent external sweeps of the
published RC.2 artifacts — each run from a fresh clone against the released archive and a pinned
registry install, with security excluded from both by declaration — and lands the work those sweeps
made ready. Twenty-five findings across call lifecycle, endpoint reachability, timeout honesty,
refusal signalling, automation contracts and published onboarding are fixed. It is a new immutable
prerelease and does not move or overwrite an existing tag, package or asset.

- **Every outbound command takes a named SIP target.** Calling, registration, bounded load,
  registrar-backed peer listing and scenario automation share one bounded resolver: NAPTR selects
  the transport, SRV the host and port, A/AAAA the addresses, and literal addresses bypass resolver
  setup entirely. Deadlines, lookup and candidate limits and the connection budget are finite, a
  `sips:` URI never falls back to a cleartext candidate, and the original hostname stays the TLS
  verification identity. The CLI reference still shows literal addresses; the behaviour is specified
  but not yet described there.
- **Calls negotiate and carry G.722.** The codec is native fixed-point sub-band ADPCM, built into
  every configuration with no feature gate and verified bit-exactly against the ITU-T Appendix II
  digital test sequences. Following RFC 3551, 16 kHz audio drives packet sizing, PCM conversion,
  capture, WAV headers and recording durations while the RTP timestamp advances at 8000. Preference
  sits below Opus and above G.711.
- **The CLI is parsed once by a typed command model.** An option a command does not define is now a
  usage error naming the flag instead of being silently discarded, and non-Unicode input is refused
  rather than panicking before startup. Command and option names, aliases, environment fallbacks,
  defaults, JSON/text separation and exit codes are preserved, and the
  [CLI reference](reference/cli.md) is generated from parser-owned help.
- **Long-running commands survive supervisor termination, not just Ctrl-C.** Calling, answering and
  both bounded-load roles route interactive interrupt and, on Unix, SIGTERM through one cancellation
  path: admission closes first, dialog, media and transport work joins within the command's
  configured bound, and exactly one terminal record follows any earlier readiness record. A clean
  stop exits 0, and platform support is typed rather than silently promised.
- **A confirmed call follows the dialog it owns and returns near its configured budget.** A queued
  remote BYE now outranks interrupt and local completion, teardown joins transport, dialog, media
  and device work to zero before one terminal result, and invitation expiry reaches a finite join
  barrier even when neither CANCEL nor INVITE is answered. Previously a peer's BYE was ignored, an
  interrupted process emitted no terminal record, and an unreported cancellation tail overshot
  short budgets.
- **Refusals reach the wire and name their real cause.** An initial offer outside the selected codec
  policy receives a transaction-owned 488 and malformed SDP a 400, so the caller reports a rejection
  promptly instead of waiting out its whole invitation timeout. A refused connection is a typed
  transport failure rather than a SIP timeout, and codec, media-security, profile, ICE and device
  selections are checked against the compiled build before resolution, bind, file or device open and
  any datagram.
- **The call module is six modules.** Hold and resume, ICE restart, offer/answer settlement,
  transfer, re-INVITE and session timers moved into private siblings, leaving the call type and its
  lifecycle in the module root. No public path, name or signature moves, so no import changes.
- **Every shipped call verb has a guide with a compiled example.** Hold and resume, blind transfer,
  attended transfer, sending and collecting DTMF, playback, recording and two-leg coupling each have
  a guide whose sample is inlined byte-exactly from a workspace example compiled with the workspace,
  and the [fit guide](guides/does-this-fit.md) links every capability it claims to the guide that
  shows it. The displayed dependency snippet is compiled by a registry-shaped consumer crate, so it
  cannot omit a package the example imports.
- **Two contracts are specified ahead of the code that will implement them.** Interchangeable local
  speech providers and deterministic real-time call-audio analysis now have normative session,
  discovery, precedence, lifecycle and refusal contracts with conformance vectors. Neither ships
  code in this candidate, and neither should be read as an available capability.

Install the exact CLI release with:

```bash
cargo install --locked --version =1.0.0-rc.3 sipx-cli
```

The [getting-started guide](getting-started.md#prebuilt-release-binaries) also shows the exact binary
archive, checksum and SPDX path. Those portable executables deliberately omit optional native
features; use Cargo when `device-audio`, `opus` or `dtls` is required. G.722 is not among them
and is present in every build.

Five changes need an edit in existing scripts and automation:

- **Undefined options exit 2.** A command that previously discarded an option it does not define now
  fails and names the flag. `register --timeout 3` ran an unbounded registration and gave no
  indication the flag was ignored; that invocation now fails instead. Remove the flag from such
  calls.
- **Cancellation has its own allowance.** `--timeout` remains the invitation-answer phase, and the
  CANCEL cleanup that follows expiry or interrupt is `--cancel-timeout <S>`, default 2, where `0`
  performs no timed cancellation wait. A script that treated `--timeout` as the total process budget
  should now set both values.
- **Bounded load defaults to signalling on both sides.** Generated media must be selected explicitly
  and symmetrically on both commands with `--mode generated-media`; an incompatible explicit pair is
  refused before dialog admission, and an invalid local mode before any I/O.
- **Scenario streams use the flat frame shape and report failure through exit status.** The
  canonical `{"id":…,"command":…,…}` form is accepted and the nested one-key-per-command shape the
  older help implied is not; `do` remains a compatibility alias only when `command` is absent. Any
  refused command or failed operation exits 1, so a later success cannot hide it.
- **`version --json` emits an object.** A consumer that parsed the previous plain-text output must
  read the stable object instead; neither output form accepts a positional argument.

The call-module split moves nothing public, so it needs no action. Supported APIs are still not
frozen before stable 1.0 and receive migration guidance when they change; Experimental APIs may
change or disappear without that guide. `register` still accepts no completion deadline, so a check
against a black-holing registrar blocks for the SIP transaction timeout rather than a stated one.
The project still has no recorded independent production application or third-party security audit,
and both sweeps excluded security by declaration, so this candidate remains an invitation to review,
not a claim that repository evidence can substitute for outside use.

## 1.0.0-rc.2 — 2026-08-05

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

Use RC.3 for new installations; beta.2 through beta.7 remain immutable for reproducible
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
Git revision. Unreleased behavior is not part of `1.0.0-rc.8` merely because it appears on this
site.
