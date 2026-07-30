---
id: X-50
title: Decide whether M10 requires TURN, because the roadmap says both
pillar: Build
status: ready
priority: 2
epic: conformance
areas: [docs]
note: found closing M-23 — M10's "Done when" is satisfied today, while the ICE epic header calls M-24 an M10 story and M-24 is open; the milestone cannot be declared or deferred without picking one
---

# Decide whether M10 requires TURN, because the roadmap says both

## Goal
Give M10 one exit criterion, so whether it is reached is a fact to be read rather than a judgement
to be made again by whoever asks next.

## Acceptance
- [ ] **The two statements are reconciled, not both kept.** `docs/roadmap.md`'s M10 section says
      "**Done when** one of two registrations of the same address of record can be called
      individually, a push wakes a client that held no connection into an answered call, and a call
      passes audio between two endpoints that symmetric RTP alone cannot connect." The ICE epic
      further down the same file is headed "ICE — `ice` _(six stories, M10)_", and the six are
      `M-19`…`M-24`. Those are different claims about what M10 costs.
- [ ] **State which reading wins and why.** As of 2026-07-30 all three Done-when clauses hold:
      `T-20` (GRUU) and `T-21` (push) are done, and `M-27` closed the third with
      `a_call_uses_a_nominated_pair_when_both_host_candidates_are_silent`, which makes both SDP
      default destinations unusable and proves audio crosses the nominated pair. `M-24` — RFC 8656
      relayed candidates — is the only open child of `M-16`. So the Done-when reading declares M10
      today and the epic reading does not.
- [ ] **Whichever is chosen, the other text changes in the same commit.** Declaring M10 means the
      epic header stops calling `M-24` an M10 story and says where it does belong. Deferring M10
      means the Done-when sentence gains the clause that makes TURN part of it — and says what a
      relay buys that the three clauses do not already claim.
- [ ] **The third clause is examined honestly either way.** "Endpoints that symmetric RTP alone
      cannot connect" is satisfied by host and server-reflexive candidates for many NAT pairs and by
      neither for symmetric-NAT-on-both-sides, which is what a relay is for. Whether the sentence
      means "some such endpoints" or "any such endpoints" is the whole disagreement, and it should be
      written down rather than left to the reader.
- [ ] No milestone is recorded as delivered until this is decided. `docs/maturity.md` reports
      predicates, not milestones, so nothing mechanical currently catches an M10 claim — which is
      why this is a story and not a check.

## Progress
- Filed 2026-07-30 while closing `M-23`, the fifth of the ICE epic's six children.
- 2026-07-30: **decided — the `Done when` sentence governs, and M10 does not require TURN.** Both
  texts in `docs/roadmap.md` now say that once: the M10 section states the sentence is the only exit
  criterion and gives three grounds for it, and the ICE epic heading lost its `(six stories, M10)`
  parenthetical and gained a paragraph saying `M-24` is in the epic and in no milestone. The grounds
  are that `M-27`'s test demonstrates the third clause without a relay, that the M10 table and
  `rfc-roadmap.md` group 2 both enumerate M10 as RFC 8445 + 8839 and never 8656,
  and that this roadmap ranks a deployability gap above a feature — a relay widens the coverage of a
  capability M10 delivers rather than delivering one M10 lacks.
- The third clause is settled in writing as **some** such endpoints, not any: host and reflexive
  candidates connect many NAT pairs, both ends behind symmetric NAT are connected by neither, and
  that residue is precisely what `M-24` buys.
- **M10 is still not recorded as reached, and the reason is not TURN.** Checking the evidence rather
  than the statuses: `T-20`'s test is an `OPTIONS` to one agent's GRUU, not two registrations of one
  address of record each taking a call, and `T-21`'s test stops when the INVITE arrives — nothing
  answers it. Only the ICE clause is demonstrated as written. The roadmap now says so, and says the
  remaining distance is one composing demonstration. That wants its own story.

## Notes
- **Reads with `M-24`**, which is the only thing standing between the two readings. If `M-24` lands
  before this is decided, the question dissolves and this story closes as moot — that is a fine
  outcome and worth saying so nobody treats the decision as blocking the work.
- Prior art for the failure this avoids: `X-30` removed roles a registry row claimed with no caller,
  `X-35` found `X-26`'s guard passing on a phantom claim, and `X-42` found a predicate reporting met
  while three open defects described it failing. All three are the same shape — a claim that was
  true of some reading and false of the one a reader would take.
