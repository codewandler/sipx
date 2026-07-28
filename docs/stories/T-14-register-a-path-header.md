---
id: T-14
title: Implement the Path header
pillar: Signalling
status: done
priority: 4
design:
epic: conformance
areas: [sipx-sip, sipx-ua]
note: track: reachability · RFC 3327 · gates T-15 and GRUU
---

# Implement the Path header

## Goal
Let a registration record the proxies that must be traversed to reach the registering UA, which
is the prerequisite for routing anything back to it from behind a NAT.

## Acceptance
- [x] `Path` is known to the parser as a route header, with the list semantics `Record-Route`
      already has — not read line-at-a-time.
- [x] A REGISTER offers `path` in `Supported`, and the returned path set is stored with the
      binding.
- [~] The path set is used, in order, when sending toward the registered contact. **Recast —
      see Progress.** RFC 3327 §5.1 says a UA does *not* route on the returned path; it is
      stored, ordered and inspectable instead.
- [x] A registrar that returns a path when it was not offered is handled rather than ignored.
- [x] Failing-first test: `a_registration_preserves_the_path_it_was_returned`.

## Progress
- Done, with **one acceptance criterion recast**, which is worth reading before the rest.
- The story said "the path set is used, in order, when sending toward the registered contact".
  RFC 3327 §5.1 says the opposite for a UA: *"the general operation of the UA is to ignore the
  Path header field in the response."* The path vector exists so that requests arriving **at the
  registrar** can be steered back toward a UA behind a NAT — §5.3 makes walking it the
  registrar's job. A UA that turned it into a pre-loaded route set would push its own outbound
  requests through proxies that never agreed to carry them. The header that *does* do that is
  `Service-Route` (RFC 3608), which is a different list with different semantics and would be
  its own story.
- So the path set is parsed, ordered, kept with the binding and exposed — `UserAgent::path()`
  and `PathSet::hops_outside`. §5.1 gives the reason a UA should see it at all: *"such inspection
  might allow the UA to detect intermediate proxies that have inappropriately added
  themselves."* That is only possible if the value survives.
- Kept as `Address`, not as a URI. RFC 5626 §5.3 hangs the `ob` marker off a `Path` value, and
  `T-15` has to read it; a vector flattened to bare URIs would parse cleanly and be useless.
- `Supported: path` goes on every REGISTER, because §5.2 tells intermediate proxies **not** to
  add themselves unless the UA has indicated support — a UA that stays quiet is unreachable from
  behind exactly the proxies the mechanism exists to traverse.
- Mutation-tested, and one mutation was *not* caught at first: removing `Path` from
  `is_comma_separated_list` changed nothing, because the address-list decoder splits on commas
  itself and no caller consults that predicate. The comma-joined test was passing for a reason
  other than the one it named. The predicate is public API, so it now has its own assertion
  rather than being left as an untested claim about the grammar.

## Notes
- Gates `T-15` (Outbound) and the GRUU work after it.
