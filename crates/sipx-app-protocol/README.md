# sipx-app-protocol

The `sipx.app.v1` application contract: its types, its JSON wire format, and a sans-IO
instruction interpreter.

## What this is

An application drives a call by receiving **events** and answering with **instructions**. It never
touches SIP. This crate is the vocabulary of that exchange, plus the state machine that runs it:

```text
      call events  ──▶┌──────────────┐──▶  effects   (answer, play, gather, dial, hang up …)
    fired timers   ──▶│ Interpreter  │──▶  timers    (set / clear)
   app documents   ──▶└──────────────┘──▶  deliveries (an envelope, and one callback token)
```

Everything in the box is a pure function of what went in. There is no socket, no clock read and no
async runtime anywhere in the crate — the driver's clock reading is a parameter of `handle`, so
"this crate never asks what time it is" is a property of the signature rather than a promise in a
comment. Every binding the spec describes (a webhook server, a WebSocket peer, an embedded
runtime with no wire at all) is a driver over this one machine.

## Stability

This crate remains experimental. Its exact public API and wire-line guarantees are maintained in the
[crate-level Stability section](https://codewandler.github.io/sipx/api/sipx_app_protocol/#stability).
That is the contract; it is linked rather than copied here so the two cannot drift.

## Two properties worth knowing about

**The continuation rule is held by types.** §6.3 requires at most one outstanding callback per
call, and requires a document accepted in response to an event to *replace* the pending program.
A `Callback` is neither `Clone` nor `Copy` and has no public constructor, and `Input::Response`
takes it by value — so a driver cannot answer one delivery twice. The pending queue is private and
the only operation that writes it is a whole-program assignment; there is no `push` and no
`append`, so a second document cannot leave a stale instruction behind.

**No dependencies.** The JSON codec, the base64 for inline audio and the RFC 3339 arithmetic are
all this crate's own. Serialization stays confined here, which is what lets `sipx-call` and the
rest of the workspace speak the contract without gaining a dependency for it. The optional `call`
feature adds the `sipx-call` adapter, and a remote SDK or a test of the state machine can have the
wire and the interpreter without it.

## Try it

The example drives the interpreter over a real call with a canned program — answer, play, gather,
hang up — with no host anywhere:

```sh
cargo run --example canned_program --features call
tests/canned_program.sh          # the same thing, with the outcome asserted
```

## See also

- [`docs/specs/app-contract.md`](../../docs/specs/app-contract.md) — the normative contract.
- [`docs/designs/app-sdk.md`](../../docs/designs/app-sdk.md) — why the interpreter is its own
  crate rather than part of the host.
