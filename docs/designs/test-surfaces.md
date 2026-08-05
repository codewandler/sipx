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

`sipx-testkit::call::CallHarness` is the downstream surface. It owns two real
`sipx-sip::transaction::TransactionLayer` values and joins them with
`sipx-testkit::link::Link<Virtual>`. A caller supplies a complete request, observes the invitation,
answers it with either the convenience `200 OK` or an application-built response, and observes the
answer. `advance(Duration)` is the only way time moves. There is no runtime, socket or clock read in
that path.

The harness ends at answered INVITE signalling. It does not pretend to provide a media endpoint or
to replace network interoperability tests. That narrow boundary is intentional: it exposes the
already-tested transaction path without making internal `sipx-call` media machinery public merely
for tests.

`Link` is generic over its instant with the existing Tokio instant as its default, preserving its
current callers while allowing the harness to use a zero-based virtual instant. Seeded loss,
duplication, latency and jitter remain the link's inputs.

## Package decision

`sipx-testkit` becomes a published workspace crate. Keeping it unpublished would make the accepted
surface unavailable to registry consumers and contradict the point of the story. Its normal
workspace dependencies are already public; development dependencies do not enter its published
graph. Release graph checks and `cargo package` remain the authority for the archive.

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
`library_output.rs` ratchets both halves: library source cannot acquire a direct output/global
subscriber site, and constructing the public harness leaves the process without a global
subscriber.

## Exit

A downstream package can place and answer a socket-free call under deterministic time, inspect the
result, and follow a public runnable example; no library crate installs output globally; and the gate
compiles the exact example the guide presents.
