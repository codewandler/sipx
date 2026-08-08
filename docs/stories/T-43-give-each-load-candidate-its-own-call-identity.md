---
id: T-43
title: Give each load candidate its own call identity
pillar: Transport
status: backlog
priority: 3
design: docs/designs/endpoint-resolution.md
epic: endpoint-resolution
areas: [sipx-cli]
predicate:
announcement:
note: load's signalling workload reuses one Call-ID and From tag across every address of a target, so a second candidate arrives at the same server as a merged request
---

# Give each load candidate its own call identity

## Goal

Present each address of a `load` target with a request the far end can accept, so a name whose
first address is dead is reachable at the second even when both addresses lead to the same server.

## Acceptance

- [ ] `load --mode signalling` builds a fresh `Call-ID` and `From` tag for each candidate its
      serial pass attempts, as `peers` does for each subscription attempt.
- [ ] A failing-first test proves it: a target whose addresses reach one server, whose first
      address refuses, is connected rather than answered `482 Loop Detected`.
- [ ] The run's reproducibility is unchanged — the identity stays derived from `--seed` and the
      call index, with the candidate position as the only new input.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `T-42`'s adjacent findings. `SignallingIdentity::new` is called once per
  admitted call, outside the candidate pass, and every candidate reuses its `call_id` and its
  `From` tag; only the Via branch differs. Two addresses of one name commonly resolve to the same
  server, and RFC 3261 §8.2.2.2 makes the second INVITE a *merged request* there — same Call-ID,
  same From tag, same CSeq, different branch — which a compliant server answers `482 Loop
  Detected`. `load` then classifies that as a rejection and stops the pass, so the fallback the
  pass exists for does not happen.

  `peers` states the rule this should follow, in `crates/sipx-cli/src/peers.rs`: *"A fresh dialog
  identity per candidate. A subscription attempted at another address is a new usage (RFC 6665
  §4.1.2.1), and reusing the Call-ID and tag of the attempt that just failed would present it to
  the registrar as the first one arriving twice."* The same sentence is true of an INVITE.

  Not fixed in `T-42` because that story's contract was the pass and its reporting, and changing
  what goes on the wire for every load call is a separate, separately testable decision.
