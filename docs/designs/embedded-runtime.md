# Design: The embedded TypeScript runtime

**Status:** proposed — engine decision recorded, implementation unscheduled until the host's
phase 3 · **Pillar:** Application · **Epic:** `app-host` · **Stories:** `A-5` `A-6`

## Why

Session mode already gives TypeScript handlers everything — as a separate process the customer
operates. The embedded runtime exists for the deployment where that is the wrong trade: one
binary, handler files beside the configuration, nothing else to run. It must add **zero new
semantics**: the same SDK API, the same contract, the engine binding as transport #3.

## Approach

The engine hosts the same handler API the TypeScript SDK exports; its calls resolve to host
bindings in-process instead of session frames over a socket
([specs/engine-binding.md](../specs/engine-binding.md)). Capabilities are the host's
deny-by-default grants; the engine gets no ambient file system, network, environment or clock
beyond what the binding hands it. A handler is one file; module resolution beyond the SDK
surface is deliberately absent ([app-host](app-host.md) non-goals: no package ecosystem inside the host — an app
that needs one runs in session mode).

### Engine choice

| | `deno_core` (V8) | `rquickjs` (QuickJS) | WASM component |
|---|---|---|---|
| TypeScript | transpiles natively | needs a build step | needs a toolchain per language |
| Async with the host | first-class, same event loop family as the host | job queue driven by the host | poll-based, host-driven |
| Isolation story | V8 isolate per app | interpreter instance per app | strongest: capability-typed boundary |
| Weight | heavy build, large artifact | small | medium, plus toolchain burden on users |
| Handler DX | "run my .ts file" | "run my .js file (we transpile when?)" | "compile your handler" |

**Decision: `deno_core`.** The requirement is literally "TypeScript, interpretable at
runtime"; only V8-with-transpile delivers that without a user-visible build step, and the
host's async model can drive it directly. The weight lands in `sipx-app` — a leaf crate no
kernel crate depends on ([app-host](app-host.md) ground rule 4) — and nowhere else in the
workspace. `rquickjs` stays recorded as the fallback if V8's build cost proves unacceptable in
practice; WASM stays recorded as the eventual polyglot boundary if handler languages beyond
TypeScript are ever wanted — it is a different product decision, not a swap.

## Alternatives considered

- **Skip embedding entirely; session mode is enough.** Live option — it is why this epic is
  phase 3 and not phase 1. Rejected as an end state because the single-binary deployment is a real
  audience, and because an engine binding is the cheapest proof that the contract really is
  transport-independent.
- **A bespoke DSL instead of TypeScript.** Rejected: the ecosystem's standing rule is that
  configuration never becomes a language; inventing one to avoid an engine would build exactly
  what that rule exists to prevent.

## Risks & open questions

- **Engine API churn.** `deno_core` moves quickly. Mitigation: the engine touches exactly one
  crate behind the binding spec; nothing else in the host may import it.
- **Sandbox honesty.** An in-process engine is not an OS boundary and the docs must say so
  plainly: isolation is the capability surface plus the isolate, and a hostile handler is the
  operator's trust decision. (Session mode remains the stronger isolation.)
- Open: handler lifecycle — per call, per app, or pooled isolates. Decided in `A-6` with
  measurements, not guessed here.

## Acceptance / done

The phase-2 reference applications run unmodified as embedded handlers; a handler reaching beyond
its grants is refused with an observable, tested refusal; and the engine crate is the only one
that knows which engine was chosen.
