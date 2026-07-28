---
id: X-20
title: Let a caller take the digest primitives without taking a runtime
pillar: Build
status: done
priority:
design: docs/designs/sip-core.md
epic: sip-core
areas: [sipx-ua]
note: sipx-ua pulls tokio unconditionally, so sans-IO code cannot use S-16's Authenticator
---

# Let a caller take the digest primitives without taking a runtime

## Goal
Make `sipx-ua`'s digest primitives — `auth` and the `challenge` module `S-16` added — usable by a
caller that has no async runtime, by feature-gating the parts that genuinely need one.

## Why
`S-16` put challenge minting and verification in `sipx-ua`, which is the right crate for them: they
are user-agent behaviour, and they share the client side's hash formulas rather than repeating them.

But `sipx-ua` depends on `tokio` and `sipx-transport` unconditionally, and only two of its seven
modules need either. A caller whose whole design is that decision logic touches no IO — a proxy or
registrar built sans-IO, which is the shape this kernel's split exists to support — cannot use the
authenticator without linking a runtime into its decision core. That caller's alternative is to
write digest a second time, which is precisely what `S-16` was meant to prevent.

The split is already clean, so this is a manifest change rather than a refactor:

| Module | Needs a runtime | Why |
|---|---|---|
| `auth` | no | hashing and header text |
| `challenge` | no | hashing, header text, and a `u64` clock argument |
| `outbound` | no | RFC 5626 bookkeeping over response headers |
| `registrar` | no | builds REGISTER requests and reads responses |
| `error` | **yes** | `Error::Transport` wraps `sipx_transport::Error` |
| `agent` | **yes** | drives `sipx_transport::Handle` |
| `flows` | **yes** | drives `agent` |

## Acceptance
- [x] `sipx-ua` gains a `runtime` feature, on by default, carrying `tokio` and `sipx-transport`.
      Existing dependents are unaffected — the default build is what it is today.
- [x] `sipx-ua` with `--no-default-features` exposes `auth`, `challenge`, `outbound` and
      `registrar`, and resolves **no** `tokio` in its dependency graph.
- [x] Failing-first test: `scripts/check-features.sh` grows a `sipx-ua` block and an assertion on
      the resolved graph — `cargo tree` naming `tokio` under a runtime-free `sipx-ua` fails the
      script. Checking that it *builds* is not enough: the build would still succeed with the
      runtime linked in, which is the whole thing being ruled out.

## Progress
Done. `sipx-ua --no-default-features` gives `auth`, `challenge`, `outbound` and `registrar` with no
`tokio` and no `sipx-transport` in the resolved graph; the default build is byte-for-byte the API it
was, and `sipx-cli` — the only dependent — needed no change. 101 tests pass, clippy is clean under
both feature settings.

### The check caught two entanglements the manifest change alone would not have

**`Registration::outbound` named `agent::Flow`.** `Flow` is a pair of `outbound::InstanceId` and
`outbound::RegId` — no runtime anywhere in it — sitting in `agent` only because that is where it was
first written. A public field of a runtime-free struct referred into a runtime-gated module, so the
crate did not compile without the feature even though nothing about it needed one. `Flow` now lives
in `outbound` beside the two identifiers it is made of, and `agent` re-exports it, so `agent::Flow`
still resolves and no dependent sees a move.

**`outbound::keepalive_for` takes a `TransportKind`.** That one is honest: the function's whole job
is to answer a question about a transport, so it belongs with the transport and is gated, along with
the test that exercises it.

Neither was visible from reading the manifest. That is the argument for the check asserting on the
**resolved graph** rather than on whether the build succeeds — the second entanglement would have
compiled fine had `sipx-transport` merely been left non-optional, which is exactly the outcome that
looks like success and delivers nothing.

## Notes
- Feature-gating rather than a `sipx-auth` crate split. A split is the better long-term shape if the
  surface grows, but it moves public API between crates — a breaking change for every dependent — to
  solve a problem a default feature solves without one. Worth revisiting when something other than
  digest wants the same treatment.
- The gate is on the **resolved graph**, not on the build. `check-features.sh` exists because a
  feature combination that compiles is not the same as a feature combination that is correct, and
  this is the same lesson in the dependency direction.
