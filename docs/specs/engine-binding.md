# Spec: The engine binding (embedded runtime)

**Status:** draft — `A-6` finishes it; vectors required before any code · **Epic:** `app-host` ·
**Design:** [embedded-runtime](../designs/embedded-runtime.md)

> The third transport of the same contract: no wire at all. A handler file runs inside the
> host and its SDK calls resolve to host bindings directly. This spec exists to pin the
> discipline that makes that safe and semantics-preserving; the engine choice itself is the
> design's.

## 1. Semantics

- **[sipx-app]** The binding implements the contract's session-mode semantics minus the
  socket: same envelopes and documents as values (never re-serialized strings on a hot path,
  but **defined by** the JSON forms — a vector may serialize both sides and compare), no
  alternation rule, `originate` available if granted.
- **[sipx-app]** A handler observably behaves identically under this binding and under the
  session binding. The reference applications are the test: same files, both bindings, same
  outcomes under the harness. Any divergence is a defect in this binding, definitionally.

## 2. Isolation

- **[sipx-app]** The handler receives exactly: the SDK surface, granted capabilities, and
  contract values. No file system, network, environment, process or clock access exists in
  its global scope; time visible to a handler is the envelope's `at` and timer verbs.
- **[sipx-app]** Capability refusals are the same typed outcome an ungranted contract verb
  produces — observable, testable, and identical across bindings.
- **[sipx-app]** One handler failure (throw, unhandled rejection, engine termination) is one
  call's declared `on_unreachable`; it must be unable to take a sibling call with it. The
  isolate-per-what decision (`A-6`) must be made under that constraint.
- The docs must say plainly: this is capability isolation inside one process, not an OS
  boundary. The stronger isolation is session mode, and recommending it for untrusted code is
  the honest default.

## 3. Open until A-6

Isolate granularity and pooling; handler load/reload lifecycle; CPU/heap budgets and what
exceeding one maps onto; TypeScript transpile caching; and the vector set (parity suite with
the session binding, refusal vectors, failure fan-out).
