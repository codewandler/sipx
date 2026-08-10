---
id: M-124
title: Resolve cluster-internal SIP names after ordinary DNS declines
pillar: Core
status: in-progress
priority: 11
design: docs/designs/cluster-names.md
epic: media
areas: [sipx-transport, sipx-call, dns, resolution]
predicate:
announcement:
note: after M-123 · RFC 3263 runs first and unchanged; this is only what happens when it returns nothing
---

# Resolve cluster-internal SIP names after ordinary DNS declines

## Goal

Let an operator dial `sip:1000@<service>.<namespace>.svc.<suffix>` and have it work, without
teaching sipx to prefer a container platform's opinion over their own resolver's.

## Context

`docs/designs/cluster-names.md` is the design and states the whole shape. The parts that constrain
the implementation:

**Ordinary RFC 3263 resolution runs first and unchanged** — NAPTR, then SRV, then A/AAAA — and the
fallback fires only when that returns nothing *and* the name is the four-label
`<service>.<namespace>.svc.<suffix>` form. Not a preference and not an override. An operator who
arranged for that name to resolve has already answered the question, the fallback reads credentials
and would otherwise do so on every dial, and RFC 3263 selects a transport as well as an address.

**The fallback does not use DNS.** A container platform's internal zone is served from a virtual
service address, which is a rule in each node's packet filter and is held by no interface — so a
resolver outside the platform cannot reach it, and pointing one at it fails as a category error that
looks exactly like a timeout. The fallback reads the current context and namespace from kubeconfig
and asks the API server for EndpointSlices, which yield ready backing addresses.

**Resolving the name does not make the call work.** That is `M-123`'s half, and this story must not
be read as fixing it: media is negotiated, not resolved.

## Acceptance

- [x] A name that resolves in ordinary DNS never reaches the fallback — proven by a test whose fake
      resolver answers and whose fake API server fails the test if it is called at all.
- [x] A four-label cluster name with no DNS answer produces ordered candidates from a fixture
      EndpointSlice, with unready addresses excluded.
- [x] The two-label short form never triggers the fallback: it is indistinguishable from an ordinary
      short name and guessing there would hijack names that have nothing to do with any cluster.
- [x] The cluster suffix is configuration with a documented default, not a constant.
- [x] Candidates enter the existing target selection, so failover across them is the failover sipx
      already has rather than a second mechanism.
- [x] No Kubernetes client dependency in the core: the API call is one HTTPS GET against a
      documented resource.
- [x] The gate is green.

## Progress

- Filed 2026-08-10 alongside `M-123`, from the design written after diagnosing a real cluster dial.
- `sipx dial` now runs the cluster adapter only after an authoritative no-candidate result for the
  exact namespaced service form. It reads the current kubeconfig context and performs one bounded
  EndpointSlice HTTPS request, then returns ready addresses through the existing candidate walker.
- Unit vectors pin DNS-first behavior with a fail-if-called fake, suffix recognition, short-name
  exclusion, transport selection, and ready endpoint ordering. The CLI reference documents the
  default `--cluster-domain cluster.local` and the supported kubeconfig boundary.
- Independent review kept successful ordinary-DNS answers even when the fallback budget is empty,
  aligned WS/WSS ports and secure URI transport selection with the existing target model, and
  excludes EndpointSlice ports that name only a different transport.
- The application adapter accepts PEM CA bundles, rejects insecure or credential-bearing API URLs,
  disables redirects on the bearer-token request, and parses kubeconfig with the pure-Rust,
  deserialize-only `serde-saphyr` feature. Focused CLI tests and Clippy, the workspace MSRV check,
  and the Windows GNU cross-check all pass.
- The complete local acceptance gate passed all 51 steps on the rc.23 candidate on 2026-08-10.
