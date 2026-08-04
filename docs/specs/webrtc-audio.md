# Spec: browser-compatible audio profile

**Status:** normative target · **Epic:** `webrtc-audio` · **Stories:** `M-48`, `M-46`, `M-49`,
`M-50`, `M-51` · **Scope:** one SIP audio stream over WSS, ICE, DTLS-SRTP and multiplexed
RTP/RTCP

Where this document and an implementation disagree, this document is right until it is changed
deliberately. The component specifications remain normative for their own protocols. This document
defines the additional composition rules: when a named browser-audio call may be offered or
answered, which event may start each protocol, which peer may supply media, and how all four packet
classes share one bounded component.

## 1. Scope and invariants

The browser-audio profile is a fail-closed call policy. It composes mechanisms sipx already has; it
does not make them defaults for an ordinary SIP call. A selected profile has all of these properties
or it has no media session:

1. signalling uses SIP over secure WebSocket as specified by [sip-tls.md](sip-tls.md) §4;
2. the SDP has exactly one active `audio` media section using `UDP/TLS/RTP/SAVPF`;
3. RTP and RTCP use one ICE component and `a=rtcp-mux` is negotiated and honoured;
4. the component's nominated pair, not the provisional SDP destination, supplies the DTLS peer;
5. the negotiated DTLS role and the fingerprint received over WSS are verified before SRTP or
   SRTCP keys are installed;
6. Opus is the primary codec, with PCMU, PCMA, comfort noise and `telephone-event` present as the
   required audio vocabulary; and
7. no missing capability or failed stage is permission to retry with SDES, plain RTP, non-ICE
   media, a second RTCP port, or another peer address.

The profile remains an endpoint feature. It is not a browser API, capture/render engine, general
WebRTC stack, or permission to weaken the ordinary SIP, ICE or SRTP contracts.

## 2. Normative references

- **RFC 5761** — multiplexing RTP and RTCP on one port. §4 defines packet distinction and the RTP
  payload-type restriction that keeps the RTCP range unambiguous; §5 defines SDP offer/answer with
  `a=rtcp-mux`.
- **RFC 7118** — SIP over WebSocket. §4 requires the `sip` WebSocket subprotocol and §5 defines one
  SIP message per WebSocket message. WSS inherits the certificate policy in [sip-tls.md](sip-tls.md).
- **RFC 7874** — mandatory-to-implement WebRTC audio formats: Opus, PCMA, PCMU, comfort noise and
  telephone events. It does not make optional Opus encoding controls mandatory.
- **RFC 8445** — ICE. §6 forms bounded checklists; §7 performs checks; §8 nominates the selected
  pair; §9 covers restarts. The detailed sipx contract is [ice.md](ice.md).
- **RFC 8825** — WebRTC overview and the separation between signalling, transport and media
  functions. It is the reason this profile composes protocols rather than claiming a browser API.
- **RFC 8827** — WebRTC security architecture. §5.5 requires DTLS-SRTP for media and binds media
  security to authenticated signalling and the peer identity assertion available to the
  application.
- **RFC 8829** — JSEP offer/answer processing. §5 covers session descriptions and §5.8 covers
  applying an answer without starting media under terms the answer did not select.
- **RFC 8834** — WebRTC media transport and RTP usage. §4 requires RTP/RTCP, congestion-safe packet
  sizing and the secure media profile; §5 defines the audio use of that transport.
- **RFC 8839** — ICE SDP offer/answer. §4.2 defines initial exchanges; §4.4 defines subsequent
  exchanges and restart; §5 defines credentials and candidates.

The composition also depends on RFC 3264 (offer/answer), RFC 4733 (`telephone-event`), RFC 5763 and
RFC 5764 (DTLS-SRTP), RFC 7587 (Opus RTP), RFC 8122 (certificate fingerprints), and RFC 8866 (SDP).
Those requirements are already specified in [srtp.md](srtp.md), [ice.md](ice.md), and
[sdp-format-identity.md](sdp-format-identity.md); this document narrows them but does not copy their
parsers or cryptographic rules.

## 3. Profile types and ownership

These are semantic types. Later stories may choose different Rust spellings, but they MUST preserve
the distinctions: an error that loses which fail-closed boundary fired cannot support the negative
proof in §10.

| Type | Variants or fields | Meaning |
|---|---|---|
| `BrowserAudioRole` | `Offerer`, `Answerer` | SIP offer/answer role; it does not imply an ICE or DTLS role |
| `DtlsRole` | `Client`, `Server` | `Client` corresponds to `a=setup:active`; `Server` to `passive` |
| `ProfileState` | states in §6 | one owner advances the profile; state never moves backwards |
| `SelectedComponent` | local base, nominated remote, ICE generation | the only address pair allowed to carry DTLS, SRTP and SRTCP |
| `ProfileError` | §8.1 | setup or renegotiation failed; no weaker policy is selected |
| `IngressClass` | `Stun`, `Dtls`, `Srtp`, `Srtcp` | a bounded datagram's class after §7.1 |
| `IngressDrop` | §7.3 | typed and countable reason a datagram changed no protocol state |

The call owns one `BrowserAudioSession`. The session owns one bound UDP component from gathering
through shutdown. ICE, DTLS and protected media borrow that component through the owner; none may
close and rebind it, and none may retain a detached duplicate after cancellation. `sipx-sdp` owns
only pure description validation. Sockets, deadlines and asynchronous queues remain in
`sipx-media`; call policy and SIP state remain in `sipx-call`.

## 4. Exact SDP profile

### 4.1 Session shape

A conforming profile description contains exactly one `m=` line, and it is an active audio line.
An offer with a second media section — including a rejected or bundled section — is outside this
profile and receives `ProfileError::MediaSectionCount`. This deliberate narrowness keeps one media
section, one ICE component and one cancellation owner equivalent. A remote description MAY carry
one-section `a=group:BUNDLE` and `a=mid`; they add no second component and are ignored. sipx neither
requires nor emits them.

The description MUST contain:

| Element | Requirement |
|---|---|
| `v=0`, `o=`, `s=-`, `t=0 0` | ordinary RFC 8866 session fields |
| `m=audio <port> UDP/TLS/RTP/SAVPF 111 0 8 13 101` | exact payload set/order for locally generated descriptions; a remote description may add safe non-colliding formats |
| `c=IN IP4/IP6 <default>` | the default candidate address, never `0.0.0.0` for hold |
| `a=sendrecv` or a negotiated RFC 3264 direction | direction; hold changes this attribute, not ICE credentials or the connection address |
| `a=rtcp-mux` | mandatory in offer and answer; no separate component 2 exists; the conventional `a=rtcp:9 IN IP4 0.0.0.0` (or IPv6 unspecified equivalent) is an ignored mux placeholder |
| `a=ice-options:ice2` | emitted by sipx; a complete remote description may instead advertise `trickle`, which does not make later trickled candidates part of this profile |
| `a=ice-ufrag`, `a=ice-pwd` | fresh per ICE generation, within [ice.md](ice.md) §13's bounds |
| one or more component-1 `a=candidate` lines | host and optionally server-reflexive candidates; component 2 is invalid for this profile |
| `a=fingerprint:sha-256 ...` | the certificate the sender will present; media level takes precedence over session level |
| `a=setup` | `actpass` in an offer; `active` or `passive` in an answer |
| the five format mappings in §4.4 | Opus, PCMU, PCMA, CN and `telephone-event` |

The locally generated payload numbers above are fixed to make shell and byte-level proofs
deterministic. A remote offer MAY use other unambiguous dynamic numbers for Opus and
`telephone-event` and MAY carry additional formats; an answer includes only the five supported
required mappings while preserving their relative offered order and dynamic numbers. Payload types
64 through 95 MUST NOT be used while RTCP is multiplexed, including for an otherwise unknown extra
format, because their marker-bit form collides with RTCP packet types under RFC 5761 §4.

### 4.2 Offerer

Before serialising an offer, the offerer MUST establish all of these reversible facts:

1. the `opus`, `ice`, `dtls`, `tls` and WebSocket build capabilities needed by the profile exist;
2. a local DTLS certificate and SHA-256 fingerprint exist;
3. the one UDP component is bound and gathering has completed under the selected host or STUN
   policy; and
4. fresh local ICE credentials and the component-1 candidate set exist.

The capability check is the first operation. `ProfileError::OpusUnavailable` and any other missing
build capability therefore return before a socket is bound, a gatherer is started, or any network
I/O occurs. Only after that preflight may certificate creation, binding and gathering begin. Only
after every item succeeds may the offerer emit `UDP/TLS/RTP/SAVPF`, `a=rtcp-mux`, the gathered
candidates, `a=setup:actpass`, its fingerprint and §4.4's formats. A later preparation failure
closes the bound component and returns before SIP signalling; it does not send a partial offer.

An answer is acceptable only if it retains the exact protocol, multiplexing, usable ICE
credentials and component-1 candidates, a supported SHA-256 fingerprint, one of `active` or
`passive`, and Opus plus the auxiliary audio vocabulary. `active` makes the answerer the DTLS
client and this offerer the server. `passive` makes this offerer the client. `actpass`, `holdconn`,
or an absent answer role is incompatible. The offerer validates that complete relation, including
the payload numbers and their offered relative order, before generic codec settlement, accepting
the peer description into its ICE agent, sending ACK, or starting any media protocol.

### 4.3 Answerer

The answerer validates the whole remote offer before binding or gathering. It MUST reject the
profile rather than answer under another media policy when the offer:

- arrived over anything other than WSS with the `sip` subprotocol;
- does not have the one-section shape from §4.1;
- names another protocol token, omits `a=rtcp-mux`, carries a real separate RTCP destination,
  carries `a=ice-mismatch`, supplies no usable component-1 candidate, or supplies a component-2
  candidate; the port-9 unspecified RTCP placeholder is not a destination and is ignored;
- has no supported fingerprint or uses MD2/MD5 as already forbidden by [srtp.md](srtp.md) §6.1;
- gives `a=setup` anything except `actpass`; or
- lacks an offered Opus format or the mandatory auxiliary formats from §4.4.

After validation, the answerer binds the component, gathers, and emits its own credentials,
candidates, fingerprint and format intersection. It answers `a=setup:active` by default, following
RFC 5763 §5 so its `ClientHello` opens its own NAT binding. A configured policy MAY answer
`passive`; it MUST NOT answer `actpass` or copy the offer without resolving the role.

A valid answer preserves the five required payload numbers and their relative offered order. It
does not echo additional formats it does not implement and does not add a format the offer omitted.
If that leaves no Opus, PCMU, PCMA, comfort-noise or telephone-event mapping required by this named
profile, it rejects the entire profile rather than quietly creating a narrower call.

### 4.4 Audio vocabulary

| Local payload | Mapping | Profile rule |
|---:|---|---|
| 111 | `a=rtpmap:111 opus/48000/2` | required primary codec; absence or unavailable Opus build feature refuses the profile |
| 0 | `a=rtpmap:0 PCMU/8000` | required G.711 fallback format; the static mapping remains valid if the line is omitted by a peer |
| 8 | `a=rtpmap:8 PCMA/8000` | required G.711 fallback format; same static-number rule |
| 13 | `a=rtpmap:13 CN/8000` | required comfort-noise format for the 8 kHz G.711 formats |
| 101 | `a=rtpmap:101 telephone-event/8000`; sipx emits `a=fmtp:101 0-16` | required event format; absent `fmtp` means the RFC 4733 default events 0–15, and an explicit range must cover 0–15; it is not selected as the audio codec |

Opus's RTP clock and channel mapping are always `48000/2` even when the application supplies mono
audio, per RFC 7587 §7. Optional Opus `fmtp` parameters do not change format identity. This first
profile neither emits nor promises FEC, DTX, CBR, stereo or bitrate controls. It MAY accept
well-formed optional parameters it does not act on, but MUST NOT echo a parameter as an agreement
unless the media runtime implements it.

The first mutually supported audio codec in the offer's order is selected, excluding CN and
`telephone-event`. For this named profile Opus MUST be present and available, so the generated
offer places it first and the complete vectors negotiate it. A peer cannot remove Opus to steer the
session to G.711 while still claiming this profile.

### 4.5 What is never in the profile

- No `a=crypto`; the protocol token selects DTLS-SRTP and an SDES key in this description is a
  `ProfileError::WeakerMedia` rather than ignored input.
- No `RTP/AVP`, `RTP/SAVP`, `UDP/TLS/RTP/SAVP`, or clear signalling alternative. The feedback
  profile's trailing `F` is part of this profile's identity.
- No component-2 candidates or usable separate `a=rtcp` destination. The port-9 unspecified mux
  placeholder is tolerated but grants no authority; RTCP follows the nominated component-1 pair.
- No `a=ice-mismatch` fallback. Generic SIP calls retain [ice.md](ice.md) §13.4's fallback; a named
  browser-audio call required ICE and fails when it cannot use it.
- No early protected media. RTP, RTCP or DTLS from the provisional SDP address cannot nominate
  itself by arriving first.

The call framework's reliable-provisional media APIs are not entry points for this profile. They
return the typed `Error::DtlsEarlyMedia` before binding a media component or sending an INVITE;
changing the profile's fixed keying instead returns `ProfileError::WeakerMedia` at the same pre-I/O
boundary. An ordinary browser-audio dial may observe provisional SIP responses while it waits, but
it does not instantiate media from them: only a valid final answer can begin ICE checking.

## 5. DTLS, ICE and key ordering

The order is a security boundary, not merely a startup preference:

```text
valid offer/answer
        │
        ▼
ICE checks ── nomination ──► bind DTLS peer to nominated pair
                                  │
                                  ▼
                         negotiated-role handshake
                                  │
                                  ▼
                   verify signalled fingerprint and profile
                                  │
                                  ▼
                         export and install SRTP keys
                                  │
                                  ▼
                         start SRTP and SRTCP media
```

The component owner processes authenticated ICE connectivity checks before nomination. It MUST NOT
start a DTLS handshake until ICE reports a nominated pair for component 1. At nomination it records
the local base, remote socket address and ICE generation as one immutable `SelectedComponent`.
Only that remote address may supply DTLS, SRTP or SRTCP for that generation.

The handshake runs in the role resolved by §4.2/§4.3 and is bounded by five seconds, matching the
existing DTLS call path. Completion is not key installation. The owner obtains the peer certificate,
hashes it with the signalled algorithm, compares it in constant time, verifies the negotiated SRTP
profile, exports the key block, and only then atomically installs both SRTP and SRTCP directions.
Every failure discards provisional exporter material. There is no state in which one direction or
only RTP has keys.

## 6. State machines

### 6.1 States

| State | Resources that may exist | Media permitted |
|---|---|---|
| `Idle` | policy only | none |
| `Preparing` | certificate, one bound component, gathering operation | none |
| `OfferPending` | local description and component | STUN gathering only; no peer checks until the remote description exists |
| `AnswerPending` | validated remote offer; gathering in progress | STUN gathering only |
| `IceChecking` | both descriptions, ICE agent and component owner | authenticated STUN only |
| `Nominated` | immutable selected pair for this ICE generation | STUN; DTLS may now start from the selected peer |
| `DtlsHandshaking` | negotiated role, expected fingerprint, temporary DTLS adapter | STUN and DTLS from the selected peer |
| `KeysInstalled` | two SRTP and two SRTCP directional contexts | protected media may start atomically |
| `Running` | all contexts and bounded workers | STUN keepalives, SRTP and SRTCP from the selected peer; later DTLS is counted and refused |
| `RestartChecking` | old running generation plus new ICE checklist | old generation continues media; new generation accepts authenticated STUN only |
| `Renegotiating` | running generation plus a pending compatible description | existing protected media only |
| `Closing` | cancellation token set; workers being joined | no newly received media is delivered |
| `Closed` | no socket, queue, key or task | none |
| `Failed` | stored `ProfileError`; cleanup proceeds as for `Closing` | none |

`Failed` is observable but not a resource-owning terminal shortcut: entering it starts the same
cleanup path as cancellation, and the operation resolves only after the owned workers have joined.

### 6.2 Offerer transitions

| State | Input | Guard | Action and next state |
|---|---|---|---|
| `Idle` | select browser-audio | all build features available | create certificate, bind and gather → `Preparing`; otherwise fail before signalling |
| `Preparing` | gathering complete | at least one component-1 candidate | serialise §9.2, send offer → `OfferPending` |
| `OfferPending` | provisional response | any | retain resources; do not start DTLS or media |
| `OfferPending` | final success with SDP | §4.2 answer valid | apply remote description, start ICE → `IceChecking` |
| `OfferPending` | final success without valid SDP | — | `ProfileError::IncompatibleAnswer` → `Failed` |
| `OfferPending` | refusal, timeout or cancellation | — | `Closing` |

### 6.3 Answerer transitions

| State | Input | Guard | Action and next state |
|---|---|---|---|
| `Idle` | remote offer | whole §4.3 offer valid | retain description, bind and gather → `AnswerPending` |
| `Idle` | incompatible offer | — | return a typed refusal suitable for SIP 488; remain `Idle` with no media resources |
| `AnswerPending` | gathering complete | at least one component-1 candidate | serialise §9.3, send answer, apply both descriptions, start ICE → `IceChecking` |
| `AnswerPending` | gather failure or cancellation | — | send no successful answer; `Closing` |

### 6.4 Shared transport transitions

| State | Input | Guard | Action and next state |
|---|---|---|---|
| `IceChecking` | authenticated check | ICE rules | advance ICE only; remain `IceChecking` |
| `IceChecking` | nominated component 1 | exact current ICE generation | freeze `SelectedComponent`, resolve DTLS role → `Nominated` |
| `IceChecking` | ICE failure/setup deadline | no nominated pair | `ProfileError::NoNominatedPair` → `Failed`; do not use SDP default |
| `Nominated` | start handshake | selected peer and compatible role | create bounded adapter → `DtlsHandshaking` |
| `DtlsHandshaking` | handshake complete | certificate fingerprint and SRTP profile verify | export and atomically install all contexts → `KeysInstalled` |
| `DtlsHandshaking` | timeout, wrong certificate, no profile | — | named error → `Failed`; install no key |
| `KeysInstalled` | workers registered with owner | all queues/tasks created or none | enable protected send and delivery together → `Running` |
| `Running` | unauthenticated, wrong-peer or replayed packet | — | typed counted drop; remain `Running` |
| any nonterminal | cancellation, CANCEL, BYE or owner drop | — | close admissions, cancel children, drain/join owned tasks → `Closing` → `Closed` |

### 6.5 Renegotiation and ICE restart

A subsequent offer or answer MUST repeat the protocol token, `a=rtcp-mux`, the fingerprint/setup
contract, ICE credentials and candidates, and required formats. An omission is not inheritance; it
is `ProfileError::ProfileRemoved`.

| Change | Required behavior |
|---|---|
| direction, required-payload order or session timer only; payload mappings, ICE credentials and fingerprint unchanged | enter `Renegotiating`; keep the selected pair, DTLS association and keys; atomically apply the accepted direction/codec at answer |
| neither ICE credential changes | not an ICE restart; candidates may be repeated but do not create a new generation |
| both peer `ice-ufrag` and `ice-pwd` change | create a new ICE generation and enter `RestartChecking`; keep old protected media flowing until the new generation has new keys |
| exactly one ICE credential changes | malformed restart; reject the exchange and keep the old generation unchanged |
| fingerprint or setup changes without both ICE credentials changing | reject the exchange; this bounded profile does not re-key a live pair in place |

For a valid restart, the old `SelectedComponent`, DTLS association and keys remain active while the
new checklist runs. New-generation DTLS does not start until new nomination. After its fingerprint
verification and atomic key installation, the owner switches the generation in one operation and
then destroys the old contexts. A failed restart leaves the old running generation intact and
reports the restart failure; it does not end an otherwise healthy call or send new media on the SDP
default address.

### 6.6 Hangup and cancellation

Cancellation is accepted in every nonterminal state and is idempotent. It performs this order:

1. stop accepting new profile commands and new protected media delivery;
2. cancel ICE timers, DTLS work and media workers through the one owner token;
3. close every bounded sender so a blocked receiver wakes;
4. join every owned task; then zeroize/drop provisional exporters and installed keys;
5. close the component; and
6. enter `Closed` and resolve all waiters.

A SIP CANCEL before the answer, a BYE after it, an application cancellation and dropping the call
all use this path. No fixed sleep stands in for completion. A deadline may bound failed cleanup, but
expiry aborts and joins the remaining owned tasks before returning; it never detaches them.

## 7. One-component ingress and resource bounds

### 7.1 Classifier

The component owner checks length, then the first byte, then — only for the RTP/RTCP range — the
second byte. It does not attempt one parser and fall through to another.

| Bytes | `IngressClass` | Basis |
|---|---|---|
| first byte `0..=1` | `Stun` | RFC 5764 §5.1.2; the STUN parser then verifies cookie, length, integrity and fingerprint |
| first byte `20..=63` | `Dtls` | RFC 5764 §5.1.2; the DTLS record parser owns record validation |
| first byte `128..=191`, second byte `192..=223` | `Srtcp` | RFC 5761 §4; the RTCP packet type remains visible under SRTCP |
| first byte `128..=191`, any other second byte | `Srtp` | RTP version 2 and the non-colliding payload space |
| empty, one-byte RTP/RTCP prefix, or any other first byte | no class | typed drop from §7.3 |

The profile never allocates from a length claimed inside an unauthenticated packet before checking
it against §7.2. Payload types 64 through 95 are refused in SDP, so an SRTP marker bit cannot turn
one of those payloads into the SRTCP range.

### 7.2 Bounds

| Resource | Bound | Full/oversize behavior |
|---|---:|---|
| active media sections | 1 audio | refuse profile before binding |
| ICE components | 1 | component-2 candidate refuses profile |
| remote candidates | 32 for component 1 | refuse description; no partial candidate set |
| candidate line | 512 ASCII octets | refuse description before storing it |
| ICE checklist | 100 pairs | prune lowest priority as [ice.md](ice.md) §6.3 requires |
| inbound UDP datagram | 2048 octets | read with one extra sentinel octet; oversize is dropped and counted, never truncated into a parser |
| outbound UDP payload | 1200 octets | packetise below the bound or return a typed send error; do not rely on IP fragmentation |
| STUN/ICE handoff queue | 64 datagrams | non-blocking refusal; increment `stun_queue_refusals` |
| DTLS handoff queue | 64 datagrams while handshaking | non-blocking refusal; increment `dtls_queue_refusals`; DTLS retransmission provides recovery |
| SRTP ingress queue | 64 packets | drop newest and increment `srtp_queue_refusals`; sequence/replay state is unchanged |
| SRTCP ingress queue | 32 packets | drop newest and increment `srtcp_queue_refusals`; replay state is unchanged |
| DTLS handshake | 5 seconds | `ProfileError::DtlsTimeout`, no keys |
| profile-owned tasks | at most 6 simultaneously | DTLS handshaking uses the preparation supervisor + ingress owner + ICE driver + temporary DTLS worker (4); the supervisor and DTLS worker finish before running adds SRTP sender + playback queue + decoded-audio worker + SRTCP reporter (6 total); never one task per packet or candidate |

These are implementation ceilings, not advertised network preferences. A future change may lower a
queue after measurement. Raising a bound requires a spec change and a test that demonstrates why
the additional retained hostile input is needed.

Every handoff preserves FIFO order inside its class. A full queue drops the arriving item rather
than awaiting capacity in the socket owner: blocking the owner on SRTP would also stop ICE
keepalives and DTLS retransmissions. No queue is unbounded and no overflow creates a new task.

### 7.3 Source, state and drop disposition

STUN is handed to the ICE agent under [ice.md](ice.md)'s credential and source rules. Before
nomination, every DTLS, SRTP and SRTCP datagram is `IngressDrop::BeforeNomination`. After nomination,
those classes are accepted only from the selected remote address for the current generation;
another source is `IngressDrop::WrongPeer`. Before `KeysInstalled`, SRTP and SRTCP are
`IngressDrop::KeysUnavailable` and are not saved for later replay.

`IngressDrop` has at least these stable reasons:

| Reason | Counter | State effect |
|---|---|---|
| `Empty` | `ingress_empty` | none |
| `TruncatedClassPrefix` | `ingress_truncated_prefix` | none |
| `Oversized` | `ingress_oversized` | none |
| `UnknownProtocol` | `ingress_unknown_protocol` | none |
| `BeforeNomination` | per-class `*_before_nomination` | none |
| `WrongPeer` | per-class `*_wrong_peer` | none |
| `KeysUnavailable` | `srtp_keys_unavailable` / `srtcp_keys_unavailable` | none |
| `Malformed` | per-class `*_malformed` | none |
| `AuthenticationFailed` | per-class `*_authentication_failures` | none; error text does not reveal which key byte differed |
| `Replay` | `srtp_replays` / `srtcp_replays` | none |
| `QueueFull` | the four queue-refusal counters in §7.2 | none |
| `UnexpectedDtls` after the handshake | `dtls_unexpected_records` | none |

Every discard increments exactly one reason before the bytes are released. Counters are monotonic
and low-cardinality; peer addresses and packet bytes are not labels. Malformed network input never
panics, allocates from an unchecked length, changes ICE/SRTP replay state, or reaches a different
class's parser.

### 7.4 Runtime authority and observable facts

The component ingress gate is media-owned and advances in one direction for an ICE generation:
`IceChecking` → `Nominated` → `DtlsHandshaking` → `KeysInstalled` → `Running`. The socket owner and
the gate are distinct implementation objects only so the security decisions can be tested without
I/O; the socket owner MUST consult that one gate before handing a datagram to any protocol. A call
layer flag, an SDP default address, or the first protected packet to arrive MUST NOT bypass it.

Only the ICE driver's selected-pair output may create `SelectedComponent`. Only a DTLS result that
has already verified the signalled fingerprint and protection profile may advance the gate to
`KeysInstalled`. The type accepted at that transition MUST therefore be constructible only by the
verified DTLS path, not from raw exporter bytes. The transition installs the two directional master
key-and-salt pairs as one value; RTP and RTCP contexts are derived from that same value when media
starts.

The runtime exposes a read-only snapshot containing the current state, the selected local and remote
addresses, the ICE generation, and §7.3's counters. It never exposes key material. The selected codec
is a media-session fact and is not copied into this component snapshot: `M-51` combines the
session's codec with the component's selected pair, generation, state and counters when it reports
the independent proof. A snapshot taken while traffic is flowing is individually monotonic but not
an atomic transaction across every counter.

DTLS admission closes at `KeysInstalled`. This profile does not retain a post-handshake DTLS
association or expose DTLS application data; a later record in `KeysInstalled` or `Running` is
`UnexpectedDtls`. That makes §6.1 and §7.3 one rule and prevents an unconsumed association channel
from becoming a second unbounded input path.

## 8. Fail-closed errors

### 8.1 Setup and negotiation

| `ProfileError` | Boundary |
|---|---|
| `OpusUnavailable` | required build feature/encoder/decoder absent before offer or answer resources are committed |
| `InsecureSignalling` | profile selected on something other than authenticated WSS with the `sip` subprotocol |
| `MediaSectionCount` | not exactly one audio media section |
| `WrongProtocol` | an initial offer's `m=` token is not exactly `UDP/TLS/RTP/SAVPF` and does not claim a weaker media mode |
| `RtcpMuxRequired` | `a=rtcp-mux` absent, declined or contradicted by component 2 |
| `IceRequired` | credentials/candidates absent, malformed, over bound, or `ice-mismatch` selected |
| `SetupRole` | offer is not `actpass`, answer is not `active`/`passive`, or local side cannot hold the selected role |
| `FingerprintRequired` | no supported well-formed fingerprint |
| `CodecSetIncomplete` | required §4.4 vocabulary missing or Opus not selectable |
| `WeakerMedia` | an answer or re-offer selects SDES, plain RTP, non-mux RTCP, non-ICE media, or carries `a=crypto`; this takes precedence over `WrongProtocol` |
| `ProfileRemoved` | a subsequent description omits a mandatory profile element |
| `NoNominatedPair` | ICE fails or its setup deadline ends before component 1 is nominated |
| `DtlsTimeout` | five-second handshake bound expires |
| `FingerprintMismatch` | presented certificate does not match the signalled fingerprint |
| `NoSrtpProfile` | DTLS selects no supported SRTP profile |
| `Cancelled` | owner cancellation won; cleanup has completed before the error is returned |

All errors before a successful answer are suitable for an application to map to SIP 488 or local
setup failure. The exact SIP status remains call policy, but it MUST NOT be a second attempt under a
weaker media mode. Errors after answer end or retain the call according to §6.5; none installs
partial keys.

## 9. Byte-level vectors

### 9.1 Encoding convention

`BA-SDP-O1` and `BA-SDP-A1` below are US-ASCII. Each displayed line is followed by the two octets
`0d 0a`, including the last line. There is no leading byte, indentation, trailing space, UTF-8 BOM,
or extra empty line. Replacing each displayed newline with CRLF therefore yields the exact vector.
Tests MUST consume these vectors or derive fixtures byte-for-byte from this section; a visually
similar SDP with different policy is not the vector.

| ID | Length | SHA-256 of the encoded bytes |
|---|---:|---|
| `BA-SDP-O1` | 555 octets | `44fd3d3cc886a667f3b89d50c5bb7453ce985d24851252660c25c8399ae12c25` |
| `BA-SDP-A1` | 563 octets | `518f6918170dc6bd118b653df7db3d4a4136f94cd38c973c6ee5f49784c0343e` |
| `BA-SDP-B1` | 1298 octets | `451fd0acdd766200f1f5b711d92cac518f7242558ff722b1cb440d544f47c75f` |

### 9.2 `BA-SDP-O1` — complete offer

```text
v=0
o=- 496232 1 IN IP4 192.0.2.10
s=-
t=0 0
a=ice-options:ice2
m=audio 49170 UDP/TLS/RTP/SAVPF 111 0 8 13 101
c=IN IP4 192.0.2.10
a=sendrecv
a=rtcp-mux
a=ice-ufrag:ofr1
a=ice-pwd:offerPassword0123456789AB
a=candidate:1 1 UDP 2130706431 192.0.2.10 49170 typ host
a=fingerprint:sha-256 00:01:02:03:04:05:06:07:08:09:0A:0B:0C:0D:0E:0F:10:11:12:13:14:15:16:17:18:19:1A:1B:1C:1D:1E:1F
a=setup:actpass
a=rtpmap:111 opus/48000/2
a=rtpmap:0 PCMU/8000
a=rtpmap:8 PCMA/8000
a=rtpmap:13 CN/8000
a=rtpmap:101 telephone-event/8000
a=fmtp:101 0-16
```

Required result: the offer parses as one browser-audio section; component 1 is the only ICE
component; Opus payload 111 is selected; no socket address is yet a DTLS peer.

### 9.3 `BA-SDP-A1` — complete answer

```text
v=0
o=- 772211 1 IN IP4 198.51.100.20
s=-
t=0 0
a=ice-options:ice2
m=audio 53000 UDP/TLS/RTP/SAVPF 111 0 8 13 101
c=IN IP4 198.51.100.20
a=sendrecv
a=rtcp-mux
a=ice-ufrag:ans1
a=ice-pwd:answerPassword0123456789A
a=candidate:1 1 UDP 2130706431 198.51.100.20 53000 typ host
a=fingerprint:sha-256 20:21:22:23:24:25:26:27:28:29:2A:2B:2C:2D:2E:2F:30:31:32:33:34:35:36:37:38:39:3A:3B:3C:3D:3E:3F
a=setup:active
a=rtpmap:111 opus/48000/2
a=rtpmap:0 PCMU/8000
a=rtpmap:8 PCMA/8000
a=rtpmap:13 CN/8000
a=rtpmap:101 telephone-event/8000
a=fmtp:101 0-16
```

Required result: the offerer becomes the DTLS server; ICE checks begin; neither
`198.51.100.20:53000` nor the offer default is accepted for DTLS until that exact pair is nominated.

### 9.4 `BA-SDP-B1` — completed native-browser offer

This vector was captured only after the native browser reported ICE gathering complete. Its
volatile identifiers are retained because the exact bytes are evidence. The accepted shape is the
contract: one-section BUNDLE/mid, `trickle` capability with a complete host candidate already
present, the port-9 unspecified RTCP placeholder under mux, and safe extra formats. The answer
contains only `111 0 8 13 126`, preserving the five required mappings' relative offered order.

```text
v=0
o=- 6190024055914035375 2 IN IP4 127.0.0.1
s=-
t=0 0
a=group:BUNDLE 0
a=extmap-allow-mixed
a=msid-semantic: WMS
m=audio 52175 UDP/TLS/RTP/SAVPF 111 63 9 0 8 13 110 126
c=IN IP4 192.168.68.52
a=rtcp:9 IN IP4 0.0.0.0
a=candidate:3370245473 1 udp 2113937151 192.168.68.52 52175 typ host generation 0 network-cost 999
a=ice-ufrag:Oxrs
a=ice-pwd:1FMgxGqFxm0ynDDjASZyytlm
a=ice-options:trickle
a=fingerprint:sha-256 86:9C:49:68:4F:32:C7:67:61:B5:F7:C1:12:5F:8E:30:24:6A:2A:50:2B:1C:C1:2C:6B:3B:CF:43:03:B1:2E:E5
a=setup:actpass
a=mid:0
a=extmap:1 urn:ietf:params:rtp-hdrext:ssrc-audio-level
a=extmap:2 http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time
a=extmap:3 http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01
a=extmap:4 urn:ietf:params:rtp-hdrext:sdes:mid
a=sendrecv
a=msid:- 26756219-a927-4fa5-8e3d-ba8c62bf5ef3
a=rtcp-mux
a=rtcp-rsize
a=rtpmap:111 opus/48000/2
a=rtcp-fb:111 transport-cc
a=fmtp:111 minptime=10;useinbandfec=1
a=rtpmap:63 red/48000/2
a=fmtp:63 111/111
a=rtpmap:9 G722/8000
a=rtpmap:0 PCMU/8000
a=rtpmap:8 PCMA/8000
a=rtpmap:13 CN/8000
a=rtpmap:110 telephone-event/48000
a=rtpmap:126 telephone-event/8000
a=ssrc:2005259182 cname:xhHzorYIekgQwPXO
a=ssrc:2005259182 msid:- 26756219-a927-4fa5-8e3d-ba8c62bf5ef3
```

The reverse native-browser boundary is also normative: when answering O1 it may advertise
`ice-options:trickle`, include the same port-9 RTCP placeholder, and omit telephone-event `fmtp`.
The absent `fmtp` means events 0–15. The five mappings, mux, candidate, fingerprint, and resolved
setup role remain mandatory.

### 9.5 SDP negatives

Each mutation begins from the complete pair and changes exactly the named bytes.

| ID | Mutation | Required result |
|---|---|---|
| `BA-SDP-N1` | remove `a=rtcp-mux\r\n` from the answer | `RtcpMuxRequired`; no DTLS start |
| `BA-SDP-N2` | replace answer token `UDP/TLS/RTP/SAVPF` with `RTP/SAVP` | `WeakerMedia`; no SDES retry |
| `BA-SDP-N3` | replace `a=setup:active` with `a=setup:actpass` | `SetupRole`; no handshake |
| `BA-SDP-N4` | remove the answer fingerprint | `FingerprintRequired`; no handshake |
| `BA-SDP-N5` | remove answer candidates | `IceRequired`; no send to the `c=`/`m=` default |
| `BA-SDP-N6` | remove payload 111 and its rtpmap from the answer | `CodecSetIncomplete`; no G.711 downgrade |
| `BA-SDP-N7` | append `a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:dGVzdA==\r\n` | `WeakerMedia`; SDP key bytes are not consumed |
| `BA-SDP-N8` | append a second `m=video` section | `MediaSectionCount`; no partial audio profile |
| `BA-SDP-N9` | append a component-2 candidate | `RtcpMuxRequired`; no second socket |
| `BA-SDP-N10` | present a certificate whose SHA-256 digest differs in the last octet | `FingerprintMismatch`; zero installed contexts |

### 9.6 Classifier vectors

Hex strings are complete UDP payloads. Classification is asserted before parser outcome. Each
negative must increment exactly the named counter and leave every protocol state unchanged.

| ID | Bytes | Stage/source | Required result |
|---|---|---|---|
| `BA-PKT-S1` | `00 01` | any | class `Stun`, then truncated STUN → `stun_malformed += 1` |
| `BA-PKT-D1` | `16 fe fd 00` | nominated peer | class `Dtls`, then truncated record → `dtls_malformed += 1` |
| `BA-PKT-R1` | `80 6f` | running, nominated peer | class `Srtp`, then truncated RTP/SRTP → `srtp_malformed += 1` |
| `BA-PKT-C1` | `80 c8` | running, nominated peer | class `Srtcp`, then truncated RTCP/SRTCP → `srtcp_malformed += 1` |
| `BA-PKT-U1` | `40 00` | any | no class → `ingress_unknown_protocol += 1` |
| `BA-PKT-E1` | empty | any | no class → `ingress_empty += 1` |
| `BA-PKT-T1` | `80` | any | no class → `ingress_truncated_prefix += 1` |
| `BA-PKT-B1` | 2049 zero octets | any | no class → `ingress_oversized += 1`; no 2048-byte prefix is parsed |
| `BA-PKT-P1` | `16 fe fd 00` | before nomination | class `Dtls`, dropped as `BeforeNomination` before the DTLS parser |
| `BA-PKT-P2` | `80 6f` | running, wrong source | class `Srtp`, dropped as `WrongPeer` before replay/authentication state |
| `BA-PKT-K1` | `80 c8` | nominated, before keys | class `Srtcp`, dropped as `KeysUnavailable`; not retained |

The four malformed-class rows are deliberately tiny. Their first byte is sufficient to select the
parser and insufficient for that parser to read its header, which proves that classification is not
being confused with validation.

### 9.7 State and cancellation vectors

| ID | Scripted events | Required result |
|---|---|---|
| `BA-STATE-1` | apply O1/A1; deliver DTLS before nomination; nominate; complete handshake; present matching certificate | early DTLS counted; handshake starts only after nomination; all four contexts install together; `Running` |
| `BA-STATE-2` | as above with wrong certificate | `FingerprintMismatch`; no key or media worker; all tasks joined |
| `BA-STATE-3` | running call; re-offer with unchanged credentials/fingerprint and `sendonly`; answer `recvonly` | same component/keys; direction changes atomically; no ICE or DTLS restart |
| `BA-STATE-4` | running call; both sides change both ICE credentials; nominate new pair; verify new DTLS fingerprint | old media continues until new contexts install; one atomic generation switch; old keys destroyed afterwards |
| `BA-STATE-5` | running call; only peer ufrag changes | renegotiation refused; old generation remains running unchanged |
| `BA-STATE-6` | cancel separately in `Preparing`, `IceChecking`, `DtlsHandshaking`, `RestartChecking`, and `Running` | each reaches `Closed`; no socket, timer, queue, key or task remains; second cancel is harmless |

## 10. Explicit omissions

This contract does not include, and completion MUST NOT imply:

- TURN allocation or a relayed candidate. [ice.md](ice.md) and `M-24` own that future widening;
  relay-required networks remain unsupported by the first profile.
- video SDP, video RTP payloads, frame capture/rendering, congestion policy for video, or combined
  audio/video sessions.
- data channels, SCTP, browser JavaScript APIs, DOM integration, a browser media engine, or a GUI.
- multiple media sections, simulcast, incremental candidate trickling, ICE-lite, or multiple ICE
  components. One-section BUNDLE/mid and a remote `ice-options:trickle` capability token are
  tolerated only when the description already contains a complete usable candidate set; sipx does
  not promise or accept later candidate delivery in this profile.
- arbitrary application codecs or optional Opus controls not implemented by `sipx-audio`.
- a general claim of WebRTC compatibility. `M-51` may claim only the exact host or
  server-reflexive audio path its independent proof demonstrates.

These omissions are part of the profile. A future story widens them by changing this spec first;
an implementation does not widen them by accepting an attribute accidentally.

## 11. Implementation and evidence map

| Contract | Owner | Evidence story |
|---|---|---|
| SDP grammar/profile validation, with no socket or clock | `sipx-sdp` | `M-46`, `M-49` |
| named policy, offer/answer state and typed refusal | `sipx-call` | `M-49` |
| `a=rtcp-mux` RTP/SRTCP distinction and setup roles | `sipx-sdp`, `sipx-rtp`, `sipx-media` | `M-46` |
| one component, queues, nominated-peer binding, DTLS and atomic key install | `sipx-media` | `M-50` |
| SRTP/SRTCP transforms and separate replay state | `sipx-rtp` | `M-47`, `M-50` |
| WSS transport and signalling integrity | `sipx-transport` | existing [sip-tls.md](sip-tls.md) evidence |
| bounded two-sipx composition | call/media/CLI tests | `M-50` |
| both roles against an independently implemented endpoint and public claims | interop harness and public docs | `M-51` |

Later tests cite the stable vector IDs in §9. A child story may add more cases, but it MUST NOT
replace O1/A1, weaken a negative, or introduce a second profile contract in its own prose.
