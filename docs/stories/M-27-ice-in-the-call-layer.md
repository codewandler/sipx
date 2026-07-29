---
id: M-27
title: Offer and answer ICE from a call
pillar: Media
status: ready
priority: 3
design: docs/specs/ice.md
epic: ice
areas: [sipx-call, sipx-media]
note: found by M-22 — ICE works and is reachable only through sipx-media's API; no call places one with it
---

# Offer and answer ICE from a call

## Goal
Let a call actually use ICE. `M-19` through `M-22` built gathering, the agent, the STUN codec and
the driver, and nothing in `sipx-call` offers or answers a candidate — so the NAT traversal the
epic delivered is unreachable from the layer applications program against.

## Acceptance
- [ ] `sipx-call`'s offer/answer path gathers candidates and puts them in the offer, and answers an
      offer that carries them, using `MediaPort::gather` and `sipx_media::ice::negotiate` as they
      exist. This story wires what `M-22` built; it does not re-decide it.
- [ ] Whether a call uses ICE is the application's choice, with a stated default. A stack that
      required ICE would regress every peer that does not speak it; one that never offers it leaves
      `M-19`…`M-22` dead code.
- [ ] **The no-ICE path stays byte-identical in behaviour** — nothing offered, no checks, no timers,
      symmetric RTP. `M-22`'s regression proof was that the existing media suites pass unchanged;
      hold this story to the same standard at the call layer.
- [ ] A STUN server is configuration, not a constant, and its absence degrades to host candidates
      rather than failing the call.
- [ ] The RFC 8445 and 8839 registry notes are updated to say a call can now do this — they
      currently describe a capability reachable only through `sipx-media`.
- [ ] Failing-first test: a call placed between two endpoints whose host candidates cannot reach
      each other completes over a nominated pair.

## Progress
- Not started.

## Notes
- Found by `M-22`, which recorded it plainly: "`MediaPort::gather` is not wired into `sipx-call`'s
  offer/answer — ICE is reachable through `sipx-media`'s API and no call places one with ICE yet."
  That was correct scoping — `sipx-call` belonged to `S-23` in that wave — not an oversight.
- **Reads with `C-2`.** Both change what `sipx-call` puts in an offer and both touch the media
  session's lifecycle; if they run near each other, one of them is rebasing.
- The remaining ICE gaps are separate and already filed: restart is `M-23`, relayed candidates are
  `M-24`. This story is only the call-layer wiring.
- Priority 3: it is what turns four merged stories into a feature a user can reach, which makes it
  worth more than its position in the ICE epic suggests.
