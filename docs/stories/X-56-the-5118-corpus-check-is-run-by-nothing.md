---
id: X-56
title: Run the RFC 5118 corpus check from the gate, as its RFC 4475 twin already is
pillar: Build
status: done
priority: 3
epic: conformance
areas: [scripts, ci]
note: found by X-51's evidence check — `import-rfc5118-corpus.sh --check` proves the twelve fixtures are still the RFC's own bytes, and no gate step and no CI job invokes it; the 4475 twin is run by the fuzz job
---

# Run the RFC 5118 corpus check from the gate, as its RFC 4475 twin already is

## Goal
Make the RFC 5118 fixtures as tamper-evident as the RFC 4475 ones already are, so M12's first clause
keeps holding rather than merely holding today.

## Acceptance
- [x] **The gap, demonstrated first.** `scripts/import-rfc5118-corpus.sh --check` verifies the twelve
      Appendix A messages against the RFC's own archive. Grep the repository: nothing invokes it —
      not `scripts/gate.py`, not any job in `.github/workflows/ci.yml`. Its RFC 4475 counterpart *is*
      invoked, by the `fuzz` job. Show that a fixture edited by hand leaves the whole gate green.
- [x] **It runs from a gate step and a CI job**, and `./scripts/gate.py --check` accounts for it —
      every command a CI job runs is either mirrored by a gate step or named in `NOT_RUN_LOCALLY`
      with a reason, which is the property `X-22` established.
- [x] **The failing-first test is the tampered fixture.** Edit one byte of one 5118 message, show the
      new step red, restore it, show it green. A check that cannot detect a hand-edited corpus is the
      thing this story is about.
- [x] Consider whether the 4475 and 5118 checks belong in the same step or job, and say which and
      why. Two corpora with one rule between them is one place to remember; two steps is two.

## Progress
- Filed 2026-07-30 by `X-51`, which ran the check by hand while verifying M12's first clause, found
  it passing, and noticed nothing would ever notice if it stopped.
- Closed 2026-07-31. **One CI job, one step per corpus**, which is the last item's answer: two
  corpora with one rule between them is one place to remember, while a step each is what makes a
  red result name which corpus drifted. The `fuzz` job keeps its own RFC 4475 invocation, because
  that one runs after the fuzzer in the tree the fuzzer wrote to and is a different claim — that a
  campaign deposited none of its generated inputs into committed seed data. A step in another job
  checks out a fresh tree and cannot see that at all; `test-gate.py` now pins the ordering so
  folding the two together cannot silently lose it.
- The gap was demonstrated at the base by the step list rather than by a full run: no gate step
  named either importer and `ci.yml` did not contain the string `5118` anywhere, so a hand-edited
  5118 fixture had nothing that could observe it. Four `test-gate.py` cases were red there.
- **Failing-first, the tampered fixture:** one byte of `rfc5118/ipv6-good` flipped (`7` → `8`) and
  the new step exits 1 naming the file that differs; restored, it exits 0. Inside the real gate
  both steps pass over 50 RFC 4475 messages and 12 RFC 5118 messages, and `gate.py --check` reports
  25 steps over 17 CI jobs with none unaccounted for.
- Both steps reach the network, so both importers guard the fetch. It stays a failure rather than
  becoming a skip: a provenance check that passes when it could not reach the RFC is the MSRV hole
  in a second place.
- **Corrected by `X-58`, and the correction belongs here rather than only there.** Two sentences
  written into this record were wrong, and one of them was copied into `AGENTS.md`, where it is
  what the next agent reads as the why:
  - "`curl -f` prints nothing" — the flags in use are `-fsSL`, and `-S` is *show errors*, so curl
    prints `curl: (6) Could not resolve host: www.rfc-editor.org` and exits 6. One command
    disproves it. The guard is still worth having, but for what it does: it names the corpus and
    the host in a sentence, and it exits a code that means "not a result".
  - "these are the gate's first network-dependent steps" — `docs site` has reached the network
    since it existed. `scripts/build-docs.sh` runs `npm ci` whenever `website/node_modules` is
    absent, and that directory is gitignored, so it is absent in every fresh worktree.
  - And the guard exiting 1 put an unreachable RFC editor in the failed tally, so `gate.py` printed
    `N of 25 steps failed` naming a corpus it had not read a byte of. `X-58` made it exit
    `EX_TEMPFAIL`, which the gate reports as a non-result — `X-34`'s exit-code contract, which the
    guard shipped here without honouring.

  The wiring itself — two gate steps, a `corpus` CI job, `gate.py --check` accounting for both, a
  tampered fixture caught — was reproduced independently and stands. The fetch guard was a
  coordinator addition on top of this story's Acceptance, and it is the part that was wrong.
- The implementor wrote the test cases and was then killed by an org monthly spend limit before
  writing any implementation. Its test was a precise specification and the coordinator implemented
  against it, so this story is `coordinator-implemented`: one pair of eyes, not two.

## Notes
- **This is `X-31`'s lesson in a second corpus.** `X-31` made the 4475 corpus tamper-evident because
  a fixture that can be quietly edited is a conformance claim that can be quietly weakened. The 5118
  import script was written with the same `--check` mode and then wired to nothing.
- M12's first clause — "the whole 5118 corpus is classified and green" — genuinely holds today;
  `X-51` verified it and `crates/sipx-testkit/src/rfc5118.rs:300`'s `DEVIATIONS` list is empty. This
  story is about the claim staying true, not about it being false.
