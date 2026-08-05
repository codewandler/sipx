# Spec: Transport layer

**Status:** normative · **Crate:** `sipx-transport` · **Stories:** T-1 … T-4, T-25 … T-29, T-32, T-34, T-35 · **Design:**
[sip-transport](../designs/sip-transport.md)

## 1. Normative references

- RFC 3261 §9.1 (constructing and sending CANCEL), §17.1 (client transactions), §18
  (transport), §18.1 (clients), §18.2 (servers), §18.2.1 (`received`), §18.2.2 (sending
  responses), §19.1.1 (`maddr`), §20.42 (`Via`).
- RFC 3581 — `rport`.
- RFC 3263 — locating SIP servers.
- RFC 2782 — SRV weighting.
- RFC 5923 — connection reuse.
- RFC 7118 — SIP over WebSocket.
- RFC 7339 §4–§5, §7.2 — hop-by-hop SIP overload control and the loss algorithm.
- RFC 7415 §3.3–§3.5.1 — rate-based overload control.

**Out of scope:** TLS certificate policy (a later story), and everything the sans-IO core
already specifies.

## 2. The driver contract

`sipx-sip` is a state machine that cannot act. This crate is what acts for it. The whole of
the layer is one loop:

```
loop {
    select! {
        bytes = socket.recv()      => feed the core, perform its outputs
        deadline = timers.next()   => feed the core a fired timer, perform its outputs
        command = app.recv()       => feed the core a request or response, perform its outputs
    }
}
```

**[sipx] Outputs are performed in the order they were produced.** The core emits `Send`
before the `SetTimer` that will retransmit it; reordering them opens a window where a
retransmission fires for something not yet sent.

**[sipx] The loop owns everything mutable.** No transaction is reachable from two tasks, so
there are no locks in the signalling path and no way to observe a half-applied transition.
The application talks to the loop over channels.

**[sipx] UDP receipt is separated from transaction work by the configured bounded event queue.**
The socket reader owns no transaction state: it copies datagrams in kernel arrival order into one
`Config::capacity` channel, while the driver remains the only task that parses them and mutates the
transaction layer. On each readiness it first copies at most `UDP_RECEIVE_BATCH` ready datagrams,
then enqueues those datagrams individually; batching reduces scheduler handoffs but cannot enlarge
the channel's event bound. This keeps a small kernel receive buffer from becoming the burst
boundary. A full userspace queue backpressures the reader and is therefore still a finite overload
boundary; shutdown cancels and joins the reader before the driver reports completion. A configured
capacity above the runtime channel limit is a typed pre-bind refusal, never a channel-construction
panic.

## 3. Timers

**[sipx]** A single earliest-deadline-first queue for the whole endpoint, keyed by
`(TransactionKey, Timer)`, not one task per timer. A busy proxy has tens of thousands of live
timers and spawning a task for each is how a stack acquires a scheduling problem it cannot
profile.

**[sipx]** `ClearTimer` marks the entry dead rather than removing it from the middle of the
queue; the entry is discarded when it surfaces. Cancellation is common and cheap this way.

**[sipx]** Transaction termination forgets the fixed RFC timer vocabulary for that transaction
by exact key. It MUST NOT scan timer-generation entries belonging to every other live transaction;
termination cost is bounded by `Timer::ALL`, not by endpoint concurrency.

**[sipx]** A timer that fires for a transaction that no longer exists is dropped silently. It
is a race the design permits, not an error.

## 4. Per-transport behaviour

| | UDP | TCP / TLS | WS / WSS |
|---|---|---|---|
| Framing | one message per datagram | `Content-Length`, incremental | one message per WebSocket frame |
| Retransmission | yes (Timers A, E, G) | no | no |
| Reliability | `Unreliable` | `Reliable` | `Reliable` |
| Connection | none | pooled, reused | pooled, reused |
| Max message | 64 KiB | 1 MiB | 1 MiB |

**[RFC 3261 §18.1.1]** A request within 200 bytes of a known path MTU, or larger than 1300
bytes when the path MTU is unknown, is sent over a congestion-controlled transport. `Config` holds
the optional known path MTU, never a separately configurable copy of the derived limit. The one
derivation is therefore `path_mtu - 200` when known and 1300 otherwise; subtraction saturates so an
unusable claimed path cannot permit a datagram.

**[sipx]** When that limit is exceeded for an outbound UDP request, sipx changes the target to TCP
at the same address before creating the transaction. A transport-owned top `Via` is consequently
built as TCP and the transaction uses reliable timers. Application-supplied `Via` identity is not
reconstructed, but the request still uses TCP: replies return on the connection that carried it.
The switch is counted in `Counters::oversized_request_tcp_fallbacks` and logged with the peer, size,
and derived limit. If opening or using TCP fails, the transaction receives
`TcpFallbackUnavailable`, which retains the size, limit, and concrete connection failure. The
request is never retried as a datagram, truncated, or fragmented by policy.

The rule is for requests. An oversized UDP response still follows §18.2.2 over the transport
selected by the request's topmost `Via`; changing that transport would strand the transaction. The
send-time UDP size refusal remains as a defensive invariant for a request that reaches the socket
without passing through outbound request selection, and reports `TooLarge` rather than emitting an
oversized datagram.

## 5. Receiving

**[RFC §18.2.1]** When a request arrives from a source address that differs from the top
`Via` sent-by, the server adds `received=<source IP>`. Always — not only when it differs, if
the sent-by is a hostname rather than an address.

**[RFC 3581]** If the top `Via` carries `rport` with no value, the server adds
`rport=<source port>` and sends the response to the source address and port rather than to the
sent-by. This is what makes SIP work through a NAT at all, so it is on by default.

**[sipx]** A datagram that fails to parse is logged and dropped. One malformed packet must not
disturb the socket; the alternative is a trivial denial of service.

**[sipx]** A stream that fails to parse closes the connection, because framing is lost and
resynchronizing means guessing where the next message starts.

## 6. Sending responses (RFC 3261 §18.2.2)

In order:

1. If the top `Via` has `maddr`, send there.
2. Else if it has `received`, send to that address, at the port from `rport` if present, else
   the sent-by port, else the default for the transport.
3. Else send to the sent-by.

**[sipx]** On a connection-oriented transport, the response goes back over the connection the
request arrived on when it is still open, before any of the above is consulted. Opening a new
connection to a NATed client's `Via` cannot work, and RFC 5923 exists to say so.

## 7. Sending requests

**[RFC 3261 §18.1.1]** The `Via` sent-by names where this element wants responses. sipx fills
it from configuration, not from the socket's local address: behind a NAT those differ, and the
socket's view is the wrong one.

**[sipx]** `Via` `branch` is `z9hG4bK` followed by 64 bits from a cryptographic RNG. The magic
cookie is required (RFC 3261 §8.1.1.7); the width is ours, because a guessable branch lets an
off-path attacker inject responses into a transaction.

### 7.1 Cancelling one outgoing INVITE

**[RFC 3261 §9.1] A CANCEL is derived from one existing INVITE client transaction, never from
caller-reconstructed identity.** `Handle::cancel_invite` therefore accepts the mutable `Responses`
returned by the exact `Handle::send` call. `Responses` retains the request after transport policy
and `Via` construction, its selected `Target`, and its `TransactionKey`; no public operation accepts
a branch string or replacement key.

The operation returns `CancelInviteOutcome`:

- `Sent(InviteCancellation)` identifies both the INVITE and the newly created CANCEL transaction.
  `InviteCancellation::outcome` reports the CANCEL transaction's final response, timeout, or
  transport failure as `CancelTransactionOutcome`.
- `FinalResponse` carries the final INVITE response that won the race before a CANCEL transaction
  existed. No CANCEL is sent.
- `InviteTimeout` and `InviteTransportError` name how the INVITE ended before the §9.1 precondition
  was met. A concrete driver error accompanies the transport outcome when one was recorded.

`Reason` is the one optional policy input. When supplied, it is appended to the derived request;
it cannot alter transaction identity. The operation rejects a non-INVITE response stream, an
INVITE missing a required identity header, and a second cancellation request with typed transport
errors before creating a CANCEL transaction.

| INVITE observation when cancellation is requested | Action | Result |
|---|---|---|
| No response observed | Wait on that INVITE's response stream; send nothing | pending |
| One or more provisional responses observed | Derive and create exactly one CANCEL transaction | `Sent` |
| Final response observed before CANCEL creation | Send nothing | `FinalResponse` |
| INVITE timeout before CANCEL creation | Send nothing | `InviteTimeout` |
| INVITE transport failure before CANCEL creation | Send nothing | `InviteTransportError` |
| A CANCEL transaction was already created | Send nothing | `Error::InvalidCancellation` |

Events read while enforcing the provisional-response precondition remain buffered on the original
`Responses`. Cancellation cannot make a provisional or crossing final response disappear from the
application's ordinary INVITE stream.

The derived CANCEL has the INVITE's Request-URI; top `Via` value including branch; every `Route` in
wire order; first `To`, `From`, and `Call-ID`; and CSeq number. Its method and CSeq method are
`CANCEL`, its `Max-Forwards` is 70, and its body is empty. Vector C1 fixes the byte-level result:

```text
INVITE sip:bob@203.0.113.9 SIP/2.0
Via: SIP/2.0/UDP 192.0.2.10:5060;rport;branch=z9hG4bKcancel-vector
Route: <sip:edge.example;lr>
To: <sip:bob@example.com>
From: <sip:alice@example.net>;tag=from-1
Call-ID: cancel-1@example.net
CSeq: 42 INVITE
Max-Forwards: 69
Content-Length: 0


=>

CANCEL sip:bob@203.0.113.9 SIP/2.0
Via: SIP/2.0/UDP 192.0.2.10:5060;rport;branch=z9hG4bKcancel-vector
Route: <sip:edge.example;lr>
To: <sip:bob@example.com>
From: <sip:alice@example.net>;tag=from-1
Call-ID: cancel-1@example.net
CSeq: 42 CANCEL
Max-Forwards: 70
Content-Length: 0


```

## 8. Connection reuse (RFC 5923)

**[sipx] An inbound connection is reused for responses on that transaction, always.**
**[sipx] An inbound connection is *not* reused for unrelated outbound requests by default.**

Reusing it is convenient and is also how a peer that connected to you gets your outbound
traffic routed through it. The default is off; a configuration flag turns it on for
deployments where both ends are trusted. This is the security-relevant default `T-1` was asked
to decide, and this is the decision.

**[sipx] Connections are pooled by `ConnectionKey`, and this is the only place its fields are
written down.** The table below is generated from the type by `scripts/check-pool-key.py`, and
the gate fails when the two disagree. A hand-written list stood here instead and was wrong
twice: once when the verified identity joined the key, and again when the WebSocket resource did
in `T-23`. Both times a reader was told the key was two fields when it was three, then four.

Why each field is in the key — which is an argument, not a list, and so is not generated — is in
[`sip-tls.md` §5](sip-tls.md), where the two non-obvious ones come from.

<!-- BEGIN generated:pool-key crates/sipx-transport/src/target.rs -->
| Field | What it is |
|---|---|
| `peer` | The far end. |
| `transport` | Which transport it speaks. |
| `identity` | Original URI host: verified for TLS/WSS and used as authority for WS/WSS. |
| `path` | The resource the upgrade asked for, for WebSocket connections sipx opened. |
<!-- END generated:pool-key -->

**[sipx]** The pool is bounded (default 1024), and connections are closed after an idle timeout
(default 5 minutes). Eviction is least-recently-used, and a connection with a live transaction
is never evicted.

The bound is on live connection tasks and sockets, not only entries in the routing map. Every
pooled connection has an endpoint-owned cancellation signal and occupies one pool slot until its
task has terminated. Idle or least-recently-used eviction signals cancellation, which interrupts a
task blocked on a read, closes the socket, and reports the connection closed. A replacement is not
started while the cancelled task still occupies the last live-task slot. Instead it reserves its
own generation and bounded writer queue, then starts when the retiring task reports completion. One
replacement therefore retires only the generation with the same key; it never consumes a second
victim or evicts an unrelated connection merely because cancellation has not completed yet.

Every message, pong, transaction destination and close event on a stream carries the generation
that produced it. A deliberately retired generation remains eligible for exactly one close event,
which fails only its own transactions and keep-alive waiters. Once a replacement is current, queued
events from the old generation cannot remove it or answer its waiters. Endpoint shutdown follows the
same cancellation path and waits for its tracked connection tasks, so dropping routing metadata
never detaches the resource it described. A final close report blocked by a full event channel is
cancellable by shutdown; completion cannot depend on a driver which has stopped draining that
channel.

**[sipx]** When a connection closes, every transaction bound to it is given a transport error
rather than being left to time out. Waiting 32 seconds to discover something we already know
is a bad experience and a resource leak.

## 9. Resolution (RFC 3263)

Order: if the URI names an IP address or a port, that is the answer and nothing is looked up.
Otherwise NAPTR → SRV → A/AAAA, with RFC 2782 weighted selection among equal priorities.

**[RFC]** A `sips:` URI restricts candidates to TLS-capable transports.
**[sipx]** The pure `Uri::selected_transport` rule is the single mapping used both here and by
dialog route-set consumers. It maps the URI scheme and transport parameter before resolution,
supplies the transport's default port, rejects unknown transports and rejects `sips` over UDP.
The pre-resolution URI host remains on every outbound WebSocket target: WSS verifies it and both WS
and WSS use it as the HTTP `Host` authority. Connection pooling keys include that authority and the
WebSocket resource, so virtual hosts and paths at one address never share an upgrade.
**[sipx]** Resolution is behind a trait so tests use a fixture and never touch DNS. The
weighted selection takes its randomness from an injectable source, so the distribution is
testable with a fixed seed.
**[sipx]** Candidates are tried in order; a transport failure moves to the next before the
request fails.

## 10. Backpressure

**[sipx]** The channel from the loop to the application is bounded (default 1024). When it
fills, **new server transactions are rejected with 503 Service Unavailable and a `Retry-After`**
rather than the loop blocking or events being dropped.

Blocking the loop would stop timers, which turns a slow application into a stack that stops
retransmitting and drops calls it had already established. Dropping events silently loses
requests. Answering 503 is what the status code is for, and it tells the peer something true.

## 10.1 Configuration and pre-pool admission

**[sipx] Endpoint configuration is validated before any socket is bound or background task is
started.** The application event/command capacity, pool connection limit, inbound handshake limit,
overload peer-state limit, inbound handshake timeout, overload-report validity and WebSocket
keepalive interval must all be non-zero. The minimum valid value for a count is one; the minimum
valid duration is any duration greater than zero except overload validity, whose millisecond wire
field requires at least one millisecond. Invalid values return a typed configuration error naming
the field. Values are never silently clamped. RFC 7415's protected-request threshold `TAU2` must be
greater than the ordinary threshold `TAU1`; equal thresholds erase the policy hook and are rejected
at bind.

`Config::cleartext` selects listener kinds exactly; selecting TCP never implies UDP and selecting
UDP never implies TCP. The default preserves the historical combined listener.

| `CleartextTransports` | UDP socket | TCP listener | `Handle::local_addr()` |
|---|---:|---:|---|
| `None` | no | no | first configured non-cleartext signalling listener |
| `Udp` | yes | no | UDP bound address |
| `Tcp` | no | yes | TCP bound address |
| `UdpAndTcp` (default) | yes | yes | their shared bound address |

`None` without a configured TLS, WebSocket, secure-WebSocket, or QUIC server listener is a typed
`InvalidConfig` error naming `cleartext`, before any bind. Client credentials or a client-only QUIC
endpoint do not count as a listener. When no cleartext listener exists, the primary local address is
selected in the stable order TLS, WebSocket, secure WebSocket, then QUIC. An absent or zero
`sent_by_port` uses that primary listener's actual port.

| Cleartext selection | Configured port | Bind state |
|---|---:|---|
| `Udp` | exact or `0` | bind UDP once; report its actual address |
| `Tcp` | exact or `0` | bind TCP once; report its actual address; create no UDP socket |
| `UdpAndTcp` | exact | bind UDP, then TCP on the same address; any conflict is returned |
| `UdpAndTcp` | `0` | bind UDP, then TCP at UDP's chosen port; on `AddrInUse`, drop UDP and retry, at most 16 attempts |
| `None` | any | perform no cleartext bind |

The driver stores the UDP socket as optional state. Its receive branch is disabled when the socket
is absent, and an outbound UDP target on such an endpoint returns
`Error::TransportNotConfigured` rather than using a placeholder datagram socket. TCP pooling and
accepted streams are independent of UDP state.

The default inbound handshake budget is 64 live handshakes per endpoint and the default deadline is
10 seconds. The budget is shared across TLS, WebSocket and secure WebSocket listeners. An accepted
socket must acquire a permit without waiting before a handshake task is spawned. If no permit is
available, the new socket is closed immediately; there is no pre-handshake wait queue. TLS followed
by a WebSocket upgrade is one secure-WebSocket handshake and has one deadline for both phases.

Timeout, protocol failure, successful adoption and endpoint shutdown each close or transfer the
socket and release exactly one permit. Listener loops and handshake tasks are endpoint-owned:
shutdown cancels them and waits for their completion.

`Handle::shutdown` waits on a durable endpoint completion barrier. This includes a caller whose
shutdown command loses a race with command-receiver closure: command closure means cleanup has
started, not that it has finished. The barrier becomes complete only after listeners, handshake
tasks, pooled connections and endpoint sockets have been released.

## 10.2 Graceful drain

`Handle::begin_drain` is the transport half of a graceful endpoint drain. It closes admission for
outbound requests which can establish a dialog: an `INVITE`, `SUBSCRIBE`, or `REFER` without a
`To` tag returns `Error::EndpointDraining` before a command or transaction is created. Responses,
ACKs and requests carrying a `To` tag remain legal, so a live dialog can finish. The call dispatcher
owns the inbound half because only it knows whether an initial request belongs to a live route; see
[`call-dispatch.md` §10](call-dispatch.md).

The state transition is monotonic and idempotent:

| State | New dialog request | Existing transaction | `shutdown` |
|---|---|---|---|
| `Running` | admitted | driven normally | enter `Stopping` |
| `Draining` | typed refusal before transaction creation | driven normally | enter `Stopping` |
| `Stopping` / `Stopped` | endpoint closed | cancelled and released | wait on the durable completion barrier |

`Handle::settled` is an event-driven transaction barrier. A waiter is registered on the endpoint
driver and released only after the transaction layer reports zero client and server transactions.
It is not a polling interval and does not infer completion from elapsed time. A racing transaction
therefore either precedes the serialized waiter and is included, or follows the zero observation
and loses the race with final shutdown.

The call layer's bounded drain ends by calling the existing `Handle::shutdown` path. It does not own
listeners, connection tasks, handshake tasks or sockets separately: the endpoint's existing
`CancellationToken` closes their admission, its `TaskTracker` joins them, and the durable shutdown
barrier reports their release. Deadline expiry explicitly counts live dialog routes and endpoint
transactions before taking this same path.

Transport behavior at that boundary is fixed:

| Transport | During drain | At natural completion or deadline |
|---|---|---|
| UDP | the socket remains readable so existing transactions and in-dialog requests progress | socket closes through endpoint shutdown |
| TCP / TLS | pooled generations remain reusable by existing transactions and dialogs; no new dialog request may create a transaction | every pooled generation is cancelled, joined and closed |
| WS / WSS | the same pool rule applies; control frames and in-dialog messages continue | WebSocket tasks are cancelled and joined before completion |
| QUIC | an already admitted stream continues through its transaction; a new dialog stream is refused before creation | endpoint close terminates every remaining connection and mid-stream transaction, which is included in the forced count |

The deadline is a bound on failure, not evidence that work finished. Natural completion is always
the route-closure and transaction-terminal events described above.

## 10.3 Hop-by-hop overload control (RFC 7339 and RFC 7415)

**[RFC 7339 §2, §4]** Overload control is an explicit endpoint capability and is disabled by
default. Setting `Config::overload.advertise` makes the endpoint a supporting client for this
extension; every client-generated topmost `Via` then carries valueless `oc` and
`oc-algo="loss,rate"`. A request never carries `oc-validity` or `oc-seq`. A server that received
the offer fills `oc`, selects exactly one offered algorithm in a quoted `oc-algo` value, and adds
`oc-validity` and an increasing `oc-seq` to the topmost `Via` of every response. A normal response
reports `oc=0;oc-validity=0`: the server supports the selected algorithm and is not asking for
control. The four parameters are exposed as typed `sipx-sip` values; malformed numeric values are
not silently interpreted as zero.

**[RFC 7339 §5.4–§5.7]** Client control state is keyed by the next hop's IP address and port.
A report with a sequence no greater than the last report for that peer is stale and changes
nothing. A report without `oc-validity` lasts 500 milliseconds. Expiry turns control off. A newer
report with `oc-validity=0` also turns it off immediately and its `oc` value is ignored. Sequence
history survives both forms of deactivation: otherwise a delayed response can reactivate an older
reduction after the server has explicitly stopped it.

Only a response matched to a live client transaction may update this state. An unmatched response
has not proved that the endpoint sent the request whose `Via` it carries; accepting its feedback
would let a forged datagram throttle an unrelated peer.

The peer-state map has a configured non-zero bound (default 1024). A new peer first evicts the
least-recently-used expired or control-off entry; if every entry is active, it evicts the
least-recently-used entry. Reads refresh recency. This is an explicit resource limit: a deployment
that simultaneously controls more next hops than the configured bound must raise it.

**[RFC 7339 §7.2]** Loss control keeps the two message categories named by the RFC. The endpoint's
policy hook assigns each request to ordinary traffic, which is reduced first, or protected traffic,
which is reduced only after ordinary traffic is exhausted. The observed category mix converts the
server's overall percentage into a per-category discard probability. Randomness is injected into
the controller; production uses the operating-system generator and tests use a fixed seed.

**[RFC 7415 §3.5.1–§3.5.2]** Under `rate`, `oc` is requests per second. Admission uses the RFC's
leaky bucket with time supplied by the driver, never read by the algorithm. Its two exposed burst
tolerances are `TAU1` for ordinary requests and the larger `TAU2` for protected requests; defaults
are five and ten target inter-request intervals, the RFC's stated two-priority values. Equal values
are available to tests and internal no-priority use, but endpoint configuration requires
`TAU1 < TAU2`. A non-zero validity with `oc=0` rejects every request; validity zero disables control
instead.

An overload refusal is local and typed: no transaction or network write is created, and the
endpoint increments `Counters::overload_rejections`. This includes the direct ACK path; overload
control applies to all downstream requests, while the policy hook is how an application protects
an in-dialog or emergency request. Requests that are admitted continue to advertise both algorithms
regardless of the algorithm currently selected by the server. A default endpoint neither adds the
capability parameters nor installs received client-control state; its server half still answers a
peer that explicitly offered the extension, because the peer's offer—not this endpoint's client
policy—is what scopes that response.

**[sipx] Server feedback is tied to the existing backpressure path.** When the application queue is
full, the 503 and `Retry-After` remain, the existing shed counter still increments, and a client that
offered overload control also receives the configured loss percentage or request rate in the
response's topmost `Via`, with validity and sequence. The default feedback is 100% loss for 500
milliseconds: it describes the endpoint's observed state rather than inventing a load estimator.
Each shed event refreshes that detector interval. Until it expires, every response—including an
application response and transaction-generated 100 Trying—reports the active value with the
remaining validity; an ordinary response cannot undo feedback while the same bounded queue remains
saturated. After expiry, every response reports explicit control-off state.

## 11. Test vectors

| # | Scenario | Expected |
|---|---|---|
| X1 | Loopback `OPTIONS` between two endpoints | 200, and the transaction terminates on both sides |
| X2 | Request from a source differing from its `Via` sent-by | Response carries `received` |
| X3 | Request with an empty `rport` | Response carries `rport=<source port>`, and goes to the source port |
| X4 | Malformed datagram, then a good one | The good one is processed; the socket survives |
| X5 | Message split across TCP segments | Assembled correctly |
| X6 | Two messages in one TCP segment | Both delivered, in order |
| X7 | TCP connection closes mid-transaction | The transaction gets a transport error at once |
| X8 | Loopback with 50 % loss | The request is retransmitted and the transaction still completes |
| X9 | SRV records with weights 10 and 90, fixed seed | Selection matches RFC 2782's distribution |
| X10 | Application channel full | New requests are answered 503, and timers keep firing |
| X11 | A shed request and an unmatched response | Both appear in the counter snapshot (§12) |
| X12 | Loopback `OPTIONS`, capture on | Two records — request out, response in — with the real ports |
| X13 | Malformed datagram, capture on | Captured malformed, and counted a parse failure and not a request (§12.2) |
| X14 | `Authorization` with a digest `response`, capture on | `realm` and `nonce` survive; the `response` value does not (§13.3) |
| X15 | SDP `a=crypto` in a captured body | Tag and suite survive; the key after `inline:` does not (§13.3) |
| X16 | Capture off | No file is opened, and the snapshot's capture counters stay zero |
| X17 | Quiet TCP and WebSocket peers are idle- or capacity-evicted | Peer observes EOF and the live task releases its pool slot |
| X18 | More partial TLS/WS/WSS handshakes than the configured budget | No more than the budget run; excess sockets close without queuing; deadlines reclaim every permit |
| X19 | Zero event capacity, pool limit, handshake limit/deadline or WebSocket keepalive | Typed configuration error before any configured address is bound |
| X20 | Replace connection A at a full pool containing A and B | A replacement is reserved under a new generation; B remains live; the live-task bound is never exceeded |
| X21 | Idle/LRU cancellation followed by that generation's close | Its transactions and pong waiters fail exactly once; a replacement generation is untouched |
| X22 | Final close report blocks behind a full event channel during shutdown | Shutdown cancellation releases the reporting task and completion does not hang |
| X23 | A shutdown caller arrives after command-receiver closure | It still waits until the durable cleanup barrier completes |
| X24 | An old generation's pong is queued ahead of a replacement pong | The old pong answers no replacement waiter; the matching generation's pong does |
| X25 | Client-generated `Via`, then a server overload response | Request has valueless `oc` plus `loss,rate` and no server-only fields; response has typed value, selected algorithm, validity and sequence |
| X26 | Newer report followed by an older report, then validity zero | The old report is ignored; zero disables control; neither can be undone by a delayed response |
| X27 | 50% loss feedback over a fixed-seed ordinary request population | The deterministic distribution is reduced by half; protected requests survive while ordinary capacity remains |
| X28 | Rate feedback with a fake clock and zero burst tolerance | One request is admitted per target interval and intervening requests are rejected |
| X29 | Full application queue after an overload-capable request | Existing 503 and shed count remain, and the response reports overload in its topmost `Via` |
| X30 | Learned 50% loss control driven by a 128-attempt, eight-concurrent bounded load plan | Admission ends at the call bound, both forwarding and local rejection occur, the rejection counter agrees, and every owned task finishes within cleanup |
| X31 | Ordinary 200 response to a request offering overload control | Topmost `Via` selects one quoted algorithm and reports `oc=0`, validity zero and a sequence |
| X32 | Rate bucket above `TAU1` but no higher than `TAU2` | Ordinary request is rejected and protected request is admitted |
| X33 | Unmatched response carrying valid-looking overload feedback | It reaches the unmatched path but changes no client control state |
| X34 | More reporting peers than the configured overload-state bound | The map never exceeds the bound and evicts expired/control-off least-recently-used state first |
| X35 | Queue saturation followed by an application response for an earlier request | The response still reports active feedback; a generated 100 Trying also carries a selected report |
| X36 | Observation capacity one, followed by several parsed messages | The driver never waits; one event is retained and every overflow increments `observation_dropped` |
| X37 | Observation receiver is closed while traffic continues | Traffic and timers continue; closure creates no driver failure |
| X38 | Request policy returns a protected field directly, as mixed-case/compact `Other`, or appends a duplicate allowed standard field | Send is refused before transaction creation; only the application-field allowlist or a truly unknown extension may be appended |
| X39 | Refused UDP source sends malformed bytes | `source_refusals` increments and `parse_failures` does not: admission ran before parsing |
| X40 | Refused source reaches TCP, TLS, WebSocket, secure WebSocket or QUIC | The connection closes before framing or handshake work and that transport's `source_refusals` increments |
| X41 | Source set changes from A to B while A has a pooled stream | New A connections are refused, B is admitted, and the existing A generation remains usable until close |
| X42 | Replacement exceeds `source_admission_limit` | Typed capacity refusal; the previous complete generation and its number remain active |
| X43 | TCP-only bind to port `0` | TCP connects at `local_addr`; UDP can bind that exact address; implicit sent-by uses the TCP port |
| X44 | UDP-only bind | UDP receives at `local_addr`; TCP can bind that exact address |
| X45 | Combined UDP+TCP bind | both listener kinds occupy the same address and port |
| X46 | No cleartext and no other server listener | typed pre-bind `InvalidConfig` naming `cleartext` |
| X47 | INVITE with a large SDP body exceeds the unknown-path 1300-byte cutoff on a UDP target | It arrives at the same peer over TCP with a TCP top `Via`; UDP remains silent and `oversized_request_tcp_fallbacks` increments once |
| X48 | The X47 peer has no TCP listener | Typed `TcpFallbackUnavailable` retains the message size, derived limit and connection cause; no UDP datagram is sent |
| X49 | Known path MTUs of 1500 and 1200 bytes | The one derived request limits are 1300 and 1000 bytes respectively |

### 11.1 Live endpoint policy and observation

**[sipx] Source admission precedes protocol work.** The active policy is either allow-all or one
immutable set of exact IP addresses and CIDR prefixes. UDP reads the current generation before STUN
classification or SIP parsing. TCP, TLS, WebSocket, secure WebSocket and QUIC read it immediately
after the socket-level accept and before stream parsing, TLS authentication or HTTP upgrade. A
refusal creates no task, future, per-source map entry or application event; it closes or drops, bumps
the transport's `source_refusals`, and logs at debug level.

Replacing or clearing the policy publishes one complete generation under one synchronization point.
The configured `source_admission_limit` is non-zero and bounds both retained prefixes and the linear
work of every admission decision. A replacement above it returns a typed capacity error before the
publication point, leaving the old generation unchanged. The default maximum is 1024 prefixes.
UDP has no connection and therefore reads the latest generation per datagram. A connection keeps the
generation that admitted it; replacement governs later accepts and is deliberately not a revocation
mechanism. That makes rotation atomic without letting policy work race every frame on an established
call.

**[sipx] Observation is data, never a callback.** One optional bounded receiver carries cloned,
read-only endpoint events. Message events contain the parsed or finalized `Message`, local and peer
addresses, transport, direction and transaction classification. Connection events contain a typed
connection identity and accepted/opened, authenticated, pooled/reused, failed or closed state.
Every producer uses `try_send`; a full receiver increments `observation_dropped`, and a closed
receiver is detached. Capture and counter snapshots remain the no-custom-consumer path.

**[sipx] Request policy is structurally narrow.** It receives an immutable request and target in the
caller's task and returns allow, reject, or a list of headers to append. It cannot receive a mutable
request. The standard-header allowlist is `Alert-Info`, `Call-Info`, `Organization`, `Priority`,
`Subject` and `User-Agent`; an allowed standard field already present is refused rather than appended
as a duplicate. A genuinely unknown extension field is also allowed. Before classification,
`HeaderName::Other` is resolved case-insensitively and through SIP compact forms, so `Other("vIa")`
and `Other("v")` are both protected `Via`. Every other standard field—including `Contact`, body
metadata such as `Content-Type`, and dialog/event semantics such as `Event`—is refused before a
command reaches the endpoint. The transport then adds its branch and `Via`, creates the transaction
key, and serializes framing. The policy cannot replace target resolution and no policy runs after
transaction creation.

## 12. Counters

**[sipx]** The endpoint keeps counters, and nothing else: no metrics library, no exposition
format, no push. A snapshot — a plain struct — is read through the handle
(`Handle::counters`, next to `Handle::outstanding`), and what an application does with it is
the application's business. A stack that picks an exposition format picks it for every user of
the library, and that is the one observability decision that cannot be undone later.

The counters live in atomics shared between the driver and every handle, exactly as
`ShedCounts` already does (§10): the loop is busy in precisely the situation the counters
describe, so a counter that could only be read by asking the loop would be unreadable when it
mattered.

The two neighbours on `Handle` show the choice being made. `Handle::shed` is synchronous and
reads shared atomics; `Handle::outstanding` is `async` and returns `Result`, because it asks
the loop and the loop may be gone. `Handle::counters` is deliberately the first shape and not
the second: a snapshot that returned `Err(EndpointClosed)` under load — or blocked behind the
work it is trying to describe — would fail exactly when an operator reached for it.

The snapshot covers, at minimum:

- requests and responses, in and out, **per transport** — which transport is the first
  question a support case asks;
- requests shed for backpressure (§10), embedded as the existing `ShedCounts`;
- outbound requests rejected under overload control (§10.3);
- responses that matched no client transaction (RFC 3261 §16.7), counted whether or not an
  application is watching for them;
- parse failures, per transport — a malformed datagram and a stream whose framing is lost are
  the same failure on different transports, and both are counted. A connection task has no counters
  in scope, so it reports the loss to the driver (`Event::FramingFailed`) and the driver counts it,
  which keeps every counter in the crate at one increment site;
- retransmissions sent — a rising count with no matching traffic growth is a peer that is not
  hearing us, and the difference between a network problem and an application one;
- oversized-request TCP fallbacks selected before transaction creation, including selections whose
  connection attempt then fails — the counter answers whether the endpoint changed transport,
  while `unsent` and `send_failures` answer whether that selected send reached the wire;
- transactions timed out, per the timer that fired (B, F or H);
- every place the stack discards something it was given: see §12.1.

### 12.1 No silent discards

**[sipx]** Every discard in the signalling path has a counter. A test enumerates the discard
sites — every `tracing` line that reports dropping or ignoring, and every `let _ = …` that
discards a result — and fails when one appears without a counter or a written reason. A silent
drop is the failure this section exists to end; the enumeration is what keeps it ended as the
code changes.

A discard whose reason is logged but not counted is still a failure here: logs rotate, and an
operator asking "how often" deserves an answer that is not `grep | wc -l`.

**The enumeration's limit, stated because it is real.** A discarded *result* is found structurally —
`let _ = …` is unambiguous — but a log line that reports a loss can only be recognised by the words it
uses, and there is no closed vocabulary for that. The check holds a list of words, and the first
version of that list held three and missed two live sites: a TCP connection closed on a framing error
and a WebSocket closed on a malformed message, each of which discards everything in flight on that
connection. Adding a word costs a false positive and one comment; leaving one out is a silent hole, so
the list errs long. It is not a proof that no silent discard exists — it is a ratchet that stops the
ones it can name from coming back.

### 12.2 What the numbers do not promise

**[sipx]** A counter that overstates its own accuracy is worse than a missing one, because it
will be used to rule a cause out. Three limits, stated here because they belong where the
counters are defined and not in a release note:

1. **A snapshot is not an instant.** The fields are separate atomics read one after another, so
   a snapshot taken while traffic flows can show `requests_in` from a later moment than
   `responses_in`. Each field is individually monotonic and none is ever lost; the *relationship
   between two fields* is only exact when the endpoint is quiet. Differences between successive
   snapshots are sound; arithmetic identities across fields of one snapshot are not.
2. **In and out do not balance, by construction.** A datagram that fails to parse is counted as
   a parse failure and **not** as a request or a response, because which one it would have been
   is exactly what could not be determined. `requests_in + responses_in + parse_failures` is the
   number of messages that arrived; `requests_in + responses_in` alone silently omits the
   malformed ones.
3. **Retransmissions are counted where the timer fires**, so a retransmission the socket then
   refuses is still counted as sent. Counting it after the socket call would mean a peer that
   stopped hearing us produced a *falling* count, which inverts the signal the counter exists
   to give.

Every counter is incremented at exactly one site with `Relaxed` ordering, which is what makes
the first limit the only ordering hazard: there is no path on which one event increments a
counter twice, and none on which an increment is lost.

### 12.3 The signalling path is two crates, and the numbers are two sets read as one

**[sipx]** §12.1 says *every* discard in the signalling path is counted. This section says which
crates that path is, where each counter lives, and why the storage is two sets while the reading is
one. Added by `X-54`, because until then the rule was enforced over `sipx-transport` alone and the
dialog layer had seven discards that nothing enumerated — found by hand, which is exactly the method
the enumeration exists to replace.

**The path is `sipx-transport` and `sipx-call`.** It does not stop at the socket: an ACK that
reaches no call is lost as thoroughly as one that reached no socket, and it is the loss that leaks
calls. `sipx-sip` and `sipx-sdp` are not in it because they discard nothing — the sans-IO core hands
every failure back as a typed error. `sipx-media` and `sipx-rtp` are the *media* path and are
deliberately outside: the milestone clause this serves says the **signalling** path, and media
counters are their own work. The list is written once, in `CRATES` in
`crates/sipx-transport/tests/discards.rs`, next to the one copy of the detector.

**The storage is two sets, and the boundary is load-bearing.** `sipx-transport` cannot depend on
`sipx-call` — the dependency runs the other way, and reversing it would put the dialog layer beneath
the socket. So a single struct of atomics is not available at any price worth paying: it would have
to live in the lower crate and carry fields for facts that crate cannot observe. A crate that
*defines* a counter it cannot *increment* is where the second increment site eventually appears,
and the one-site property above is what §12.2's promise rests on. Two sets, each incremented at one
site, each checkable on its own.

**The reading is one, because the crate boundary is not the operator's problem.** An operator
holding a capture asks "what did this endpoint lose", not "which crate lost it".
`sipx_call::SignallingCounts` is that one reading: it embeds `Handle::counters` and `Calls::counts`
unaltered rather than re-deriving either, for the reason `Counters::shed` already gives about
itself — two tallies of one event eventually disagree, and then neither can be trusted. It lives in
`sipx-call` because that is the crate that already depends on both.

**A half that was not measured reads as absent, not as zero.** `SignallingCounts::dispatch` is an
`Option`. An endpoint with no dispatcher running has not dispatched nothing; it has not been asked,
and a zero cannot tell those apart. This is §12.2's first paragraph applied to the join: a counter
that overstates its own accuracy is worse than a missing one, because it will be used to rule a
cause out.

**Where a discarded *result* is counted: at the transmit, not at the caller and not at the
hand-off.** A request the endpoint tries to put on the wire and cannot is counted in `UnsentCounts`,
split by method because the consequence is: an unsent CANCEL leaves a phone ringing, an unsent ACK
leaves a 2xx retransmitting toward a closed port, an unsent BYE leaves a dialog up at the far end
that no timer reaps. The caller is the wrong place precisely *because* the caller is the one that
discards the result — six of the seven sites `X-54` closed are `let _ = …` on a path that is already
failing, where the error is genuinely unactionable and the *loss* is not. Counting below the caller
covers those six and every one added later, without a counter having to be remembered at each new
site.

**The hand-off is the wrong place too, and this is the correction that matters.** `X-54` first put
the increment inside `Handle::send` and `Handle::send_directly`, and the type's documentation then
claimed something the code could not do. `Handle::send` returns as soon as the driver has created
the transaction and replied with its key; the transmit happens afterwards, in `perform`. A counter
at that hand-off therefore fires only when the endpoint refuses the request outright — a closed
endpoint, or one with no usable `Via` — and **never** on a refused connection, an unreachable peer
or an over-MTU datagram, which is the entire question it claims to answer. `send_directly` *does*
await its transmit, so `ack` behaved one way while `bye` and `cancel` behaved another, with nothing
in the spec or the type saying so. A counter that is wrong in a direction its own documentation
conceals is worse than an absent one, and this section is the rule `M-32` extends: **count where the
loss happens, not where it is reported.**

Two consequences of counting at the transmit, both stated on `UnsentCounts` as §12.2 requires:

- It **overlaps** `DiscardCounts::send_failures` for requests on the transaction path, and the two
  are views rather than tallies — that field is the transaction path's aggregate over requests and
  responses alike, this is the per-method breakdown over requests on any path. Neither adding nor
  subtracting them means anything.
- A send that loses the race with `shutdown` fails before any transmit is attempted and is **not**
  counted, where the hand-off design counted it and made an ordinary teardown look like lost
  signalling.

**A loss whose only possible actor is one consumer is reported to that consumer.** A call event
dropped because the application fell behind is counted by `CallEvents::dropped`, per call, and is
deliberately not in the joined snapshot. An endpoint-wide total would say that some call somewhere
lost some events, which nobody can act on; the party who can act is the one holding that receiver.

**How this extends.** A new crate joins the path by being added to `CRATES` and by growing a member
on `SignallingCounts` — never by adding fields to another crate's struct, and never by a second
tally of an event already counted. The media half is the next one.

## 13. Capture

**[sipx]** An endpoint can record the signalling it exchanges to a file: every message sent
and every message received, with a timestamp, the transport, and both addresses, bodies
included. Off by default; enabling it is per endpoint (`Config::capture`) and costs an
`Option` check per message when off.

**The file contains the decrypted messages — call content, identities, everything the peers
exchanged except the secrets §13.3 names.** TLS and WebSocket-over-TLS traffic is captured
*before* encryption on send and *after* decryption on receive, because capturing ciphertext
from inside the process would be strictly worse than capturing it from outside. Whoever enables
a capture is responsible for the file.

That responsibility is not discharged by redaction and §13.3 does not pretend otherwise: a
capture is written to be *attached to a bug report*, which is to say handed to someone outside
the trust boundary it was recorded in. Redaction removes the secrets that would still be valid
in someone else's hands. It does not make the file safe to publish.

### 13.1 Format: pcapng

The capture is written as **pcapng** (the format the IETF's pcapng draft specifies and every
current packet-analysis tool reads: Section Header Block, one Interface Description Block,
Enhanced Packet Blocks). Chosen over the classic pcap format for three reasons, in order of
how much they would hurt to retrofit:

1. **Per-packet metadata.** A pcapng packet block carries options; each captured message gets
   a comment naming the transport, the direction, and whether the bytes were decrypted in
   process. Classic pcap has nowhere to put that, and "was this TLS or TCP" is not a question
   a bug report should leave to the port number.
2. **Nanosecond-capable, per-interface timestamp resolution.** Classic pcap fixes the
   resolution for the whole file in a field several tools still misread.
3. **Self-describing structure.** Block types and lengths make a truncated capture readable up
   to the truncation — which is the normal state of a capture taken during a crash.

Packets are written with `LINKTYPE_RAW` (101): a synthesised IPv4 or IPv6 header matching the
address family of the real addresses, then a synthesised **UDP** header carrying the real
ports, then the message. One captured message is one packet. The addresses and ports are the
real ones; the link and transport layers are invented, because there is no link layer inside a
process and no captured byte ever came off one.

**The UDP header is synthetic even when the real transport was TCP, TLS or WebSocket, and the
authoritative statement of the transport is the block comment, not the packet.** This is a
deliberate limit rather than an oversight. Writing a truthful TCP header would mean inventing
per-connection sequence numbers, and that is invented protocol state whose only purpose is to
let a tool reassemble a stream sipx has *already* framed — the message boundaries are known
here, which is why one message is one packet. Inventing the state would add a class of
capture-only bug (a wrong sequence number renders a capture unreadable in a way the wire never
was) to buy back a step already done. A reader who wants the transport reads the comment; a
reader who infers it from IP protocol 17 has been told plainly here not to.

For the same reason the UDP checksum is written as zero — "not computed", which is what it in
fact is, and legal on IPv4. Note that a zero UDP checksum is *not* legal on IPv6 (RFC 8200
§8.1), so a strict reader may flag IPv6 packets in a capture; the alternative is a pseudo-header
checksum over a datagram that never existed, which is more invention for a cosmetic gain. The
IPv4 header checksum **is** computed, because it covers only the twenty bytes actually written
and costs six lines, and leaving it zero would have every tool flag every packet — the exact
noise this section is otherwise trying to avoid.

### 13.2 Faithfulness

**Ordering is established in the driver loop; the write is not performed there.** At the point
the bytes go to or come from the socket, the loop stamps the record with a monotonically
increasing sequence number and a timestamp, and hands it to a writer over a bounded channel.
The writer owns the file and runs off the loop.

This split is the whole of "enabling it must not change message ordering or timing", and it is
worth saying why the obvious alternative is wrong. Writing inline — one buffered write per
message, on the loop — reads as the more faithful design and is not. The loop that writes is
the same loop that fires retransmission timers, so an inline write puts the filesystem in the
retransmission path: a slow or full disk then delays Timer A, which is precisely the
"observation that perturbs a retransmission race" the story forbids, and it fails worst in the
disk-full case this section already anticipates below. **Do not re-introduce the inline write.**

Faithfulness does not depend on where the syscall happens. It depends on the order being
*decided* at the observation point, which is what the sequence number records. The writer may
fall behind the loop; it cannot reorder what it was given, because the order is data by the time
it arrives. What the writer can do is run out of room: the channel is bounded, and an overrun
drops records rather than blocking the loop — counted as `capture_dropped`, never silent
(§12.1). A capture with a gap that says so is usable; a stack that stalled to avoid the gap is
not.

UDP datagrams are captured before parsing, so a malformed message is captured malformed — the
bytes a peer actually sent are the whole point of the exercise. On stream transports the
framing happens in the connection's task and the raw bytes are not retained, so the message is
captured as parsed and re-serialised; start lines and header values are preserved byte-for-byte,
but the capture is not a byte-exact record of the stream and does not pretend to be one.

A write that fails (a full disk is the usual reason) is logged once, counted in the snapshot
(`capture_errors`), and disables the capture: a capture that is silently not happening is the
same failure as a silent discard, one level up.

### 13.3 Redaction

**[sipx]** Secrets are removed before a record reaches the writer, on by default. The rule is
narrow and mechanical: replace the *value* of a field that is a live credential, keep everything
that makes the message diagnosable.

| Redacted | Where | Why it cannot stay |
|---|---|---|
| `response` parameter | `Authorization`, `Proxy-Authorization` | The digest response is the answer to a challenge; with the nonce beside it in the same capture it is replayable (RFC 7616 §5.5). |
| `nextnonce`, `rspauth` | `Authentication-Info` | Server-side halves of the same exchange. |
| Key after `inline:` | SDP `a=crypto` | The SRTP master key **in the body** (RFC 4568 §6.1). It decrypts the media the same capture may be describing. |
| `pn-prid`, `pn-param` | `Contact` | A push token is a bearer credential for waking a device (RFC 8599 §4). |
| `+sip.instance` URN | `Contact` | A stable device identifier that outlives the call and correlates a user across captures (RFC 5626 §4.1). |

Kept deliberately: request lines and status codes, `Call-ID`, `CSeq`, `Via` and `branch`, `To`
and `From` including display names and AORs, `realm`, `nonce`, `qop`, `algorithm`, `opaque`, the
`a=crypto` tag and crypto-suite, and every SDP line that is not key material. Each is either
required to follow a transaction or is the thing a support case is about. The challenge
parameters stay because a digest failure is unreadable without them and a nonce with no response
beside it is not a credential.

Three consequences, stated rather than discovered later:

- **Redaction is by name, so an unknown credential-bearing extension header is kept.** The list
  is the specified places a secret appears, not a guarantee that nothing else is sensitive.
- **What remains still identifies people.** `To`, `From` and the SDP's addresses survive, and
  they are enough to say who called whom, when, and from where. This is the residue the §13
  disclosure is about.
- **Redaction changes the bytes**, so a redacted record is not byte-exact. The block comment
  says when a record was altered, and §13.2's stream caveat already means byte-exactness is not
  claimed for stream transports.

An endpoint may opt *out* — a capture taken in a lab against a test registrar has no secrets
worth removing, and forcing redaction there would hide a digest bug from the one capture taken
to find it. Opting out is explicit, per endpoint, and never the default. **It is deliberately not
reachable from the command line**: a flag would put "ship the credentials" one word away from
whoever is debugging an incident, which is when they are least able to weigh it.

#### 13.3.1 Every spelling, not the common one

**[sipx]** Redaction reads raw bytes, because a datagram is captured before parsing (§13.2) and a
message that does not parse is exactly where a credential turns up somewhere unexpected. The price is
that the scan cannot assume one spelling of a header, and a first implementation did: it matched the
literal `authorization:` against physical lines split on CRLF. Three legal spellings walked past it,
each carrying a digest response into a capture in cleartext — a folded header (§7.3.1), an
`Authorization : …` with the whitespace HCOLON permits (§25.1), and a bare-LF message, which became
one long line and so matched no header name at all.

The rule is therefore structural rather than literal:

1. Lines are split on **CRLF, bare LF or bare CR**.
2. Continuation lines are **unfolded** into one logical header before anything reads it, because a
   fold can fall inside a parameter name. The fold becomes a single space, as §7.3.1 says; if that
   yields no credential and the header was folded, the fold is removed entirely and the line is
   scanned again. A fold inside a token names no parameter in SIP, but "no parser would read that as
   a credential" is a worse thing to be wrong about than one extra scan.
3. A header's **name is the bytes before its first colon with trailing whitespace trimmed**, not a
   prefix.
4. **A line whose name cannot be established is redacted conservatively**, not skipped. Where the
   structure is absent a credential could be anywhere, and being wrong that way costs a mangled value
   in a capture instead of a leaked one.

Two consequences are worth stating. A **redacted** header is written back unfolded, since the fold is
equivalent to a space and a redacted record is not byte-exact in any case; an untouched header keeps
its original bytes, folds and terminators included. And the **body** is never re-terminated — its
length is declared in `Content-Length`, so a bare LF inside an SDP body stays a bare LF.

#### 13.3.2 Also removed

**[sipx]** Beyond the parameters §13.3 tabulates:

- **An opaque credential.** `Authorization: Bearer <token>` (RFC 8898) and the long-deprecated
  `Basic <base64>` carry the credential *as* the value, so there is no parameter to find and the whole
  of it goes. An unrecognised scheme whose value contains no `=` is treated the same way, because a
  token68 is the only other thing it can be. The scheme name is kept: which scheme failed is the
  diagnosis.
- **Every `inline:` key on an `a=crypto` line**, not the first. RFC 4568 §9.1 is
  `key-params = key-param *(";" key-param)`, and a single-occurrence search left the second key in
  the file.
- **SDP `k=`** (RFC 4566 §5.12). Deprecated by its own RFC and still a key in cleartext. The method is
  kept and the key goes; `k=prompt` names no key and is left alone.
- **A credential in a nested message.** `message/sipfrag` (RFC 3420) and multipart bodies put real
  headers where the body scanner sees body lines, so a credential header found there is redacted too —
  length-preservingly, because it is inside the body.

A `quoted-pair` does not end a quoted value (§25.1), so an escaped quote inside a digest response does
not leave its tail behind.
