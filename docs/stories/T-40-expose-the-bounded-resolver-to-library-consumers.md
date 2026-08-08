---
id: T-40
title: Expose the bounded resolver to library consumers
pillar: Transport
status: ready
priority: 24
design: docs/designs/endpoint-resolution.md
epic: library-parity
areas: [sipx-transport, sipx-ua, docs]
predicate:
announcement:
note: T-38/T-39 built bounded resolution inside sipx-cli · the public guide still tells applications to resolve the proxy themselves
---

# Expose the bounded resolver to library consumers

## Goal

Let an application reach the same bounded target resolution the diagnostic phone uses, instead of
being told to resolve names itself and hand in an address.

## Acceptance

- [ ] The resolver `T-38` specified is reachable from the library, not only from `sipx-cli`, with
      the same deadlines, ordering, identity rules and typed failures.
- [ ] `sipx_ua::Config` accepts a named target, and the original hostname remains the TLS/WSS
      verification identity exactly as the CLI path guarantees.
- [ ] `website/docs/guides/integrate-existing-system.md` no longer instructs applications to
      "Resolve the outermost proxy in the application and pass that address as the `Target`", and a
      failing-first public-content regression prevents that instruction returning.
- [ ] A failing-first test proves a library consumer resolves and connects through a named target,
      and distinguishes resolution failure, resolution timeout and connection failure.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `T-39`'s adjacent findings. `destination::Resolver` lives in `sipx-cli` and
  `sipx_ua::Config` takes an already-resolved `Target`, so every capability `T-38` and `T-39`
  delivered stops at the CLI boundary.

## Notes

- First story of the `library-parity` epic: things the diagnostic phone can do that a library
  consumer cannot. `C-6` is the other known instance.
- `T-39` also left the spec's `ConnectionFailed { attempted, last_error }` promise unfulfilled at
  the surface — `T-41` owns that.
