---
id: T-16
title: Implement the Service-Route header
pillar: Signalling
status: ready
priority: 8
design: docs/designs/sip-transport.md
epic: conformance
areas: [sipx-sip, sipx-ua]
note: track: reachability · RFC 3608 · the outbound twin of T-14's Path
---

# Implement the Service-Route header

## Goal
Let a registrar tell a UA which proxies its *outbound* requests must traverse, and let a UA obey
it — the return direction of the route set `Path` establishes inbound.

## Acceptance
- [ ] `Service-Route` is known to the parser as a route header with the same list semantics
      `Record-Route` has, not read line-at-a-time.
- [ ] A 2xx to REGISTER carrying a `Service-Route` establishes a pre-loaded route set, applied in
      order to subsequent out-of-dialog requests within that registration.
- [ ] The route set is discarded when the registration is replaced or lapses — a stale service
      route sends every call to a proxy that no longer wants it.
- [ ] The RFC registry entry for RFC 3608 moves off "not started" in the same change.
- [ ] Failing-first test: `an_out_of_dialog_invite_follows_the_registrars_service_route`.

## Progress
- Not started. `Service-Route` is absent from `HeaderName` and falls through to `Other`.

## Notes
- `T-14` does the same job for `Path`; the header-list machinery is shared, so taking them
  together is cheaper than taking them apart.
- Ledgered by [sipx-clstr](https://github.com/codewandler/sipx-clstr) alongside `Path`
  ([ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md)); its location
  service names both as typed-header gaps.
