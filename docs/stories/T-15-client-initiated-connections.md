---
id: T-15
title: Implement Outbound, for client-initiated connections
pillar: Signalling
status: done
priority:
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
- [x] `reg-id` and `+sip.instance` on REGISTER, and the `ob` parameter on the contact.
- [x] `outbound` offered in `Supported` and required correctly when the registrar demands it.
- [x] Several flows registered at once for one instance, so one failing does not unregister the
      user.
- [x] Keep-alives on the flow, with the failure of one flow not disturbing the others.
- [x] Failing-first test: `a_second_flow_survives_the_first_being_cut`.

## Progress
- Done for the **UA side**, which is the whole of what a client can do on its own. The registrar
  side — minting a flow token and putting it in `Path` (§5) — is what makes the mechanism useful to
  the *network*, and sipx is not a registrar. RFC 5626 is recorded as `partial` for exactly that
  reason, with the note saying which half.
- Split the usual way: `sipx-ua/src/outbound.rs` decides (instance IDs, `reg-id`, `ob`, whether the
  registrar accepted, which keep-alive a transport needs, how long to wait, how long to back off)
  and takes the random fraction as an argument so a test can pin it. `sipx-ua/src/flows.rs` is the
  set that has a clock and several sockets.
- **`Flows::register` returns one outcome per flow and no aggregate `Result`.** That is the story's
  criterion expressed in a type rather than in a comment: §4.2 registers to each outbound proxy
  precisely so one going away is survivable, and a function returning a single `Result` cannot help
  but let one failure stand for all of them. Same for `Flows::keepalive`.
- **§4.4.2's rebinding rule is the reason STUN is the UDP keep-alive.** A changed
  `XOR-MAPPED-ADDRESS` means the flow has failed even though every ping was answered: the socket
  works, but the mapping the registrar holds no longer reaches this UA, so a call routed down the
  flow would silently never arrive. Both mutation tests that survived the first pass were about
  this — the rule itself, and matching a reply to *whichever* keep-alive was outstanding rather than
  to its own transaction ID, which would let a forged response tear down a working flow.
- **The keep-alive needed the parser to count what RFC 3261 §7.5 tells it to ignore.** §4.4.1's
  ping and pong are CRLFs between messages; the parser goes on discarding them and now counts them
  on the way past (`StreamParser::take_keepalives`), so a transport waiting for a pong can tell one
  arrived. Counting pairs, not pings: which it was depends on who sent it, and the parser has no
  business deciding.
- **The STUN client is scoped to a keep-alive and says so.** RFC 5389's Binding request with no
  attributes, and a response read for one attribute — verified against RFC 5769's published
  vectors, including the 11-byte `SOFTWARE` attribute whose padding a decoder must skip to reach
  `XOR-MAPPED-ADDRESS` at all. No `MESSAGE-INTEGRITY`, no `FINGERPRINT`, no server role. ICE needs
  the real protocol, not more attributes bolted onto this.
- `Config::keepalive_timeout` is configurable with §4.4.1's ten seconds as the default, because the
  RFC gives two failure rules and only one of them is a duration — §4.4.2 bounds the STUN case by 7
  retransmissions of an RTO estimate instead.
- Mutation-tested, eight ways: stopping `register` or `keepalive` at the first failure, giving every
  flow `reg-id=1`, assuming acceptance instead of reading `Require`, dropping `ob`, not counting the
  discarded CRLFs, matching a STUN reply by anything but its transaction ID, and ignoring a changed
  reflexive address. Each fails the test that names the behaviour.

## Notes
- Blocked by `T-14`: Outbound's flow token is carried in `Path`.
- `PathSet` keeps `Address` rather than bare URIs so the `ob` parameter §5.3 hangs off a `Path`
  value survives — which is what a registrar implementation would read.
