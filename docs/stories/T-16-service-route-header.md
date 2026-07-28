---
id: T-16
title: Implement the Service-Route header
pillar: Signalling
status: done
priority:
design: docs/designs/sip-transport.md
epic: conformance
areas: [sipx-sip, sipx-ua]
note: M6 · RFC 3608 · the outbound twin of T-14's Path
---

# Implement the Service-Route header

## Goal
Let a registrar tell a UA which proxies its *outbound* requests must traverse, and let a UA obey
it — the return direction of the route set `Path` establishes inbound.

## Acceptance
- [x] `Service-Route` is known to the parser as a route header with the same list semantics
      `Record-Route` has, not read line-at-a-time.
- [x] A 2xx to REGISTER carrying a `Service-Route` establishes a pre-loaded route set, applied in
      order to subsequent out-of-dialog requests within that registration.
- [x] The route set is discarded when the registration is replaced or lapses — a stale service
      route sends every call to a proxy that no longer wants it.
- [x] The RFC registry entry for RFC 3608 moves off "not started" in the same change.
- [x] Failing-first test: `an_out_of_dialog_invite_follows_the_registrars_service_route`.

## Progress
- Done. `Service-Route` is a typed route header with the same list semantics `Path` and
  `Record-Route` have, and the registration stores what came back.
- **Absent means clear, not "keep what you had".** RFC 3608 §6.1 has two sentences that are
  really one rule: the stored value is updated from "the latest 200 class response", and a
  response with no `Service-Route` "clears any service route ... previously stored". So
  `from_response` returns an empty set rather than an `Option`, and the agent assigns
  unconditionally. This was the mutation that survived the first time: a unit test on
  `interpret` proved the parse was empty without proving the *agent* replaced its stored value,
  so `if !service_route.is_empty()` passed the whole suite. The test that closes it registers
  twice against a registrar that stops dictating a route.
- **The route set is not attached behind the caller's back.** §6.1 says a UA "MAY choose to
  exercise" the route, and a `Route` header silently added to every request is close to
  undebuggable from outside. `UserAgent::service_route()` hands it over and
  `DialOptions::with_service_route` takes it, so the plumbing is visible at the call site.
- `Outcome::Registered` became a struct. `PathSet` and `ServiceRoute` are the same shape and
  opposite directions — one routes requests *toward* this UA and is not ours to follow, the other
  routes the requests we send — and two positionally interchangeable fields of identical type is
  how they would eventually get swapped.
- **A hop without `;lr` is reported, not dropped.** §5 requires it; a hop lacking it is asking
  for RFC 2543 strict routing, which sipx does not implement. The registrar is the offending
  party, the request still reaches the proxy named, and a UA that discarded a whole route set
  over a missing parameter would be unroutable for an invisible reason. `lr` is a *URI*
  parameter, not a header parameter — looking in the wrong list is how the check was wrong
  first time.
- Mutation-tested: reversing the order, reading the value out of `Path` instead, keeping a stale
  route across a refresh, and dropping `ServiceRoute` from the comma-list predicate each fail the
  test that names the behaviour.

## Notes
- `T-14` does the same job for `Path`; the header-list machinery is shared, so taking them
  together is cheaper than taking them apart.
- Ledgered by [sipx-clstr](https://github.com/codewandler/sipx-clstr) alongside `Path`
  ([ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md)); its location
  service names both as typed-header gaps.
