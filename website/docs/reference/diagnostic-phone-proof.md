---
title: Diagnostic-phone proof
description: The executable release matrix used to decide whether the sipx command meets the beta product threshold.
---

# Diagnostic-phone proof

The CLI is not treated as complete because its lower layers have the necessary types. Its exit
criterion is a bounded process-level proof: twelve scenarios drive the shipped `sipx` binary, and
the signalling claim is checked separately against two independent peer profiles. A specified row
without an executable test remains open.

Run the structural audit without opening a socket:

```bash
./scripts/diagnostic-phone-proof.py --check
```

Run every local process vector, each with a finite failure bound:

```bash
./scripts/diagnostic-phone-proof.py --run
```

Add the container-backed independent-peer runs for the complete release proof:

```bash
./scripts/diagnostic-phone-proof.py --run --interop
```

The command prints one matrix with the requested and observed path for `DPH-1` through `DPH-12`,
then a transport-by-transport peer matrix. Missing tests, failed commands, timeouts, and transports
with fewer than two peer paths all make it exit non-zero. Its timeout bounds only a failure; the
tests themselves wait for readiness, signalling, media, or process-exit events instead of sleeping
and assuming the event happened.

## Current state on `main`

The following is the structural state, not a claim that the tests passed on the reader's machine:

| Evidence | Present | Open |
|---|---|---|
| Diagnostic-phone vectors | `DPH-1`–`DPH-12`; latest bounded run passed every vector | — |
| Two-peer signalling paths | UDP, TCP, TLS, WebSocket, secure WebSocket; both peer profiles passed the complete shared list | — |
| Public CLI contract | Seven executable command helps and three versioned JSON contracts agree with the public reference | — |

The existing vectors cover all five loopback signalling transports; strict codec and media-security
selection; a server-reflexive ICE path; deterministic device audio; bounded load; and interruption
cleanup. That is different from independent interoperability. WSS therefore has its own registration
test in the shared peer list, with each profile declaring its actual HTTPS port and resource; the
proof does not infer WSS from separate TLS and WebSocket successes.

The public-reference row is executable too: `./scripts/check-cli-reference.py --check` builds the
default command, runs root and all seven subcommand help paths, and compares them with the CLI page.
It also discovers the three versioned JSON schemas or envelopes beside their Rust producers and
holds every literal structural field against the checked public table. The diagnostic proof runs
that check as a product path rather than accepting hand-reviewed prose.

Positive Opus selection and reliable early media now have their own command-process cases in the
same product-path table. The structural audit confirms those cases are wired into the proof; the
bounded `--run` invocation decides whether they pass on a candidate. Lower-layer existence is useful
engineering evidence, but it is not substituted for shell-level product evidence. The early-media
fixture sends its final answer only after the provisional announcement has completed, so
`early_media` and `early_samples_recorded` report audio that causally preceded confirmation rather
than audio inferred from a timer.

This matrix is a release threshold, not a coverage percentage. See the
[development process](development-process.md) for why sipx uses executable predicates, and the
[CLI reference](cli.md) for the command contract being exercised.
