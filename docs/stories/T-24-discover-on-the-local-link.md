---
id: T-24
title: Discover SIP endpoints on the local link
pillar: Transport
status: blocked
priority:
design: docs/designs/discovery.md
epic: discovery
areas: [sipx-transport, sipx-cli]
note: blocked on a scope decision — mDNS is a second protocol and a new parser eating unauthenticated multicast
---

# Discover SIP endpoints on the local link

## Goal
Find SIP endpoints on the local network with no infrastructure at all — the source that makes
`sipx peers` interesting on a laptop that is registered nowhere.

## Acceptance
- [ ] **The scope decision is made and recorded first** (see Notes). If the answer is no, this
      story closes with the reasoning written into
      [`docs/designs/discovery.md`](../designs/discovery.md) the way `X-26` recorded G.722's, and
      the epic ships with two sources instead of three. That is a legitimate outcome.
- [ ] If yes: sipx browses `_sip._udp.local` (DNS-SD, RFC 6763, over mDNS, RFC 6762) and turns
      responses into peers the epic's list can show, labelled by source and age.
- [ ] Whether sipx also *advertises* itself is a separate decision inside this story, stated
      explicitly. Answering "who is here" and announcing "I am here" are different commitments, and
      a phone that advertises by default has made a privacy choice for its user.
- [ ] A network that blocks multicast degrades to the other sources with a stated reason, not an
      empty list.
- [ ] Spec before code, per AGENTS.md non-negotiable 4: a new protocol parser gets a
      `docs/specs/` entry with its RFC citations and byte-level vectors before it is implemented.
- [ ] Failing-first test: named when the story is unblocked, since it depends on the decision.

## Progress
- Not started, and deliberately `blocked` rather than `backlog`: the scope question below is a real
  fork, not a detail to settle while implementing.

## Notes
- Fourth story of the `discovery` epic; see [the design](../designs/discovery.md), where this is
  recorded as the epic's one genuine scope decision.
- **The decision to make before this becomes `ready`.** mDNS is a second protocol with its own
  parser eating **unauthenticated multicast input from anyone on the link**, and it likely means a
  new dependency — against a vision that prizes "a smaller stack whose every path is tested" and a
  non-negotiable that no network input may panic. The case for: it is the only source that needs no
  infrastructure. The case against: substantial new attack surface for a convenience.
- If it goes ahead, the parser is in the same risk class as `M-20`'s STUN codec and `M-22`'s ICE
  driver — unauthenticated datagrams off a live socket — and should expect the same standard of
  review, including hostile-input probing.
- **Not** a subnet port scan. That was considered and rejected in the design: indistinguishable
  from reconnaissance, wrong on any network with more than one broadcast domain, and mDNS is the
  answer the standards already give.
