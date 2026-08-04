---
id: P-9
title: Select codecs, media security and ICE from the diagnostic phone
pillar: Phone
status: done
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

- [x] The closed values and defaults in `diagnostic-phone.md` §2 map directly to the public
      call-level policy delivered by `M-27` and `M-28`.
- [x] Unsupported codecs or unsafe security combinations fail before network I/O; explicit modes do
      not silently fall back.
- [x] The terminal result reports what actually negotiated, not only what was requested.
- [x] `DPH-3` through `DPH-6` fail first and pass through real calls.
- [x] RFC 5763/5764, 8445 and 8839 registry claims are updated in the same change where reachability
      changes.

## Progress

Done. `dial` and `answer` validate the closed codec, security and ICE values before binding and map
them directly to an expanded exact-order `MediaPolicy`; capability construction and negotiation
remain in `sipx-call`. Established calls now expose their resolved keying mode, while media sessions
report the candidate kinds of the pair the existing ICE agent selected.

The four vectors run through the built command process. `DPH-3` and `DPH-4` prove typed setup
refusals leave a bound observer's datagram queue empty. `DPH-5` proves both the no-feature refusal
and, with all features, DTLS-SRTP audio between two processes. `DPH-6` silences both default host
paths behind finite test mappings and proves PCMA audio crosses a nominated server-reflexive pair;
both terminal reports name that actual path. A protected-signalling process test separately proves
strict plain and SDES calls resolve to different established-call modes.

The default and all-feature strict clippy passes are green for `sipx-call` and `sipx-cli`; the full
all-feature call suite, full default CLI suite, all-feature media suite, RFC report, fixed-sleep and
provenance checks are green. The workspace gate is run once for the merged wave.
