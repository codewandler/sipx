# Spec: Transport layer

**Status:** normative · **Crate:** `sipx-transport` · **Stories:** T-1 … T-4 · **Design:**
[sip-transport](../designs/sip-transport.md)

## 1. Normative references

- RFC 3261 §18 (transport), §18.1 (clients), §18.2 (servers), §18.2.1 (`received`),
  §18.2.2 (sending responses), §19.1.1 (`maddr`), §20.42 (`Via`).
- RFC 3581 — `rport`.
- RFC 3263 — locating SIP servers.
- RFC 2782 — SRV weighting.
- RFC 5923 — connection reuse.
- RFC 7118 — SIP over WebSocket.

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

## 3. Timers

**[sipx]** A single earliest-deadline-first queue for the whole endpoint, keyed by
`(TransactionKey, Timer)`, not one task per timer. A busy proxy has tens of thousands of live
timers and spawning a task for each is how a stack acquires a scheduling problem it cannot
profile.

**[sipx]** `ClearTimer` marks the entry dead rather than removing it from the middle of the
queue; the entry is discarded when it surfaces. Cancellation is common and cheap this way.

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

**[RFC §18.1.1]** A request that would exceed the path MTU on UDP must be sent over a
congestion-controlled transport instead. **[sipx]** sipx checks against a configured MTU
(default 1300 bytes, the usual conservative value) and returns an error naming the size rather
than silently truncating; automatic switching to TCP arrives with the story that implements
`Route` handling, because switching transports changes the `Via` and therefore the transaction.

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

## 8. Connection reuse (RFC 5923)

**[sipx] An inbound connection is reused for responses on that transaction, always.**
**[sipx] An inbound connection is *not* reused for unrelated outbound requests by default.**

Reusing it is convenient and is also how a peer that connected to you gets your outbound
traffic routed through it. The default is off; a configuration flag turns it on for
deployments where both ends are trusted. This is the security-relevant default `T-1` was asked
to decide, and this is the decision.

**[sipx]** Connections are pooled by `(transport, remote address)`, bounded (default 1024), and
closed after an idle timeout (default 5 minutes). Eviction is least-recently-used, and a
connection with a live transaction is never evicted.

**[sipx]** When a connection closes, every transaction bound to it is given a transport error
rather than being left to time out. Waiting 32 seconds to discover something we already know
is a bad experience and a resource leak.

## 9. Resolution (RFC 3263)

Order: if the URI names an IP address or a port, that is the answer and nothing is looked up.
Otherwise NAPTR → SRV → A/AAAA, with RFC 2782 weighted selection among equal priorities.

**[RFC]** A `sips:` URI restricts candidates to TLS-capable transports.
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
