---
id: endpoint-resolution
---

# Bounded endpoint resolution

**Status:** proposed · **Pillar:** Transport · **Epic:** `endpoint-resolution` ·
**Review:** [external functionality and usability review](../reviews/extern-2026-08-06T01-18-47+02-00-full-sweep.md)
finding 2 · **Stories:** `T-38`, `T-39`

## Problem

The phone accepts SIP URIs and addresses of record, yet outbound commands require the host to be a
literal IP address or require the caller to resolve it externally and inject `--target`. That makes
ordinary named PBX and registrar operation unreachable and separates the address dialled from the
name that secure transport must verify.

## Direction

Resolution follows RFC 3263 and is specified before an I/O adapter is added. The policy takes a URI
scheme, host, optional explicit port and optional transport selection, and returns an ordered,
bounded set of connection attempts while retaining the original service identity. Explicit IP
addresses remain a no-lookup fast path; an explicit port and transport do not silently enter a
different discovery policy.

DNS I/O belongs in `sipx-transport`. Selection and ordering are pure functions over resolver
answers so the RFC branches, empty answers, mixed address families and secure/no-downgrade cases
are deterministic tests. Lookup deadlines, answer counts, attempt counts and cache lifetimes are
finite. Cancellation ends outstanding lookups and connection attempts without a detached task.

SIPS and TLS identity are not rewritten to the selected address. RFC 5922 service identity remains
the name the user supplied unless an explicit, separately validated server name overrides it.
Failure reports distinguish no usable DNS answer, resolution timeout and connection failure.

## Delivery

`T-38` writes `docs/specs/sip-target-resolution.md` with state and byte/value vectors. `T-39` adds
the resolver adapter and routes every outbound diagnostic-phone path through the same policy. The
split is deliberate: DNS and SIP server-location rules are non-trivial I/O policy and cannot be
invented independently in each command.

## Exit

Named `dial`, `register`, `load`, registrar-backed `peers` and scenario calls work without manual
address injection; literal targets remain byte-for-byte compatible; secure targets never
downgrade; and all lookup/attempt work is bounded and cancellation-safe.
