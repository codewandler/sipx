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
| `identity` | The name whose certificate was verified, for connections sipx opened over TLS. |
| `path` | The resource the upgrade asked for, for WebSocket connections sipx opened. |
<!-- END generated:pool-key -->

**[sipx]** The pool is bounded (default 1024), and connections are closed after an idle timeout
(default 5 minutes). Eviction is least-recently-used, and a connection with a live transaction
is never evicted.

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

The snapshot covers, at minimum:

- requests and responses, in and out, **per transport** — which transport is the first
  question a support case asks;
- requests shed for backpressure (§10), embedded as the existing `ShedCounts`;
- responses that matched no client transaction (RFC 3261 §16.7), counted whether or not an
  application is watching for them;
- parse failures, per transport — a malformed datagram and a stream whose framing is lost are
  the same failure on different transports;
- retransmissions sent — a rising count with no matching traffic growth is a peer that is not
  hearing us, and the difference between a network problem and an application one;
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

## 13. Capture

**[sipx]** An endpoint can record the signalling it exchanges to a file: every message sent
and every message received, with a timestamp, the transport, and both addresses, bodies
included. Off by default; enabling it is per endpoint (`Config::capture`) and costs an
`Option` check per message when off.

**The file contains the decrypted messages — credentials, call content, everything the peers
exchanged.** TLS and WebSocket-over-TLS traffic is captured *before* encryption on send and
*after* decryption on receive, because capturing ciphertext from inside the process would be
strictly worse than capturing it from outside. Whoever enables a capture is responsible for
the file.

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

Packets are written with `LINKTYPE_RAW`: a synthesised IPv4 or IPv6 header, then the real
transport header — UDP as-is, TCP with per-connection synthetic sequence numbers so analysis
tooling can reassemble streams — then the message. The addresses and ports are the real ones;
only the link layer is invented, because there is no link layer inside a process. Checksums
are computed, not zeroed, so a strict tool does not flag every packet.

### 13.2 Faithfulness

Writing happens in the driver loop, at the point the bytes go to or come from the socket —
not on a channel to a writer task. That is what "enabling it must not change message ordering
or timing" means concretely: the capture observes the same ordering the wire sees, and the
cost of observation is one buffered write per message, paid only when capture is on.

UDP datagrams are captured before parsing, so a malformed message is captured malformed — the
bytes a peer actually sent are the whole point of the exercise. On stream transports the
framing happens in the connection's task and the raw bytes are not retained, so the message is
captured as parsed and re-serialised; start lines and header values are preserved byte-for-byte,
but the capture is not a byte-exact record of the stream and does not pretend to be one.

A write that fails (a full disk is the usual reason) is logged once, counted in the snapshot
(`capture_errors`), and disables the capture: a capture that is silently not happening is the
same failure as a silent discard, one level up.
