---
id: P-8
title: Select every released signalling transport from the diagnostic phone
pillar: Phone
status: ready
priority: 6
design: docs/designs/phone.md
epic: phone
areas: [sipx-cli, sipx-transport]
note: operational capability baseline · secure transports exist in the library and are unreachable from the CLI
---

# Select every released signalling transport from the diagnostic phone

## Goal

Let `dial`, `answer` and `register` select UDP, TCP, TLS, WS or WSS without bypassing the existing
transport and certificate policies.

## Acceptance

- [ ] The flags and fail-closed combinations in `diagnostic-phone.md` §§2–3 are implemented.
- [ ] Requested TLS/WSS certificate identity and trust roots reach the existing TLS policy; secure
      URIs never downgrade and no insecure-verification shortcut is added.
- [ ] Existing invocations remain byte-for-byte compatible at their current defaults.
- [ ] JSON and text results name both requested and negotiated transport.
- [ ] `DPH-1` and `DPH-2` are failing-first real-socket tests; all five transports also have a
      bounded loopback command test.

## Progress

- Not started; specification filed first.

## Notes

- This is driver/application reachability. Transport implementation remains in `sipx-transport`.
