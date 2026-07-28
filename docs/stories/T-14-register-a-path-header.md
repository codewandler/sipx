---
id: T-14
title: Implement the Path header
pillar: Signalling
status: ready
priority: 5
design:
epic: conformance
areas: [sipx-sip, sipx-ua]
note: RFC 3327; gates Outbound and GRUU
---

# Implement the Path header

## Goal
Let a registration record the proxies that must be traversed to reach the registering UA, which
is the prerequisite for routing anything back to it from behind a NAT.

## Acceptance
- [ ] `Path` is known to the parser as a route header, with the list semantics `Record-Route`
      already has — not read line-at-a-time.
- [ ] A REGISTER offers `path` in `Supported`, and the returned path set is stored with the
      binding.
- [ ] The path set is used, in order, when sending toward the registered contact.
- [ ] A registrar that returns a path when it was not offered is handled rather than ignored.
- [ ] Failing-first test: `a_registration_preserves_the_path_it_was_returned`.

## Progress
- Not started. The `Path` header is not currently known to the parser at all, which
  `compliance.md` records as RFC 3327 "not started" rather than "syntax only".

## Notes
- Gates `T-15` (Outbound) and the GRUU work after it.
