---
id: A-23
title: Prove the bridge against the live endpoint
pillar: Application
status: backlog
priority: 4
design: docs/designs/openai.md
epic: openai
areas: [sipx-app, interop, security]
predicate:
announcement:
note: opt-in and credentialed — the first such proof in the repo; disclaim-don't-skip, evidence recorded once
---

# Prove the bridge against the live endpoint

## Goal

One real call bridged to the live OpenAI realtime endpoint, under a harness that is safe to
own a billable credential: bounded to exactly one call, disclaiming when it cannot run,
recording evidence a stranger can audit.

## Acceptance

- [ ] An opt-in harness places one bounded call through A-22's product path against the live
      endpoint: credential resolved by *name* (host-config N7 form), never in argv, environment
      dumps, logs or captured output — asserted by the harness's own self-test.
- [ ] Absent credential, or an unreachable endpoint, is a disclaimed run: `EX_TEMPFAIL` from
      the guard, exit `2` overall, reported under a heading that is not a finding — never a
      silent pass, never a skip (the gate's `X-34`/`X-58` doctrine).
- [ ] The harness is process-group-safe per non-negotiable 5: `EXIT`/`INT`/`TERM` trap
      terminating its entire process group, cleanup awaited before the result is reported,
      a hard end-to-end timeout, and *exactly one* call even when the peer misbehaves — the
      self-test proves the bound by making the peer misbehave.
- [ ] Facts asserted, not exit codes: session established over `wss` with the pinned
      verification, G.711 session format confirmed, the agent's reply present as
      non-silence in the call's inbound RTP, barge-in exercised once. Each failure names
      which spec row it violates.
- [ ] An adversarial self-test of the harness runs in the gate job (`scripts/test-*.py`
      pattern): a harness that observed nothing cannot report green.
- [ ] CI: manual or schedule-only job, never the default matrix; the interop README's peer
      criteria are amended to name this class of peer (credentialed, opt-in) explicitly
      rather than leaving the precedent implicit.
- [ ] Evidence recorded in this story's Progress the way A-15 records publication evidence:
      run id, date, model, negotiated facts, and the spec's observation date confirmed or
      updated. If the live endpoint contradicts the spec, the finding is filed as a story
      and the spec updated — not patched around in the harness.

## Progress

- (running log / checklist — a resuming agent reads this to know exactly where things stand)

## Notes

- Design: `docs/designs/openai.md` component 5. Blocked on A-22.
- The credential is billable: the one-call bound is a safety property, not a courtesy.
- Precedents: `tests/interop/run.sh` (bounded shell proof), `scripts/test-interop-run.py`
  (adversarial harness self-test), `docs/stories/A-15-publish-beta4.md` (evidence shape).
