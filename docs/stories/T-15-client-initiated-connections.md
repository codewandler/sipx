---
id: T-15
title: Implement Outbound, for client-initiated connections
pillar: Signalling
status: ready
priority: 2
design:
epic: conformance
areas: [sipx-transport, sipx-ua]
note: M6 · RFC 5626 · T-14 unblocked it
---

# Implement Outbound, for client-initiated connections

## Goal
Registrations that survive NAT: a flow the client opened, identified well enough that a request
can be routed back down it.

## Acceptance
- [ ] `reg-id` and `+sip.instance` on REGISTER, and the `ob` parameter on the contact.
- [ ] `outbound` offered in `Supported` and required correctly when the registrar demands it.
- [ ] Several flows registered at once for one instance, so one failing does not unregister the
      user.
- [ ] Keep-alives on the flow, with the failure of one flow not disturbing the others.
- [ ] Failing-first test: `a_second_flow_survives_the_first_being_cut`.

## Progress
- Not started.

## Notes
- Blocked by `T-14`: Outbound's flow token is carried in `Path`.
