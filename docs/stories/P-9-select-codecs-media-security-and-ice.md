---
id: P-9
title: Select codecs, media security and ICE from the diagnostic phone
pillar: Phone
status: backlog
priority: 7
design: docs/designs/phone.md
epic: phone
areas: [sipx-cli, sipx-call, sipx-media]
note: M-27 and M-28 delivered the call-level policy; the CLI must consume it, not rebuild negotiation
---

# Select codecs, media security and ICE from the diagnostic phone

## Goal

Expose the call framework's codec, keying and ICE choices through one coherent CLI policy.

## Acceptance

- [ ] The closed values and defaults in `diagnostic-phone.md` §2 map directly to the public
      call-level policy delivered by `M-27` and `M-28`.
- [ ] Unsupported codecs or unsafe security combinations fail before network I/O; explicit modes do
      not silently fall back.
- [ ] The terminal result reports what actually negotiated, not only what was requested.
- [ ] `DPH-3` through `DPH-6` fail first and pass through real calls.
- [ ] RFC 5763/5764, 8445 and 8839 registry claims are updated in the same change where reachability
      changes.

## Progress

- Not started. M-27 and M-28 have delivered the call-level selection this story consumes.
