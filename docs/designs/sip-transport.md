# Design: Transport layer

**Status:** active · **Pillar:** Signalling · **Epic:** `sip-transport` ·
**Stories:** T-1 … T-4, T-22

## Why

The transport layer is the only place in the signalling stack that touches the network, which
makes it the only place that can get NAT, connection lifetime and target resolution right — or
wrong. It is also the driver for the sans-IO core, so its contract determines whether the
core's testability survives contact with a real runtime.

## Approach

_To be written by `T-1`, which produces `docs/specs/sip-transport.md`. In outline: an async
driver owning sockets and a timer wheel, feeding `Input`s to `sipx-sip` and executing its
`Output`s; a connection pool keyed by (transport, remote); RFC 3263 resolution behind an
injectable resolver; per-transport feature flags so a UDP-only build carries no TLS or
WebSocket code._

## `respond` and the wire: a guarantee, and why it is a type

**Decision: the ordering is a guarantee of `respond`, not an internal detail.** When `respond`
returns `Ok`, the response has been handed to the kernel. An application is entitled to rely on it,
because the alternative is the failure the code already names at the `NoTransaction` branch: telling
an application its 200 OK went out while the caller heard nothing, so the caller times out believing
the call failed while the callee believes it is up.

**It is enforced by the type system rather than by a test, and that was forced on us.** `perform`
hands back a `Performed`, and the `Ok` that `respond` reports is obtainable only by consuming it, so
reversing the two statements does not compile — `error[E0425]: cannot find value 'performed'`.

The reason it is not a test is worth keeping, because it is counter-intuitive: **no black-box test can
observe the reversal.** On a `current_thread` runtime, sending on a oneshot does not yield, so the
send always completed before the waiting task was polled — the datagram was out whichever order the
lines were written in. `respond_returns_only_once_the_response_has_been_sent` passed with the
ordering reversed, which is how it stood for as long as it did.

The 50 ms bound that used to stand in for the check is gone. It bought no detection power at any
value, and the argument defending it was wrong on its own arithmetic: it rested on a queued send
being flushed "within a packet interval", which is 20 ms — inside the 50 ms it was justifying. What
remains is a generous deadline that is a bound on *failure* in `X-29`'s sense, where load can only
lengthen it.

Filed and closed as `X-36`; the mistaken rationale it replaced came from `X-29`.

## Overload control belongs on the endpoint loop

RFC 7339 feedback is hop-by-hop state about the destination the transport selected, and admission
must happen before the transaction layer creates a request. The endpoint driver therefore owns one
controller keyed by peer IP and port. Responses update it on the same serial loop; commands consult
it before creating a transaction or writing a direct request. This preserves the layer's existing
single-owner rule and makes an out-of-order response unable to race an outbound request.

The controller receives elapsed time and randomness as inputs. Production supplies runtime time and
an operating-system random source; unit tests supply explicit instants and a seeded source. Policy
is separate from arithmetic: endpoint configuration carries a request-category function, while the
RFC algorithms decide how much of each category can pass. That separation lets an application
protect established-dialog or emergency traffic without replacing sequence, validity, loss, or rate
machinery.

Server feedback is deliberately narrow. The existing bounded application queue is the overload
detector, and its existing 503 path reports the configured feedback in the response `Via`. There is
no second queue, sampler task, or clock-driven load estimator hidden in the transport. A future
estimator can change the configured report; it does not need to replace the wire or client state
machines.

Every response still completes RFC 7339 negotiation. The driver decorates application responses
from the original server transaction with a selected algorithm, zero validity and an increasing
sequence; that is the explicit control-off state. The queue-full path uses the same decorator with
active feedback and non-zero validity. Keeping both paths behind one helper prevents ordinary 2xx
responses from accidentally echoing the client's valueless offer as though the server had selected
nothing.

That helper sits at `Output::Send`, not only at the application command. The transaction layer also
originates 100 Trying and retransmissions, and they are responses under RFC 7339 just as much as a
2xx. Queue-full is a detector event with the same lifetime the server advertises; it is refreshed
by another shed and remains the value all responses report until expiry. A final response for an
earlier admitted request therefore cannot cancel control while the queue is still shedding.

Feedback is accepted at the matching boundary for the inverse reason. Parsing a response proves
only that bytes resemble SIP; transaction matching proves this endpoint sent the request that
offered control. The bounded per-peer cache is LRU, preferring inactive entries for eviction, so a
forged or simply unmatched response cannot allocate state and inactive history does not grow
forever.

Rate priority is arithmetic rather than a bypass. Ordinary and protected requests share one leaky
bucket and therefore one long-term rate, but compare its content with `TAU1` and `TAU2`
respectively. The higher protected threshold permits important traffic after ordinary traffic is
blocked without creating a second bucket that could double the server's allowed rate.

## Alternatives considered

- _Pending `T-1`._

## Risks & open questions

- Connection reuse (RFC 5923) is a security-relevant default: reusing an inbound connection
  for outbound requests is convenient and is also how a malicious peer gets requests routed
  through it. The default must be decided deliberately in `T-1`.
- Backpressure: what happens when the application cannot consume events as fast as the
  network delivers them. Dropping and blocking are both wrong in different ways.

## Acceptance / done

The union of `T-1`…`T-4`: messages sent and received over each enabled transport, `rport`
handled, connections pooled and reused, targets resolved per RFC 3263, and a loopback harness
proving the core is driven correctly.
