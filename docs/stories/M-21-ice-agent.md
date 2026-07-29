---
id: M-21
title: The sans-IO ICE agent
pillar: Media
status: in-progress
priority: 7
design: docs/designs/media.md
epic: ice
areas: [sipx-media]
note: ice · RFC 8445 · after M-20 · the state machine, no socket and no clock
---

# The sans-IO ICE agent

## Goal
The sans-IO state machine — gather, prioritise, pair, order, check, resolve role conflict,
nominate — as a pure function of events, so the socket work is a driver over it.

## Acceptance
- [ ] The §5.1.2.1 formula exactly, asserted against the spec's three-row table, including the
      `1694498815` RFC 8839 prints in its own example.
- [ ] `PRIORITY` in a check uses the **peer-reflexive** type preference (§7.1.1), not the candidate's
      own — otherwise the peer prioritises the peer-reflexive candidate it learns differently from us.
- [ ] Foundations per §5.1.1.3; pairing per §6.1.2.2 including the link-local rule; pair priority per
      §6.1.2.3; pruning per §6.1.2.4; the configurable 100-pair limit per §6.1.2.5.
- [ ] Initial pair states per §6.1.2.6, asserted against the RFC's own three-checklist,
      five-foundation worked example.
- [ ] Role conflict per §7.3.1.1: all seven rows of the spec's §7.3 table including the `T = V` row,
      and on receiving a 487 the agent switches role, **changes its tiebreaker** (§7.2.5.1),
      recomputes every pair priority and re-runs the check as a triggered one.
- [ ] **Regular nomination only** (§8.1.1). Aggressive nomination is absent and there is no option to
      enable it; the controlled side still tolerates a peer that nominates twice by selecting the
      highest-priority nominated pair.
- [ ] Peer-reflexive candidates learned in both directions (§7.2.5.3.1, §7.3.1.3); triggered checks
      (§7.3.1.4) preempt the checklist; a non-symmetric response fails the pair (§7.2.5.2.1).
- [ ] Ta, RTO, Rc and Rm per §14, configurable, no literals in the machine, RTO recomputed per
      transaction because it depends on how many checks are outstanding.
- [ ] Sans-IO: no `tokio`, no clock read, no socket. Time arrives as `TimerFired`.
- [ ] Failing-first test: `two_agents_that_both_start_controlling_converge_on_one_role`.

## Progress
- Not started. Cut from `M-16`'s proposed split; the Acceptance above is that proposal verbatim.

## Notes
- The spec is [`docs/specs/ice.md`](../specs/ice.md), written by `M-16` before any code. Read the
  sections its Acceptance names rather than re-deriving them from the RFCs.
- `M-16` is the tracker for this epic and stays open until every child is done.
