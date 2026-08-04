# Spec: independent browser-audio proof harness

**Status:** normative for the `M-51` harness · **Profile:**
[browser-compatible audio](webrtc-audio.md) · **Scope:** process lifecycle, independent browser
role, evidence and CI ownership

## 1. Boundary

The peer page uses the browser's native `RTCPeerConnection`, `WebSocket`, Web Audio and statistics
interfaces. It contains no sipx parser, SDP builder, ICE agent, DTLS adapter, RTP implementation or
codec. Its small SIP message loop exists only to carry the browser-created SDP over WSS in the two
endpoint roles.

The harness is compatibility evidence, never design authority. Protocol behavior remains owned by
RFCs and `docs/specs/`. A browser disagreement is a failing result to investigate, not permission to
copy a peer quirk into the profile.

This first harness proves only a host or server-reflexive, one-component audio path. It does not
claim TURN, video, data channels, browser APIs as a sipx product surface, or a general WebRTC stack.

## 2. Inputs and identity

One invocation receives:

- a WebDriver URL or executable discovered from an explicit environment override and then the
  runner's executable search;
- a headless browser capability object;
- the local `file:` URL of the committed peer page;
- a WSS URI whose host is a DNS identity, never an unchecked IP literal;
- the expected SHA-256 subject-public-key pin for the WSS test certificate; and
- one executable sipx role command for browser-offerer and one for browser-answerer.

The certificate is issued per run and its private key is never committed. The runner rejects a
missing, malformed or all-zero pin before starting WebDriver. The browser capability must enforce
that exact pin while leaving general certificate-error bypass disabled. A Python TLS preflight also
loads the run's fixture CA, verifies the WSS DNS name, and compares the peer certificate's public-key
pin before the page may connect. Both checks must succeed; either check alone is insufficient.

WebDriver's browser discovery is operational provisioning, not part of the compatibility claim. A
driver or browser that cannot start headlessly, expose `RTCPeerConnection`, or enforce the supplied
pin is an unavailable proof environment and exits nonzero. It is never a skip reported as success.

## 3. Roles and event protocol

The runner executes these cases serially:

| Case | Browser action | sipx command responsibility |
|---|---|---|
| `browser-offerer` | create and send the INVITE offer; apply the answer; ACK; exchange media; BYE | listen on WSS, answer, report its selected path |
| `browser-answerer` | accept INVITE; apply offer; create/send answer; exchange media; accept or send BYE | place the WSS call, report its selected path |

The driver supplies one bounded JSON configuration and receives one terminal object with
`contract: "sipx.browser-audio.v1"`, a `role`, and a stable `type`. The terminal browser result has:

- `codec`: MIME type, payload type and clock rate from the selected inbound/outbound codec stats;
- `security`: WSS identity pin, DTLS state, DTLS setup role and SRTP cipher/profile from transport
  statistics;
- `candidate_pair`: selected pair identifier, nomination state, local/remote candidate types,
  addresses and ports;
- `media`: inbound/outbound packet and byte counts, received audio energy and sent oscillator
  frames; and
- `sip`: INVITE, final response, ACK, BYE and the BYE final response for the selected role.

The sipx command emits its own terminal JSON. The harness preserves both objects and validates them
independently before emitting a combined result. It never substitutes a browser inference for a
missing sipx runtime fact. It also cross-checks the browser's local endpoint against sipx's
nominated remote endpoint and the browser's remote endpoint against sipx's nominated local one;
the reversal is the same pair viewed from opposite ends.

## 4. Positive decision

A role succeeds only when all of these are observed in that same run:

1. WSS connected with subprotocol `sip` under the pinned identity;
2. one audio transceiver negotiated Opus at 48 kHz;
3. ICE reached `connected` or `completed`, and the statistics-selected candidate pair is nominated;
4. DTLS is connected, the answer selected `active` or `passive`, and a non-empty SRTP profile is
   reported;
5. inbound and outbound RTP packet and byte counts are nonzero;
6. the synthetic outbound track produced frames and the inbound track reports nonzero audio energy;
7. INVITE, final response, ACK and BYE occurred in the role-correct order; and
8. the sipx terminal object independently reports browser-audio, Opus, keyed media, the same role
   and a nominated component-1 pair.

The overall proof succeeds only after both roles satisfy the list. Process exit without all facts is
failure. A terminal result from only one role is failure.

## 5. Negative non-vacuity

Wrong-fingerprint, missing-nomination and weaker-answer cases each run as a pair:

1. the unchanged positive fixture reaches the boundary beyond the one being mutated; then
2. exactly one input is mutated and the named failure must occur before its deadline.

Each negative names a positive role and carries the canonical SHA-256 digest of that role's already
validated browser and sipx terminal objects. The validator recomputes that digest; a boolean claim
that a paired positive ran is not evidence. Each negative binds the exact typed sipx refusal to the
native browser's independent statistics from that mutation. The wrong-fingerprint case must select
and nominate ICE, reach DTLS certificate verification, and deliver no RTP to the browser. The
browser may remain in DTLS `connecting` or briefly report `connected` before it observes the peer's
typed fingerprint refusal; the sipx `FingerprintMismatch` result is the authoritative key-install
boundary. Missing nomination must show that ICE started and the native peer closed before any pair
was selected or nominated, fail as
`NoNominatedPair`, show no DTLS start, and carry no RTP. The weaker answer must fail as
`WeakerMedia` before browser ICE/DTLS and
must show that no fallback was attempted. A peer that could not
complete the paired positive makes the negative vacuous and therefore red.

## 6. Bounds and cleanup

One shell process owns the complete invocation. WebDriver, each sipx role command and any helper are
started as separate process groups and recorded immediately. `EXIT`, `INT` and `TERM` use one cleanup
path:

1. stop admissions;
2. send `TERM` to every live process group;
3. wait a finite grace period while reaping leaders;
4. send `KILL` to the groups still alive; and
5. wait for every recorded leader before returning.

The complete proof is bounded at five minutes; one role at two minutes; WebDriver readiness at ten
seconds; browser setup, ICE, DTLS and media evidence at the smaller bounds owned by their specs.
After the sipx role reports its WSS listener, its first role-specific browser method is a causal
readiness event: the role waits for that method without adding a separate first-method deadline;
the sipx command's enclosing 90-second operation bound is its sole product-side bound. A timeout is
failure and names the owning phase. No fixed sleep stands in for readiness: polling waits only for a
declared endpoint/event and the overall deadline bounds failure.

Each terminal object, WebDriver response and process output is capped at 1 MiB. Candidate and pair
identifiers are capped at 256 characters and retained browser errors at 4,096 characters. Cleanup runs on success and every
failure, including malformed evidence and an interrupted shell.

## 7. CI and harness self-test

The real proof job must install or discover a compatible headless browser/WebDriver, issue the WSS
fixture identity, and run both roles. Until the M-49/M-50 product commands exist, CI runs only the
harness self-test and does not publish a compatibility result.

The self-test reverses the harness's own trust boundaries with fixture processes:

- a timed-out helper that forks a grandchild proves process-group cleanup and reaping;
- malformed, partial and oversized JSON prove structured-fact assertions fail closed;
- a one-role result proves both-role completeness is required;
- each negative without its paired positive proves non-vacuity is enforced; and
- a pin mismatch proves no page or sipx role starts after identity preflight fails.

When real role commands land, the CI job replaces the infrastructure-only invocation with the real
two-role proof and the gate contract names that job explicitly. Until then, no generated report or
public page may say the browser-audio acceptance positive passed.
