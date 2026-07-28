# Design: The application host

**Status:** proposed · **Pillar:** Application · **Epic:** `app-host` · **Stories:** `A-1` `A-2`
`A-4` `A-7` (and, via their own designs, [`A-3`](ts-sdk.md), [`A-5` `A-6`](embedded-runtime.md))

## Why

The [app-sdk](app-sdk.md) epic ends where a process has to exist: something must hold real
calls, own the sockets a contract binding needs, enforce per-app policy, and keep running when
the customer's code does not. That process is **`crates/sipx-app`** — in this workspace, by
decision (see Alternatives). The kernel ships the contract's *interpreter*; this crate is its
*server*.

## Ground rules

The four commitments every story in this epic is held to:

1. **The contract is the product.** `sipx.app.v1` is the entire power of a handler; bindings
   are transports over it. A feature that cannot be expressed in the contract is a contract
   change first (in [`specs/app-contract.md`](../specs/app-contract.md), with vectors), a host
   feature second. No binding may offer a side channel.
2. **The host does all privileged I/O.** Handlers never get a socket, a file system, an
   environment or a raw SIP message. Every side effect of an embedded handler is a host
   capability, deny-by-default, granted per app.
3. **Failure semantics are declared, never coded.** What a slow, wrong or absent app means for
   a live call is per-app configuration with stated defaults; a code path that hard-codes one
   is a defect.
4. **The host is a leaf.** No kernel crate may ever depend on `sipx-app`, and the sans-IO
   crates' non-negotiable is untouched by its existence. The engine, the HTTP stack and
   serialization live here and stop here.

Non-goals, stated once: not a proxy or registrar (that is the downstream cluster platform);
not a configuration-driven PBX (behaviour comes from handlers — configuration declares
capabilities, failure semantics and listeners, and never becomes a language); not a media
platform (TTS, URL-fetched audio and media frames over the wire wait for the contract to grow
them deliberately); not a package ecosystem (an embedded handler is one file with capabilities;
an app that needs `node_modules` runs in session mode as its own process).

## Approach

One process, three layers, and the interpreter is not one of them — it is imported:

1. **Calls.** A transport endpoint per listener; multi-call dispatch (`C-4`) feeds one actor
   per call. The actor owns the call, its event stream (`C-3`), and one instance of the
   `sipx-app-protocol` interpreter (`C-5`). Effects the interpreter yields are executed against
   `sipx-call`; events the call yields are fed back in. The actor is the only place the two
   meet.
2. **Bindings.** Per app, one of: **document** (HTTP client —
   [specs/webhook-binding.md](../specs/webhook-binding.md)), **session** (WebSocket server —
   [specs/session-binding.md](../specs/session-binding.md)), **embedded** (the engine —
   [specs/engine-binding.md](../specs/engine-binding.md)). A binding adapts transport to the
   interpreter; it never interprets.
3. **Policy.** The app manifest ([specs/host-config.md](../specs/host-config.md)): which calls
   reach which app, declared failure semantics, capability grants, authentication material.
   Evaluated in the actor, once per decision, from configuration loaded at start.

**Determinism (`A-7`).** The actor's logic — interpreter, timers, redelivery, failure
semantics — runs under a harness with fake time, scripted bindings and scripted calls. This is
the sans-IO discipline applied one layer up, and it is built with the first host code, not
after it. It is startable today: it needs only the contract's vectors, none of the pending
call-framework stories.

**Phases.** Each independently demonstrable from a shell:
1. *One call, one webhook* (`A-1`, `A-2`, `A-7`) — document mode end to end, declared failure
   semantics proven for the slow, flapping and absent app.
2. *Session mode and the TypeScript SDK* (`A-3`, `A-4`) — `originate`, backpressure, and the
   two reference apps (inbound IVR, outbound notifier) that are also the contract's
   exit-from-experimental gate.
3. *The embedded runtime* (`A-5`, `A-6`) — the same handler files in-process, under
   deny-by-default capabilities.
4. *Operable* — multi-app isolation, management surface, packaging. Scoped when 1–3 have
   taught us what operating it needs.

## Alternatives considered

- **A separate repository, pulling kernel gaps through an upstream ledger** (the cluster
  platform's model). This was the first decision, and it was reversed the same day by the
  user's call: the host is a crate here. What the separation bought — dependency hygiene, no
  reaching around the public API — is preserved by ground rule 4 and by the workspace gate;
  what it cost was real: the contract, interpreter and host iterating across a tag boundary
  during exactly the phase where they must move together. One repository, one gate, one
  history.
- **Interpret instructions in the host instead of importing `sipx-app-protocol`.** Rejected:
  two interpreters is how the wire and the behaviour drift apart; the spec's vectors are the
  contract's meaning.
- **One task per binding rather than per call.** Rejected: a slow app must be able to stall
  exactly one call; per-call actors make the blast radius structural.
- **Dynamic app registration over an API in v1.** Deferred to phase 4: configuration at start,
  reloaded deliberately, is inspectable and testable.

## Risks & open questions

- **Scope pressure inside the workspace.** The failure mode the separate-repo option guarded
  against. The guard now is ground rule 4 plus review: a change in `sipx-app` that needs a
  kernel change files a story against the kernel crate, same as any consumer would.
- Open: whether one host process serves many apps in v1, or one app per process with "many" as
  an operational layer above (phase 4 decides; the config spec must not preclude either).
- Open: media file lifecycle for `record` — a capability design, deliberately not in the
  contract.

## Acceptance / done

Phase 1's criterion, verbatim: a shell script starts the host with a scripted webhook app,
places a call into it with the sipx CLI, hears a prompt, sends digits, asserts the gather
outcome — and the same script with the app stopped asserts the declared `on_unreachable`
behaviour. All scenarios also pass under the deterministic harness.
