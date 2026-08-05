# Design: supported test surfaces

**Status:** accepted · **Pillar:** Build · **Epic:** `test-surfaces` · **Stories:** X-75

## Why

The workspace has seeded links, virtual time and call fixtures, but downstream applications have no
supported package and guide for using them. A test facility available only to this repository is not
part of the library's usable surface, and it cannot be the foundation for later compatibility work.

## Approach

Publish one deliberately small in-process call harness whose time, bytes and loss are inputs. Keep it
silent unless the host installs tracing, compile its runnable example in CI, and inline that example
into the public guide. Decide the package boundary explicitly instead of exposing internal helpers by
accident. Cross-process benchmarking is a separate epic: it may consume this harness but must not
turn a deterministic library test into a wall-clock load generator.

## Supported boundary

`sipx-testkit::call::CallHarness` is the downstream application surface. It gives `sipx-call` two
ordinary `sipx-transport::Handle` values joined by a bounded in-process signalling driver. Its
`dial` path takes `DialOptions`, invokes `sipx_call::dial`, returns the exact `Incoming` invitation
for that attempt, invokes `sipx_call::answer`, and does not report establishment until the resulting
ACK has been handed to the answering `Call::handle`. The result owns both real `Call` values and
their normal event streams. A pending-call value owns one dial task, invitation and response stream;
no scan over earlier traffic can satisfy a later exchange.

The in-process boundary applies to SIP signalling. The calls intentionally create their ordinary
RTP/RTCP media ports, because a `Call` that bypassed `sipx-call`'s media negotiation would not prove
the application API. Tests that need deterministic packet faults rather than an application call
use `TransactionHarness` below; network interoperability remains a separate bounded integration
test.

The signalling driver admits at most its configured number of live response routes. A final
response removes its route before delivery; a later request prunes a route whose consumer was
dropped; and excess requests receive a typed overload error. Dropping a pending call aborts its
owned dial task, including when an answer fails before it can complete the exchange. Construction
without an entered Tokio runtime and dial failures before an INVITE exists are typed errors; neither
is allowed to panic or wait on an event that can no longer arrive.

`Link` is generic over its instant with the existing Tokio instant as its default, preserving its
current callers while allowing `TransactionHarness` to use a zero-based virtual instant. `Virtual`
stores nanoseconds, so adding a sub-millisecond `Duration` loses no precision. An advance visits
each link delivery and timer deadline in chronological order before reaching its requested end;
one large advance therefore has the same result as the equivalent smaller advances. Seeded loss,
duplication, latency and jitter remain the link's inputs.

## Package decision

`sipx-testkit` becomes a published workspace crate. Keeping it unpublished would make the accepted
surface unavailable to registry consumers and contradict the point of the story. Its normal
workspace dependencies are already public; development dependencies do not enter its published
graph. Release graph checks and `cargo package` remain the authority for the archive.

This Supported claim uses a distinct **test-product reachability** class. The package manifest
names one example target, `rtp_echo`; `check-app-surface.py` derives the crate from that declaration,
requires the example's non-comment Rust source to import its public library, and relies on the
gate's `cargo check --all-targets` to compile the separate target. The release rehearsal then copies
that exact archived example into a clean package-set consumer. An ordinary test, dev-dependency or
undeclared example still cannot widen production application reachability, and the test product
backs only `sipx-testkit` itself rather than the dependency closure below it.

## Output audit

The 2026-08-05 audit of library `src/` found all output at `tracing` call sites and no `println!`,
`eprintln!`, `dbg!`, subscriber installation or direct logger initialization. Binaries are output
owners and are deliberately outside this table.

| Library crate | error | warn | info | debug | trace | Purpose observed |
|---|---:|---:|---:|---:|---:|---|
| `sipx-call` | 1 | 11 | 0 | 8 | 0 | cleanup/refusal failures and dialog disposition |
| `sipx-media` | 1 | 7 | 1 | 17 | media failure, bridge lifecycle and packet disposition |
| `sipx-transport` | 2 | 18 | 0 | 42 | listener/capture failure and connection/message disposition |
| `sipx-ua` | 0 | 4 | 1 | 0 | registration lifecycle and push/refresh failure |

The canonical level policy is the public logging reference; it is not repeated in source modules.
`library_output.rs` ratchets both halves: library source cannot acquire `print`/`eprint` output,
write directly to stdout/stderr, or initialize a global/default subscriber through the common
constructor variants. Its isolated subprocess establishes a real application call through the
public harness and compares both process streams with a no-library control.

## Exit

A downstream package can place and answer a call through socket-free SIP signalling, observe the
real `Call` event streams after ACK, and separately drive transaction faults under deterministic
time; no library crate installs output globally; and the gate compiles the exact example the guide
presents.
