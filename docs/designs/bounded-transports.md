# Design: bounded transport lifetimes

**Status:** proposed · **Pillar:** Signalling · **Epic:** `bounded-transports` · **Stories:** T-25,
T-26, T-27

## Why

The endpoint exposes connection and queue limits, but those numbers do not currently bound every
resource they name. Removing a pooled entry can leave its socket task alive, inbound TLS and
WebSocket handshakes exist before pool admission without a deadline or concurrency limit, and a
zero queue capacity reaches an infallible runtime constructor that panics. The result is a gap
between the public configuration contract and the live tasks, sockets and memory it actually bounds.

## Approach

Make the endpoint own one explicit lifetime for every transport task and validate the budget before
any I/O begins.

- A pooled stream entry carries cancellation and task-completion observability. Idle and LRU eviction
  request shutdown; removal is not considered complete merely because the map entry disappeared.
- Pre-pool TLS and WebSocket handshakes acquire a bounded permit before their task is spawned and run
  under a configured deadline. Timeout, failure and endpoint shutdown release the permit and close the
  socket.
- Endpoint configuration validation rejects unusable capacities and intervals before listeners bind
  or tasks spawn. Runtime constructors receive only validated values.
- Tests observe externally meaningful termination: peer EOF, task completion, permits returned and no
  listener created after invalid configuration.

The pool's identity and routing rules remain governed by `docs/specs/sip-transport.md`; this design
changes ownership and admission, not the connection key.

## Alternatives considered

- Count only entries in the pool map. Rejected because a removed entry can continue owning a socket
  and task.
- Let handshakes enter the ordinary pool first. Rejected because an incomplete handshake does not yet
  have all of the authenticated and protocol state expected of a reusable connection.
- Clamp zero values to one. Rejected because silently changing a public resource budget hides a caller
  error and makes configuration inspection disagree with runtime behavior.

## Risks and open questions

- Cancellation must interrupt a task blocked on socket reads without turning ordinary peer closure
  into an error storm.
- The handshake limit and deadline need public defaults that preserve normal slow-network operation;
  their exact values belong in the transport spec before implementation.
- Shutdown must not wait forever for a peer after cancellation.

## Acceptance / done

The epic is done when T-25 through T-27 are done and tests demonstrate that configured transport
limits bound live tasks and sockets, not just bookkeeping entries, for TCP, TLS, WebSocket and secure
WebSocket endpoints.
