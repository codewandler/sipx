---
id: M-21
title: The sans-IO ICE agent
pillar: Media
status: done
priority:
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
- [x] The §5.1.2.1 formula exactly, asserted against the spec's three-row table, including the
      `1694498815` RFC 8839 prints in its own example.
- [x] `PRIORITY` in a check uses the **peer-reflexive** type preference (§7.1.1), not the candidate's
      own — otherwise the peer prioritises the peer-reflexive candidate it learns differently from us.
- [x] Foundations per §5.1.1.3; pairing per §6.1.2.2 including the link-local rule; pair priority per
      §6.1.2.3; pruning per §6.1.2.4; the configurable 100-pair limit per §6.1.2.5.
- [x] Initial pair states per §6.1.2.6, asserted against the RFC's own three-checklist,
      five-foundation worked example.
- [x] Role conflict per §7.3.1.1: all seven rows of the spec's §7.3 table including the `T = V` row,
      and on receiving a 487 the agent switches role, **changes its tiebreaker** (§7.2.5.1),
      recomputes every pair priority and re-runs the check as a triggered one.
- [x] **Regular nomination only** (§8.1.1). Aggressive nomination is absent and there is no option to
      enable it; the controlled side still tolerates a peer that nominates twice by selecting the
      highest-priority nominated pair.
- [x] Peer-reflexive candidates learned in both directions (§7.2.5.3.1, §7.3.1.3); triggered checks
      (§7.3.1.4) preempt the checklist; a non-symmetric response fails the pair (§7.2.5.2.1).
- [x] Ta, RTO, Rc and Rm per §14, configurable, no literals in the machine, RTO recomputed per
      transaction because it depends on how many checks are outstanding.
- [x] Sans-IO: no `tokio`, no clock read, no socket. Time arrives as `TimerFired`.
- [x] Failing-first test: `two_agents_that_both_start_controlling_converge_on_one_role`.

## Progress
- Implemented in four modules under `crates/sipx-media/src/ice/`, all of them sans-IO:
  - `candidate` — §5.1.2.1's formula and §5.1.2.2's four type preferences, §5.1.1.3's foundations,
    §5.1.2.1's local preferences assigned over addresses sorted by bytes, §7.1.1's check priority
    and §6.1.2.3's pair priority. Spec §4's three-row table is asserted as three stated integers.
  - `checklist` — §6.1.2.2 pairing (component, address family, the link-local MUST NOT, the
    component reduction), §6.1.2.4 pruning, §6.1.2.5's configurable limit, §6.1.2.6's initial
    states against RFC 8445's own Table 1, and both of §6's unfreeze triggers.
  - `timing` — §14's Ta, its 5 ms floor, the RTO as a function of the outstanding checks, RFC 5389
    §7.2.1's Rc and Rm, and spec §9's Tr and Tn. Nothing is a literal in the machine.
  - `agent` — spec §2's `Input`/`Output`, §6.1.4.2's pacing, §7.2/§7.3's client and server
    procedures, §7.3.1.1 and §7.2.5.1's role conflict, §8.1.1's regular nomination under spec §8's
    stopping criterion, §8.1.2's conclusion and §11's keepalives.
- **Spec correction.** `docs/specs/ice.md` §6.5 had one Frozen row, naming only §7.2.5.3.3's
  unfreeze. §6.1.4.2 step 2 is a second one, and without it a foundation whose single unfrozen pair
  fails stays Frozen for the session — ICE fails a path it never checked. The row was added with a
  dated attribution paragraph, the third such correction after `M-19`'s to §6.2 and `M-20`'s to
  §11.1.
- **Scope taken and not taken.** The agent is one data stream, which is one checklist, because a
  sipx media session is one; the checklist *set* is still a set, so §6.1.2.6's rule (which is
  stated over the set) is implemented and tested as written rather than collapsed. The socket
  driver, reflexive gathering, and the SDP the agent's candidates turn into are not here — the
  first two are the epic's remaining stories and the third is `sipx-sdp`, which `M-19` did.
- **A reading recorded.** §8.1.2 makes an agent prune every other pair for a nominated component;
  §8.1.1 makes it tolerate a peer that nominates more than once and then select the
  highest-priority nominated pair. Both can be satisfied by pruning and *declining* the second
  nomination — §8.1.1's tolerance is conditional on the controlled agent accepting it — but this
  story's Acceptance requires the tolerance, so only the controlling side prunes (it is the side
  that knows there will be no second nomination) and a Completed checklist still serves its
  triggered-check queue.
- **Review round 1 (three blocking findings, all fixed, each with a regression test that fails
  without its fix).**
  - §6.1.2.5's limit was applied only at checklist formation, so §7.3.1.4's insertion path grew
    the set without bound — 201 pairs against a configured limit of 4 under 200 authenticated
    checks from distinct source addresses, each buying the sender a check sent to an address it
    named. The limit now binds every time the set grows, discarding only pairs that are not
    In-Progress, Succeeded or nominated, and the remote candidate table is pruned with it.
    (`a_flood_of_checks_from_new_addresses_cannot_grow_the_set_past_the_limit`)
  - Nothing re-armed Ta after a checklist completed, so a §7.3.1.4 check enqueued afterwards was
    queued for a tick a driver would never deliver — which also made §8.1.1's tolerance clause
    inoperative. Arming is now tracked and every path that creates work arms it; the tests drive Ta
    from the agent's own outputs rather than firing it by hand.
    (`a_check_arriving_after_the_checklist_concluded_still_arms_ta`)
  - A second `RemoteDescription` replaced the remote candidate table under live pairs, which
    re-pointed one pair and dangled the rest and left the agent permanently silent — reachable from
    a 183-then-200 or any re-INVITE. `LocalId`/`RemoteId` are now allocated identities rather than
    positions, as `PairId` already was; a second description for the same ICE session merges
    additively, and only a change of **both** ufrag and pwd (RFC 8839 §4.4.1.1.1) rebuilds.
    (`a_second_description_adds_candidates_without_re_pointing_the_live_pairs`,
    `an_ice_restart_rebuilds_the_checklists_and_keeps_checking`)
  - Found while fixing the second: two concluded agents re-triggered each other forever, because a
    queued check of one's own moves a Succeeded pair to In-Progress and §7.3.1.4 then
    cancel-and-re-enqueues it at the far end. §8.1.2's "an agent will no longer generate triggered
    checks" now extends to a nominated pair in any state, and `two_agents_converge_on_a_selected_pair`
    asserts both ends fall quiet.
  - Smaller: §8.1.2's cancellation of In-Progress transactions is now performed when the
    controlling agent prunes (unobservable with one component, a live retransmission loop with
    two); the worst-case RTO is recorded where the stopping criterion is defined — 500 s at the
    default 100-pair limit, so `Tn` and not the "every higher-priority pair Failed" half is the
    criterion for any set of interesting size; the sans-IO guard scans whole files and names the
    words it forbids in fragments; and `Agent::new`'s doc now says plainly that the *initial*
    tiebreaker is the caller's and the redraw after a 487 is drawn from the process RNG, because
    two ends of a symmetric conflict deriving it the same way stay equal and oscillate.

## Notes
- The spec is [`docs/specs/ice.md`](../specs/ice.md), written by `M-16` before any code. Read the
  sections its Acceptance names rather than re-deriving them from the RFCs.
- `M-16` is the tracker for this epic and stays open until every child is done.
