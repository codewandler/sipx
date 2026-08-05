# Design: Bridge a call to an OpenAI realtime agent

**Status:** accepted · **Pillar:** Application · **Epic:** `openai` · **Stories:**
[A-19](../stories/A-19-specify-the-openai-realtime-bridge.md) ·
[A-20](../stories/A-20-a-wss-client-for-non-sip-peers.md) ·
[A-21](../stories/A-21-build-a-deterministic-realtime-peer.md) ·
[A-22](../stories/A-22-bridge-a-call-to-an-openai-agent.md) ·
[A-23](../stories/A-23-prove-the-bridge-against-the-live-endpoint.md)

## Why

Every capability sipx claims — TLS held to [sip-tls.md](../specs/sip-tls.md) §3, SRTP held to
[srtp.md](../specs/srtp.md), codec selection that honours the answer — has been proven against
peers chosen for their neutrality: pinned containers, loopback fixtures, a browser. What none
of those supplies is a *service*: a far end operated by someone else, driven through an
application-side control plane, that turns a phone call into something a stranger can hear
working. OpenAI's Realtime API is that shape, and it is reached over a WebSocket: the
application opens `wss://api.openai.com/v1/realtime`, configures a session, streams the
caller's audio up as base64-encoded G.711 frames inside JSON events, and receives the agent's
audio back the same way. sipx already owns everything on the telephone side of that seam —
`Call::media()` hands an application decoded samples or, in relay mode, the encoded G.711
payload itself (`recv_encoded`/`send_encoded`), so a bridge is passthrough with no transcode.
What the workspace lacks is the other side: a WebSocket client that speaks to a non-SIP peer
at all (`ws.rs`'s client mandates the `sip` subprotocol by contract), and an application
component that holds the two streams together under the host's discipline — bounded queues,
counted loss, secrets by name.

The epic buys the general shape, not a vendor shim: a bridge *specification* first
(non-negotiable 4), a WSS client and a bridge held to that spec's vectors, a deterministic
stand-in peer so the whole loop runs in the default CI matrix with no account and no
credentials, and a separately-gated live proof that records evidence once. OpenAI is named as
the interop subject the way comparison subjects are named: as a checkable fact, not as design
rationale — the rationale here cites RFCs and our own specs only.

## Approach

Five components, one story each.

1. **The bridge spec** (`docs/specs/openai-realtime.md`, A-19). Normative for this workspace,
   observational toward the vendor, with the observation date recorded. It pins: the
   endpoint URL and bearer authentication from a named secret per
   [host-config.md](../specs/host-config.md) N7; the session configuration the bridge sends
   (G.711 μ-law/A-law audio both directions — the call's own wire format, so the bridge is
   byte passthrough — server-side turn detection on); the exact client-event subset the
   bridge emits (session configuration, audio append, response cancel) and the server-event
   subset it consumes (session acknowledgement, speech-started for barge-in, audio deltas,
   done, error), each with JSON vectors including base64 audio framing; the barge-in rule —
   on speech-started, cancel the response and drop the locally queued agent audio, bounded,
   no wall-clock wait; buffering and backpressure (bounded queues both directions, counted
   drops, the session-binding discipline); connection lifecycle (a failed or closed socket
   ends the bridge with a typed outcome, no silent reconnect); and the failure taxonomy
   (auth refused, malformed event, stalled peer, oversize frame). Facts to verify against
   the vendor's published documentation, not to invent: exact event type names, session
   fields, close/error semantics.

2. **A WSS client for non-SIP peers** (A-20). The transport crate's WebSocket client is
   SIP-specific by contract (subprotocol `sip`, one message per frame). The bridge needs a
   general client: RFC 6455 over the *same* TLS policy — `tokio-tungstenite`'s handshake
   composed over `ClientTls`, one certificate discipline, not two (the workspace dep is
   built without TLS features for exactly this reason). Request headers (bearer), no
   subprotocol requirement, bounded frame and message sizes, ping/pong liveness, typed
   close. It lives in `sipx-app`: the host is where the engine, HTTP stack and
   serialization live and stop ([app-host.md](app-host.md) ground rule 4).

3. **The stand-in peer** (A-21). `sipx-testkit` gains a loopback WSS server speaking the
   spec from the other side: it authenticates the bearer, acknowledges the session, consumes
   appended audio, emits deltas carrying a distinct tone, honours cancel by stopping
   mid-response, and has negative modes — wrong bearer refused, malformed event, mid-call
   stall, oversize frame — each driving one of the spec's failure rows. No credentials, no
   network, default CI matrix: the peer criteria of
   [tests/interop/README.md](../../tests/interop/README.md) kept intact.

4. **The bridge and its product path** (A-22). An application component in `sipx-app` that
   owns one call leg and one realtime session: caller audio from `recv_encoded` up as
   append events, agent deltas down through a bounded local queue to `send_encoded`,
   barge-in per the spec, every queue bounded and every drop counted. Reached the way the
   host reaches everything: configuration under host-config discipline (endpoint URL,
   model, instructions, secret *name*), and a CLI path that answers or originates a call
   and hands it to the bridge so the loop is demonstrable with one command. Proven end to
   end against the stand-in with the M-39 evidence pattern: facts asserted (G.711
   passthrough both directions, tone correlation, barge-in truncates the queued audio),
   non-vacuity negatives that must fail (wrong bearer never bridges, a stalled peer ends
   the bridge within its bound).

5. **The live proof** (A-23). Opt-in and credentialed, which no existing peer is allowed to
   be — so it lives outside the default matrix under the gate's disclaim-don't-skip
   doctrine: absent credential or unreachable network exits `EX_TEMPFAIL`, never a silent
   pass. One real call bridged to the live endpoint, the agent's reply asserted as
   non-silence with the negotiated facts named, evidence recorded in the story's Progress
   the way A-15 records publication evidence, and an adversarial self-test of the harness
   in the gate so a checker that observed nothing cannot report green.

## Alternatives considered

- **The vendor's SIP connector instead of the WebSocket API.** Rejected as this epic's shape:
  it would make the *vendor* the SIP peer and move the interesting seam into an inbound
  HTTPS webhook receiver plus a REST accept flow — a whole HTTP server surface the workspace
  deliberately does not have — while exercising no application-side audio path at all. The
  WebSocket bridge keeps sipx as the SIP endpoint and proves the app host can hold a live
  audio seam, which is the capability this epic exists to demonstrate.
- **Transcoding to 24 kHz PCM for the realtime session.** Not chosen: the API accepts the
  call's own G.711 encodings, so passthrough via relay mode is exact, allocation-light, and
  keeps the bridge out of the resampling business. PCM support was a non-goal for this bridge;
  `M-43` later supplied the general application boundary without changing that choice.
- **An HTTP client/framework addition for session setup.** Not needed: the WebSocket
  handshake carries the authentication header; no REST call is on the bridge's path.
- **Automatic reconnection of a dropped realtime session.** Rejected: a reconnect invents a
  conversation state the far end no longer has. A dropped socket ends the bridge with a
  typed outcome and the application decides; anything cleverer is a later story with its own
  spec section.

## Risks & open questions

- **Vendor contract drift.** Event names and session fields are the vendor's to change. The
  spec records its observation date; the live proof failing on drift is a spec update plus a
  story, not a silent fix. The stand-in peer implements the spec, so the default matrix
  cannot drift silently — it can only disagree with the live endpoint, which is exactly what
  A-23 exists to detect.
- **Barge-in latency.** The agent's queued-but-unsent audio is dropped locally, so the bound
  is our own queue depth plus one packet in flight — but the *perceived* cut-off also
  depends on how much audio the far end has already delivered ahead of real time. The spec
  must state the queue-depth bound explicitly so the test asserts a number, not a feeling.
- **Credential scope for the live proof.** An API key is billable; the harness must place
  exactly one bounded call, and the self-test must prove the bound holds even when the peer
  misbehaves.

## Acceptance / done

The union of A-19 through A-23: a normative bridge spec with vectors; a WSS client and a
bridge each holding to those vectors in the default matrix; a stand-in peer that makes the
whole loop pass deterministically in CI with no credentials; one command that demonstrates a
call answered by an agent; and recorded evidence of one live bridged call, every asserted
fact named. Gate green throughout, including the harness self-tests.
