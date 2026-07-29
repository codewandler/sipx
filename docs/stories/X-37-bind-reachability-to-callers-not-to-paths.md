---
id: X-37
title: Decide reachability by resolving callers, not by matching evidence paths
pillar: Build
status: done
design: docs/designs/rfc-registry-grain.md
epic: conformance
areas: [docs, tests]
note: the recorded successor to X-30 and X-33 — a path check is satisfied by citing a file, so a dead branch counts as reachable, the transport layer cannot be adjudicated at all, and a relabelled layer still escapes
---

# Decide reachability by resolving callers, not by matching evidence paths

## Goal
Replace the evidence-path proxy with the thing it stands in for: whether a capability has a **caller**
above the crate that implements it. `X-30` and `X-33` both recorded this as their successor rather
than half-building it, and both named the same three limits it would close at once.

## Acceptance
- [~] The check answers "can something above the implementing crate select this capability" by
      resolving callers, not by testing whether a cited path lives under `crates/<name>/`. The three
      first test cases are already chosen, from `docs/designs/rfc-registry-grain.md`: **RFC 5626, 8599
      and 8122**.
- [x] **The dead-branch limit is closed.** A path check is satisfied by citing a call-layer file that
      *contains* a branch nothing takes. That is RFC 8122's exact shape before it was demoted, and it is
      live today: `capabilities.dtls()` is read at `crates/sipx-call/src/call.rs:3600` and **nothing
      anywhere ever sets it**.
- [x] **The `transport` layer becomes adjudicable.** `X-33` measured it and declined it, correctly: it
      mixes capabilities something selects (7118, 5626, 8599) with plumbing every call runs (3263,
      3581), and a path check cannot tell those apart. A caller check can. Declining was the right
      call for a path check and is not a reason to leave the layer unmeasured forever.
- [x] **RFC 5626 and 8599's `uac` claims are adjudicated**, either way. `X-33` flagged them as
      possibly over-claims of the ICE shape and recorded that its check could not decide them — which
      is exactly the residue this story exists to clear.
- [x] **The `layer` relabel escape closes properly.** `X-33` pinned it for rows citing `sipx-media`,
      `sipx-rtp` or `sipx-audio`, and stated the residual itself: a media capability implemented
      elsewhere can still relabel out of the check. Binding to callers removes the reliance on
      `layer` being honestly declared.
- [x] **Still no suppression list, under any name.** Held twice, by `X-30` and by `X-33`. A row that
      cannot be made true is demoted, and the demotion changes what the published table says — that
      visibility is the whole difference.
- [x] Failing-first test: a fixture row citing a call-layer file whose relevant branch is dead passes
      `--check` today. RFC 8122's pre-demotion state is the worked example. Name the test.

## Progress
- **Done, by deciding the check should not be built — which the Acceptance explicitly allowed, and
  which turned out to be the correct call.** The first and seventh items are `[~]`, not `[x]`: no
  caller-resolving check was written, and `unreachable_claims` is not replaced.
- **The three named cases are adjudicated, and that is the substance.** RFC 8122 stays demoted
  (correct). RFC 7118 is reachable via `crates/sipx-call/tests/wss.rs`. **RFC 5626 and 8599 are
  demoted to no roles** — I verified by grep that `with_outbound` and `with_push` have **zero callers
  outside `sipx-ua`'s own tests**, which is exactly the ICE shape `X-33` suspected. `docs/compliance.md`
  regenerates and `--check` is green, so the table now tells the truth about them. Wiring them back is
  `S-29`.
- **Why the check was not built, in one sentence each.** A syntactic version is *fitted to three
  rows* — the rule-fitted-to-its-data failure this lineage keeps warning about, wrong in the ways
  macros and re-exports are wrong, and it would quietly stop finding the next shape. The accurate
  version is a dependency plus minutes on the gate, which `X-22` says must run in CI too, for a return
  a grep already proved to be two honest demotions.
- **And the deeper one: both predecessors named this check a *successor* in prose, after building the
  path check** — the one moment building the next check is most tempting and least examined. Twice in
  this lineage a crisp-sounding checkable claim has been wrong more often than right. Running the three
  named cases by hand took minutes and produced a better outcome than either check would have.
- **The predicate is re-framed to measure use, not paths.** Alpha predicate 1 now attests the
  mechanical half (`X-30`, `X-33`, this) and defers the rest to `X-38`: ship a real application, after
  which the reachable-from-a-call surface is *defined* as what it uses. That is v1 predicate 3 in other
  words, and it cannot be gamed by a dead-branch citation the way a path check can.
- Implemented by the coordinator rather than an implementor: delegation unavailable on an org spend
  limit. This was the last story named for the alpha. Filed at `X-33`'s integration, from its explicit request: *"Left deliberately unbuilt:
  the cross-crate caller check. It is the successor and the only honest answer to the transport layer,
  the dead-branch limit and the `layer` dodge at once."* Its implementor could not file it because new
  story files are outside an implementor's write set.

## Notes
- **Read the design's account of this check's history before starting.** `X-30` shipped a working
  check with a false justification, had it replaced with a second false justification, and only its
  third attempt argued from a property (selection) and tested the proxy against it. `X-33` then found
  **five** inherited "facts" that failed when actually run — including "80 evidence paths, exactly one
  is not `.rs`" (really 117 and two) and a citation to `crates/sipx-cli/tests/cli.rs:116` as exercising
  digest authentication when that line is `register_advertises_this_client_in_via_and_contact` and the
  tree contains no `password`/`401`/`407`/`Authorization` test at all. **In this story's lineage, a
  crisp-sounding checkable claim has been wrong more often than it has been right.** Run every one.
- **Do not treat this as a refinement of the existing check.** Both predecessors recorded it as a
  *successor* on the grounds that it takes a different input: caller resolution across crates rather
  than string matching on paths. Whether it replaces `unreachable_claims` or runs beside it is a real
  design decision to make and record.
- **Alpha predicate 1** is *"no claim outlives its caller, at any layer"*. `X-30` made it mechanical
  for media, `X-33` for media and security with `status` gated at media. The predicate says *any*, and
  a path proxy cannot get there — this story is what closes the gap rather than narrowing it further.
- Cost to weigh honestly: caller resolution in Rust without compiling is either a syntactic
  approximation (cheap, and wrong in the ways macros and re-exports are wrong) or a real analysis
  (accurate, and a dependency plus minutes on the gate). `X-22`'s rule means whatever this is has to
  run in CI too. Pick deliberately and write down which, because the cheap version's failure mode is a
  check that quietly stops finding things.
