# Design: The TypeScript SDK

**Status:** proposed · **Pillar:** Application · **Epic:** `app-host` · **Stories:** `A-3` `A-4`

## Why

The contract is JSON either way; the SDK exists so a handler author writes call logic, not
protocol handling — correlation ids, snapshots, redelivery and backpressure handled once,
in one audited client, instead of in every app.

## Approach

One npm package (working name `@sipx/app`), one handler shape, two homes:

```ts
import { serve } from "@sipx/app";

serve({
  async onCall(call) {
    await call.answer();
    const digits = await call.gather({
      max: 4,
      terminators: "#",
      prompt: { file: "welcome.wav", interruptible: true },
    });
    if (digits === "0") {
      const leg = await call.dial("sip:reception@example.net");
      await call.bridge(leg);
    } else {
      await call.hangup();
    }
  },
});
```

- `serve` connects session mode (WebSocket) when run as a process; under the embedded runtime
  the same module-level API is provided by the engine binding. Handler code does not know
  which.
- Every awaited verb is: send instruction with a generated `id`, resolve on the completion
  event carrying that `instruction_id`, reject on `call.ended` — cancellation and barge-in
  surface as typed outcomes, not exceptions by default.
- `call.state` is always the latest snapshot; the SDK replaces it wholesale on every event
  (contract rule: events are authoritative) and exposes `call.on(event, …)` for the handlers
  that want the stream itself.
- The imperative surface maps one-to-one onto contract verbs — the SDK adds **no vocabulary**.
  A convenience that cannot be expressed as contract instructions does not belong in it.

**Reference applications** (part of `A-3`/`A-4`, not samples written after the fact): the
inbound IVR and the outbound notifier — the two dissimilar consumers the contract spec
names as its exit-from-experimental gate. They run under the deterministic harness and against
real calls.

**Document mode is deliberately not in the SDK.** A webhook app is a plain HTTP endpoint
returning JSON documents; wrapping that in a client library would only obscure that it is one
request, one response. The SDK's value begins where correlation begins.

## Alternatives considered

- **Generate the whole SDK from the contract schema.** Types: yes, generated from the vectors'
  schema. The imperative layer: no — its worth is exactly the hand-designed ergonomics
  (awaitable verbs, snapshot discipline), and generators produce neither.
- **One package per binding.** Rejected: the portability promise *is* the product; two
  packages would let the APIs drift.

## Risks & open questions

- **API stability vs an experimental contract.** The SDK inherits the contract's experimental
  status and says so at install; no semver promise before the contract freezes.
- Open: how recording payloads reach the app (a `record` verb completes with what, exactly —
  a host path? a capability handle?). A contract question to settle in `docs/specs/app-contract.md` before `A-3`
  ships the verb.

## Acceptance / done

Both reference applications pass under the harness and against real calls in session mode;
the same files pass embedded once phase 3 lands; and the SDK's public API surfaces every contract
verb and event, nothing else.
