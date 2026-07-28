# Design: Application SDK

**Status:** accepted — spec first, stories cut · **Pillar:** Application · **Epic:** `app-sdk` ·
**Stories:** `C-3` `C-4` `C-5` `C-6` `M-17` `M-18`

## Why

The measure of this stack's reach is what can be built on it **without writing Rust**. Today the
answer is: nothing beyond what the CLI hard-codes. Every consumer of `sipx-call` writes Rust
against a Rust API; the one scripted behaviour that exists (`sipx dial --play --dtmf --record`)
is a fixed sequence baked into the binary. The downstream cluster platform serves operators —
it forwards and registers, and by its own charter it does not terminate calls. Nobody serves the
*application author*: the person who wants "answer, play a prompt, collect digits, connect me to
a colleague" as a program in their own language.

The organizing idea of this epic is the same one the [vision](../vision.md) states for the
protocol core: **express the logic as a function from inputs to outputs, and keep the I/O in a
driver.** Applied one layer up: define the application contract as *data* — call events out,
call-control instructions in — and implement its interpreter as a sans-IO state machine in this
repository. Then every host — a remote webhook service, a subprocess speaking the contract over
a socket, an embedded script runtime in the host crate — is a thin driver over the same
tested machine, and the remote ones need nothing from this workspace but the wire.

The SDK does not add a dial plan to sipx. Routing engines and dial plans remain things built
*with* sipx; this epic moves "build with sipx" across the language boundary.

## Approach

Three pieces belong to this epic; the host itself is the [app-host](app-host.md)
epic's, one shelf up in the same workspace.

1. **The contract.** [`docs/specs/app-contract.md`](../specs/app-contract.md) defines
   `sipx.app.v1`: a closed, typed vocabulary of events (with a full call snapshot per event)
   and instructions (with client-assigned ids and correlated completion events), an envelope
   with per-call sequence numbers and authentication, and a normative continuation rule for the
   stateless binding. The wire line is versioned independently of any crate version and is
   **experimental** until two dissimilar applications run against it (an inbound IVR and an
   outbound notifier). Spec before code, per the working agreement.

2. **The types and the interpreter** (`C-5`). A new crate — working name `sipx-app-protocol` —
   owns the contract DTOs (serialization stays confined to this crate; `sipx-call` gains no new
   dependencies) and a sans-IO **instruction interpreter**: a state machine that consumes call
   events and a program of instructions, and yields effects that map one-to-one onto `sipx-call`
   operations. No socket, no clock, no async runtime — the spec's vectors drive it in unit
   tests, and an `examples/` binary drives it over a real call with a canned program, so the
   contract is provable from a shell before any host exists.

3. **The call-framework surface the interpreter's effects need** (`C-3`, `C-4`, `C-6`, `M-17`,
   `M-18`). Today a `Call` reports state only through method calls on itself; `serve()` drives
   exactly one call and drops what that call does not claim; the media bridge and the
   conference mixer cannot be reached from a `Call` at all; playback cannot be stopped once
   started; and mute does not exist. Each gap is one story:

   | Story | Delivers | Order |
   |---|---|---|
   | `C-3` | `CallEvent` stream — a typed, channel-backed event source per call | first; the keystone the others report through |
   | `C-4` | Multi-call dispatch — one endpoint's `Incoming` stream routed to N calls, nothing dropped silently | after `C-3` |
   | `M-17` | Playback control — queue, stop, interrupt-on-digit | after `C-3`; gates the contract's `gather` |
   | `M-18` | Mute/unmute — a local media gate, distinct from hold | independent, small |
   | `C-5` | Contract crate + sans-IO interpreter | parallel to all of the above |
   | `C-6` | `Bridge`/`Conference` reachable from two host-owned `Call`s | last; **not** v1-blocking |

   `C-6` is deliberately scoped to the *media* coupling of two calls the host owns. The
   signalling coupling — offer relay on every axis, glare, CANCEL/BYE mapping — is `C-1` (M9,
   after `S-19` and `C-2`), and when it lands it upgrades the contract's `bridge` verb from
   naive per-leg signalling to a real coupling without changing the verb.

**The host lives here, as `crates/sipx-app`.** The [app-host](app-host.md) epic implements the
contract's bindings — the webhook document mode, the socket session mode, and an embedded
TypeScript runtime, all three over the one vocabulary — as a leaf crate no kernel crate ever
depends on. What keeps this epic's list honest is the same discipline an external consumer
would impose: each story here names the host story that needs it (`A-2` names four of the six),
and the host may not reach around this public API — a gap is a story here, not a workaround
there.

**Sequencing against the roadmap.** M7 lives in `sipx-transport`/`sipx-sip` and M8 in
`sipx-sip`/`sipx-ua`; this epic lives in `sipx-call`/`sipx-media` plus one new crate. No file
overlap — the epic runs beside them, the same argument M6 makes for its own three tracks. The
spec costs no code at all and can land immediately.

## Alternatives considered

- **Host the webhook server in a separate repository**, pulling kernel gaps through an
  upstream ledger the way the cluster platform does. This design first chose that, and the
  choice was reversed the same day by the user's decision: the host is `crates/sipx-app`, in
  this workspace. The reversal's reasoning is recorded in [app-host](app-host.md) — the
  separation's benefits (dependency hygiene, no reaching around the public API) are kept as
  ground rules a leaf crate can honour, and its cost was real: a contract, an interpreter and
  a host iterating across a tag boundary during exactly the phase they must move together.
- **Host it in the cluster platform.** Rejected by that platform's charter: it forwards, forks
  and record-routes; it does not terminate calls, and a feature server in its core is one of
  its named non-goals.
- **Embed a script engine in the protocol or call crates so handlers run everywhere.**
  Rejected: the engine's weight and sandboxing policy belong to the host crate alone
  ([app-host](app-host.md) ground rule 4), and the contract makes the engine unnecessary
  anywhere else — an embedded runtime in `sipx-app` binds the same vocabulary every remote
  host speaks.
- **Ship the contract types with the host crate instead of their own.** Rejected:
  `sipx-app-protocol` is the piece a *remote* SDK generator and the host both consume, and the
  interpreter is the primitive (the `C-1` precedent) — it belongs beside the call framework it
  drives, importable without the host's HTTP stack and engine coming with it.
- **A per-verb RPC API instead of an instruction program.** Rejected: a call is a real-time
  process; "what to do next" must survive the app being slow or gone. An ordered program with
  declared failure semantics degrades predictably; a stream of imperative RPCs does not.

## Risks & open questions

- **Ossification.** A vocabulary frozen before two real consumers exist hardens wrong shapes.
  Mitigation: the wire line stays experimental until the IVR and the notifier both run; v1
  keeps the verb set minimal (no conference verb, no record-to-URL, no early-media playback —
  `C-2` gates that last one).
- **Split-brain call state.** Apps will cache state and drift on a missed delivery. Mitigation
  is in the contract: every event carries a full snapshot and the spec names the event stream
  authoritative.
- **`play` scope creep.** URL fetch, TTS and streaming each smuggle dependencies and security
  policy into whatever executes them. v1 sources are host-local files and inline audio only;
  anything remote is a host capability behind an allowlist, and TTS is a named non-goal.
- **Naming.** `sipx-app-protocol` and `sipx.app.v1` should be
  settled before the docs site publishes them; renames after publication are expensive.
- Open: whether `C-3`'s event set should carry media-quality events (`quality()` snapshots on
  an interval) in v1 or wait for a consumer that wants them.

## Acceptance / done

The union of the six stories' acceptance, plus: the spec exists with vectors; the interpreter
passes every vector sans-IO; and the `examples/` binary answers a real call, runs a canned
program (play → gather → hang up) against it, and a shell script asserts the outcome — the
contract demonstrated end-to-end with no host and no new workspace dependencies.
