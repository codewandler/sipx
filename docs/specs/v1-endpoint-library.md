# Spec: v1 endpoint library

**Status:** normative for the content of `1.0.0`, and internally satisfied on 2026-08-14. This
document fixes the bounded endpoint-library surface that stable promotion commits to. Every row in
the [exit table](#10-work-order-and-exit-evidence) is delivered and the integrated local gate is
green. The five stable-readiness predicates in the [roadmap](../roadmap.md#100--the-predicates)
remain separate and all remain required; repository evidence cannot manufacture external adoption.

This is an umbrella product contract. It defines no new wire format, state machine, timer or test
vector. The subsystem specifications linked below remain normative for those details, and their
byte-level vectors remain the implementation tests. In this document **MUST**, **MUST NOT**,
**SHOULD** and **MAY** have the meanings given by RFC 2119 and RFC 8174.

## 1. The boundary

The v1 product is a production **SIP user-agent endpoint library**. It lets an application originate
and receive audio calls, register an endpoint, survive ordinary network boundaries, and control the
result through public Rust APIs. It implements the UAC and UAS roles needed by those endpoint
behaviours. It does not become the network service between endpoints.

“Complete” in this contract means complete for that declared endpoint role. It does not mean every
SIP RFC, every method, or every role described by an RFC. A capability already implemented outside
this boundary is not silently removed: its crate-level Stability declaration continues to govern
it. Such a capability is not a reason to delay v1 unless this document or the exit table admits it.

The following are not v1 endpoint-library roles:

- proxy, registrar or location service;
- PBX, routing engine, dial plan, trunk controller, session border controller or IMS node;
- a packaged B2BUA (an application MAY compose endpoint call legs using library primitives);
- SIP recording server or relay service;
- browser SDK or general WebRTC stack;
- video, data channels or a general media engine; and
- speech recognition, speech synthesis, AI integration, or built-in/custom DSP products.

The diagnostic phone remains a shell-reachable consumer and proof surface. The application host and
the language-neutral application contract remain separate products; neither defines whether this
Rust endpoint library is complete.

## 2. Normative sources

The endpoint contract is derived from primary protocol specifications and this repository's more
specific normative specifications:

- RFC 3261 for SIP UAC/UAS, transactions, dialogs and the core call lifecycle;
- RFC 3262 for reliable provisional responses and PRACK, RFC 3311 for UPDATE, RFC 3515 for REFER,
  and RFC 4028 for session timers;
- RFC 3263 for SIP server selection, RFC 3581 for `rport`, RFC 3327 for Path, RFC 3608 for
  Service-Route, RFC 5626 for Outbound, and RFC 5627 for GRUU;
- RFC 7616 for digest authentication, RFC 5922 for SIP domain identity over TLS, and RFC 7118 for
  SIP over WebSocket;
- RFC 8866 and RFC 3264 for SDP offer/answer; RFC 3550 and RFC 3551 for RTP, RTCP and the audio RTP
  profile; and RFC 4733 for telephone events;
- RFC 3711 and RFC 7714 for SRTP, RFC 4568 for SDES, and RFC 5763/RFC 5764 for DTLS-SRTP;
- RFC 8445, RFC 8489, RFC 8839 and RFC 8656 for ICE, STUN, SDP ICE signalling and TURN; and
- the repository specifications for [SIP messages](sip-message.md), [transactions](sip-transaction.md),
  [transport](sip-transport.md), [TLS](sip-tls.md), [authentication](sip-auth.md),
  [SDP/ICE](ice.md), [SRTP](srtp.md), [media runtime](media-runtime.md),
  [early media](call-early-media.md), [initial offers](call-initial-offer.md),
  [UPDATE](sip-update.md), [target resolution](sip-target-resolution.md) and
  [dialog extensions](dialog-extensions.md).

Where this umbrella and a subsystem specification appear to disagree on wire behaviour, the
subsystem specification decides the wire behaviour and this document decides whether that
behaviour is required for v1. A real conflict MUST be resolved in both documents before promotion.

## 3. SIP roles and call lifecycle

### V1-SIP-1 — endpoint roles

The stable library MUST originate and receive endpoint requests. It MUST expose client and server
transactions, dialog state and a high-level call owner without requiring an application to
reimplement transaction matching, retransmission or dialog routing. The sans-I/O boundary remains
load-bearing: `sipx-sip` and `sipx-sdp` perform no socket I/O and read no clock.

The production endpoint method set is:

- `INVITE`, `ACK`, `CANCEL` and `BYE` for call establishment, cancellation and teardown;
- `OPTIONS` for endpoint reachability;
- `REGISTER` as a client of an external registrar;
- `PRACK` for reliable provisional responses;
- `UPDATE` and re-INVITE for in-dialog session changes;
- `REFER` and the corresponding `NOTIFY` lifecycle for transfer; and
- application-owned in-dialog `INFO`, `MESSAGE` and extension requests, with typed results.

The general subscription, notification and publication machinery already present in `sipx-ua` MAY
remain available under its own Stability declaration. Its higher-level API shape is not a v1
content blocker unless `A-41` admits it to the frozen endpoint surface.

### V1-SIP-2 — complete ordinary calls

Both UAC and UAS call roles MUST support:

- an initial SDP offer in the INVITE and an offerless INVITE whose offer/answer exchange completes
  later without binding a second media lifetime;
- provisional responses, reliable provisional responses, PRACK and early media;
- rejection, transaction timeout and cancellation races as typed outcomes;
- a separate ACK for a 2xx response, and finite cleanup when the final response crosses CANCEL;
- BYE from either endpoint;
- re-INVITE and UPDATE, including hold and resume;
- session refresh and expiry under the negotiated session timer;
- blind and attended transfer; and
- RFC 4733 DTMF plus application-owned dialog requests.

Every background operation MUST be bounded and cancellation-safe. Dropping or cancelling the
owning registration, call, media session or endpoint MUST converge on joined work and released
network resources. A fixed sleep MUST NOT stand in for an observable ordering relation.

### V1-SIP-3 — registration and endpoint identity

The library MUST provide registration leases rather than one-shot REGISTER construction. It MUST
handle refresh, expiry, authentication challenge/retry, Path, Service-Route, learned GRUU values and
an application-supplied Outbound instance identity. It MUST expose typed terminal and retryable
outcomes.

Outbound is operational only when the driver owns each registered flow for the lifetime advertised
by the binding, sends the negotiated keepalive, reconnects with bounded RFC 5626 backoff, and tears
the flow down when its owner is cancelled. Registering `;ob` and then dropping the connection does
not satisfy this requirement. `T-47` delivered that lifetime proof through the shared bounded flow
owner and its joined cancellation path.

Push parameters and the wake-then-refresh behaviour already supported by the registration client
remain supported, but operating a push service is outside this contract.

## 4. Signalling transports and resolution

### V1-TRN-1 — stable transport set

One endpoint API MUST originate and receive calls over UDP, TCP, TLS, WebSocket and secure
WebSocket. The same transaction and dialog semantics apply on all five; transport selection MUST
NOT create a second call API.

Stream framing, connection reuse and connection pooling MUST follow the bounded keying and limits
defined by [`sip-transport.md` §8](sip-transport.md). A secure target MUST NOT silently fall back to
a cleartext transport. TLS and WSS MUST verify the configured peer identity and trust policy, and
failures MUST be typed.

### V1-TRN-2 — target resolution

The public endpoint path MUST accept a SIP target rather than requiring every application to
pre-resolve a socket address. RFC 3263 selection, explicit transport parameters, address-family
fallback and connection-attempt diagnostics MUST be reachable through the library surface. Resolver
work, attempts and caches MUST obey configured bounds and cancellation.

The repository's SIP-over-QUIC mapping is Experimental and is not part of the v1 transport promise.
Local-link discovery is also outside v1; it names possible peers rather than resolving an already
chosen SIP target.

## 5. Audio media

### V1-MED-1 — offer/answer and codecs

The endpoint library MUST offer, answer, send and receive one telephony-audio media stream. The v1
baseline codecs are G.711 mu-law, G.711 A-law, G.722 and mono L16, with their specified RTP clocks
and payload mappings. Negotiation MUST bind the selected payload type, codec, clock and packet size;
an unavailable codec is a typed refusal and never a different codec under the negotiated payload.

Opus MAY be enabled through its existing optional feature. It remains outside the v1 baseline API
guarantee unless `A-41` graduates that feature and its configuration surface; omitting the feature
MUST remain a supported build.

### V1-MED-2 — RTP ownership

Each call MUST own its RTP/RTCP session and its media workers. The public call surface MUST provide
playback, recording, DTMF, RTP/RTCP quality observations and finite media teardown without exposing
a shared mutable session as the coordination model. Packet parsing and media setup failures MUST be
typed and MUST NOT panic on received bytes.

RTCP sender/receiver reporting, sequence rollover, replay windows, jitter buffering and symmetric
RTP MUST be real behaviour, not only SDP declarations. Bridging and conferencing remain usable
library composition features, but a routing product built from them is not part of this contract.

## 6. Security

### V1-SEC-1 — signalling and authentication

The endpoint MUST support digest challenge and response in both endpoint directions for the
algorithms its authentication specification marks supported. Credentials, private keys, digest
responses and media key material MUST NOT appear in ordinary diagnostics.

TLS/WSS policy MUST be caller-configurable without disabling certificate-name verification by
accident. An explicitly secure SIP target or explicitly secure media policy MUST fail closed.

### V1-SEC-2 — media security

The call API MUST let an application select plain RTP, SDES-keyed SRTP or DTLS-SRTP through typed
policy. SDES MUST be refused unless the signalling protection satisfies the SRTP specification's
confidentiality condition. DTLS-SRTP MUST verify the offered fingerprint before accepting exported
keys. Neither path may downgrade an explicitly requested secure policy.

The supported transforms are `AES_CM_128_HMAC_SHA1_80`, `AEAD_AES_128_GCM` and
`AEAD_AES_256_GCM`. RFC vectors prove transforms that have vectors; independent-peer evidence MUST
prove both AEAD suites over both SDES and DTLS-SRTP, because a shared key-derivation error can make
two sipx endpoints agree with each other. `M-72` records that independent evidence and its
failing-first negative for both suites and both keying paths.

Rekeying, MKI, MIKEY, header-extension encryption and SRTP transforms not named above are excluded.

## 7. NAT traversal and reachability

### V1-NAT-1 — signalling reachability

The endpoint MUST implement `rport`, received-address handling, Path, Service-Route, GRUU and the
Outbound lifetime in V1-SIP-3. A flow is usable only while the task and connection carrying it are
owned, observed and cancelled together.

### V1-NAT-2 — media reachability

The call API MUST expose explicit ICE policy and MUST gather host, server-reflexive and configured
relayed candidates. It MUST perform checks, nominate a pair, bind media to the nominated peer and
fall back among viable candidate types without treating an unavailable configured relay as a failed
call when another pair succeeds.

A configured relay means a TURN client allocation with long-term credentials, refresh,
CreatePermission and Send/Data handling under RFC 8656. Operating the TURN service is excluded.
`M-24` delivered the client lifecycle and bounded fallback path.

After ICE concludes, a subsequent offer from the controlling endpoint MUST generate the
`remote-candidates` attribute required by RFC 8839 from the nominated remote candidates. Answers
and offers from a controlled endpoint MUST omit it. It MUST NOT echo arbitrary remote input or
advertise a pair that was not nominated. `M-130` delivered this generation-bound offer behavior.

ICE-lite service behaviour is excluded. A full endpoint MUST still interoperate with an ICE-lite
peer where RFC 8445 assigns the full side the checks.

## 8. Stable Rust API guarantee

### V1-API-1 — the usable high-level surface

An application MUST be able to configure, register, dial, answer, reject, cancel, end, hold,
resume and transfer calls through public library APIs. It MUST be able to select the stable
transport, codec, ICE and keying policies named by this document without assembling private
lower-layer state or depending on the diagnostic CLI.

`A-41` classifies every high-level codec, ICE, keying, media-profile and registration-lifecycle type
needed by this document. Each is either graduated into the v1 Supported surface or replaced by a
smaller supported shape. A required capability MUST NOT remain reachable only through an
Experimental item at stable promotion.

#### V1-API-1.1 — frozen endpoint configuration

The Supported configuration boundary for an audio call is one opaque, non-exhaustive
`sipx_call::CallConfig` value. `CallConfig::new` takes a `MediaAddress`; its builders select a
`MediaPolicy` and the local initial `Direction`. The same value MUST be accepted without
translation by `DialOptions::with_call_config` for the UAC role and
`Invitation::answer_with_config` for the UAS role. The answer entry point MUST apply the selected
direction while constructing its SDP answer; it is not enough for the option to exist only on the
offering side.

The following configuration types are Supported and form the complete high-level audio-policy
vocabulary: `CallConfig`, `MediaAddress`, `MediaPolicy`, `MediaProfile`, `Codecs`,
`CodecPreference`, `IcePolicy`, `TurnPolicy`, `Keying`, `SrtpSuite`, and `Direction`. Public enums
whose vocabulary can grow are non-exhaustive. `Direction` is the protocol-closed RFC 3264 set and
remains exhaustive. Configuration structs are opaque and non-exhaustive, with constructors,
builders and read-only accessors; a downstream application does not use a struct literal to bind
its source to fields added later.

`TurnPolicy::new` prepares caller-supplied username and password values under RFC 8265's
`OpaqueString` profile, then applies the USERNAME byte bound to the prepared value, all before a
socket is bound. A prohibited value is a field-specific typed error rather than a delayed Allocate
failure.
`IcePolicy::Turn` configures host gathering plus one TURN allocation; relay failure does not remove
otherwise viable host candidates. `MediaPolicy::with_srtp_suite` is an exact requirement and is
valid only with explicitly selected SDES or DTLS-SRTP. Every combination known to be invalid from
configuration and build features MUST return a typed error before media binding, worker spawn, or
SIP transmission.

Authenticated identity has direction-specific cryptographic state, so it is not hidden inside
`CallConfig`: `DialOptions::with_identity(OutboundIdentityPolicy)` signs UAC attempts and
`Dispatcher::with_identity(InboundIdentityPolicy)` verifies requests before a UAS invitation is
surfaced. Both entry points and both policy types are Supported. This role distinction is explicit;
it MUST NOT produce separate codec, ICE, keying, media-address, or direction policy shapes.

The Supported operation map is:

| Operation | Public entry point and public types |
|---|---|
| configure media | `CallConfig`, `MediaAddress`, `MediaPolicy`, `MediaProfile`, `Codecs`, `CodecPreference`, `IcePolicy`, `TurnPolicy`, `Keying`, `SrtpSuite`, `Direction` |
| register and retain reachability | `sipx_ua::Config`, `Flows::for_instance`, `Flows::add`, `Flows::start`, `FlowLifetime`, `FlowEvent`, `FlowLifetime::cancel`, `FlowCleanup`, `InstanceId`, `RegId`, `Power` |
| dial and cancel establishment | `DialOptions`, `DialOptions::with_call_config`, `dial`, `dial_until`, `Dialing::cancel` |
| dispatch, answer and reject | `Dispatcher`, `Dispatched`, `Invitation`, `Invitation::answer_with_config`, `Invitation::refuse` |
| established lifecycle | `Call`, `Call::hang_up`, `Call::reinvite`, `Call::refer`, `Call::refer_attended`, `serve` |

This table classifies the high-level endpoint route. Lower-level transaction, SDP, ICE-agent,
backend handshake, manual registration-pass, and browser-specific APIs retain the Stability
classification of their own crate and are not pulled into Supported merely because the endpoint
route uses them internally.

#### V1-API-1.2 — defaults and feature absence

`CallConfig::new(MediaAddress::new(address))` MUST mean G.711 PCMU then PCMA, no ICE,
`Direction::SendRecv`, and `Keying::Auto`. `Keying::Auto` preserves the existing rule: SDES only
over protected signalling and plain RTP otherwise. It is a compatibility default, not permission
to weaken `Keying::Sdes` or `Keying::DtlsSrtp`.

Optional codecs and native security backends MUST NOT change these defaults when enabled. A known
option absent from a build remains representable and produces a typed preflight error; it MUST NOT
be ignored or replaced. The supported feature-off configuration therefore compiles the same
configuration vocabulary even where attempting to execute one of its unavailable choices is
refused.

### V1-API-2 — compatibility

At `1.0.0`, every item classified Supported by a published crate is covered by the workspace's
SemVer promise. Additive evolution uses constructors, builders, `#[non_exhaustive]` and defaulted
trait methods where appropriate. Experimental items remain plainly marked and are not covered by
that promise.

`X-149` establishes an API-compatibility baseline from the final release candidate and makes an
unreviewed breaking change to Supported surface fail a focused check. The guard ignores items that
the same release explicitly classifies Experimental; widening the frozen surface by accident is as
misleading as narrowing it.

The supported default-feature and no-default-feature configurations for each crate MUST continue to
compile. Optional native dependencies MUST remain opt-in where their crate contract says they are.

## 9. Explicit exclusions and non-inference

The following do not block v1 and MUST NOT be inferred from the word “complete”:

- proxy forwarding, registrar/location storage, record routing, forking policy or dial plans;
- PBX, IMS, trunk, SBC, packaged B2BUA or high-availability cluster operation;
- SIPREC, every event package, every SIP extension, or complete RFC-percentage coverage;
- SIP over QUIC, local-link endpoint discovery or operating DNS, STUN, TURN, registrar, proxy or
  push infrastructure;
- video, screen sharing, data channels, browser packaging or a general WebRTC engine;
- speech, AI service integrations, acoustic models, echo cancellation, noise reduction, automatic
  gain control, audio effects or a general DSP graph; and
- stability of the language-neutral application protocol, application host bindings or a browser
  SDK.

An exclusion is not a prohibition on later work. It means the feature can ship after v1 without
changing the truth of the v1 endpoint-library claim. If later work needs a new role, it gets its own
specification and support statement instead of retroactively widening this one.

## 10. Work order and exit evidence

Every row marked **blocking** is required before `1.0.0`. The table intentionally carries no live
status; story frontmatter and the generated board own status. Closing a story without satisfying its
Acceptance does not satisfy this table.

| Work | Order | Blocking | Exit evidence |
|---|---:|:---:|---|
| `M-24` — configured relayed ICE candidate | 1, parallel | yes | Allocate/Refresh/permission/data lifecycle, related address, graceful relay failure and a relayed media path satisfy the ICE spec and story Acceptance. |
| `M-72` — independent AEAD key-derivation proof | 1, parallel | yes | Both AEAD suites carry verified bidirectional media through both SDES and DTLS-SRTP, with pinned independent evidence and a negative that detects the competing derivation. |
| `T-47` — Outbound flow lifetime | 1, parallel | yes | The public registration owner keeps bounded flow(s) alive, drives keepalives and backoff, observes failure and joins all work on cancellation. |
| `M-130` — ICE `remote-candidates` | 1, parallel | yes | A subsequent controlling offer emits exactly the nominated remote candidate for each concluded component; answers, controlled streams and unvalidated input omit it. |
| `A-41` — high-level API freeze | 2, after protocol shapes | yes | Every type needed by V1-API-1 is Supported or replaced; rustdoc and registry-only consumer code cover both SIP roles and every policy family. |
| `X-149` — compatibility guard | 3, after API freeze | yes | The final-RC Supported surface is recorded; focused checks reject breaking removal/signature changes while permitting documented Experimental evolution. |

Protocol work in order 1 MAY run concurrently. `A-41` follows it so the frozen shapes describe the
delivered behaviour rather than a guessed future seam. `X-149` follows the freeze so it records the
candidate actually offered for external adoption.

All six blocking rows and their story Acceptance are complete. The 55-step local gate passed over
their integrated implementation on 2026-08-14; this closes the content gate, not the separate
stable-promotion evidence gate below.

Stable promotion then requires **both**:

1. every blocking row above is done with its Acceptance and focused evidence satisfied; and
2. all five existing `1.0.0` predicates in the roadmap hold, including independent application use
   of the public API and two-peer interoperation for every claimed transport.

The complete local gate, the clean registry-consumer proof and the release procedure are final
verification of that conjunction. They do not waive a content row or manufacture external adoption
evidence.
