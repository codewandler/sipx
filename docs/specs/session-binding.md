# Spec: The session binding

**Status:** draft — `A-4` finishes it; vectors required before any code · **Epic:** `app-host` ·
**Design:** [host](../designs/host.md)

> The wire is the contract's §8 (app-contract.md): envelopes and documents as JSON text frames, no
> alternation rule, `originate`, declared backpressure, binary frames reserved. This spec
> adds the host's side: session establishment, multiplexing, liveness, and what a dead
> session means for the calls it carried.

## 1. Establishment

- **[sipx-app]** WebSocket server on a configured listener; an app authenticates at upgrade
  with a named bearer secret ([host-config.md](host-config.md)). One session serves one app;
  an app may hold several sessions (deployment redundancy), and **[sipx-app]** each call is
  pinned to exactly one session for its lifetime — two sessions never both own a call.
- The same binding over a subprocess pipe (stdin/stdout framing) is the same spec with
  establishment replaced by process spawn; `A-4` decides whether it ships with A2 or waits.

## 2. Liveness and backpressure

- **[sipx-app]** Liveness is protocol-level ping with a declared interval and grace; a session
  that misses the grace is dead. Backpressure per the contract: bounded per-session outbound
  queue; on overflow, close 1013 and treat as dead.
- **[sipx-app]** A dead session applies each carried call's declared `on_unreachable` —
  individually, through the same code path the webhook binding uses. Reconnection is a new
  session: calls that already resolved their failure semantics do not migrate to it.

## 3. Multiplexing

- **[sipx-app]** Every frame names its call; a document for an unknown or ended call is
  answered with a typed error frame and otherwise ignored (it is a race, not an attack).
  `originate` is the one frame with no prior call; its result introduces the new call id.

## 4. Open until A-4

The subprocess variant's framing; per-session concurrency limits; the error-frame shape
(candidate: a `sipx.app.v1` error envelope rather than a bespoke frame); and the vector set
(pinning, dead-session fan-out, overflow close, unknown-call race, originate).
