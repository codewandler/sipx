---
id: X-93
title: Make protected release evidence faster without weakening it
pillar: Build
status: backlog
priority:
design: docs/specs/release-workflow.md
epic: conformance
areas: [release, ci]
predicate:
announcement:
note: measure cache and preflight changes against the 12m37 cold beta gate · follow-up
---

# Make protected release evidence faster without weakening it

## Goal

Reduce protected-release feedback time while retaining every independent claim the release specs
require: the complete gate, fresh package bytes, registry checksums, exact consumer, installed CLI,
Pages binding, and immutable-tag authority.

## Acceptance

- [ ] A read-only preflight before the expensive gate requires the successful exact-SHA `main` CI
      run and its successful Pages deployment job; missing or wrong-SHA evidence stops before the
      gate, while the post-consumer Pages job/HTTP proof remains unchanged.
- [ ] An Actions-managed Rust artifact cache is restored only after immutable-tag validation. Its
      key covers runner, lockfile, CI flags, stable and MSRV toolchains, and native-feature inputs;
      it does not set a shared `CARGO_TARGET_DIR` or cache isolated registry/package consumers.
- [ ] Cold and warm timings are recorded. The cache is retained only if it saves at least 60
      seconds, and a miss or corrupt entry still runs every one of the gate's steps.
- [ ] A measured exact-lock Node dependency cache may skip only installation, never site, anchor or
      rustdoc builds; it is retained only if its wall-time and storage tradeoff are material.
- [ ] Publication models crates.io's new-crate rate limit explicitly: a first multi-crate release
      uses registry-provided retry deadlines or measured conservative pacing, retains a finite total
      bound, and resumes from checksum-proven visible archives without manual deadline arithmetic.
      Tests cover repeated 429 responses and prove that an ordinary version update is not delayed
      merely because first-name creation once required pacing.
- [ ] Structural tests refuse cache placement before tag validation, loss of the complete gate, or
      substitution of CI success for any normative release proof; the complete gate stays green.

## Progress

- Exact-SHA CI run `30906258443` completed in 6m41 with parallel cached jobs. Protected release run
  `30906820031` then spent 12m37 in the same commit's serial cold gate and failed after 13m19 total.
  The missing provenance input was visible 33 seconds into the gate; `X-92` now handles that
  configuration case before the gate starts.
- The release workflow currently caches npm downloads but not Rust artifacts or `node_modules`.
  Its gate reuses one ordinary workspace target across serial steps, which is intentional. Package
  rehearsal/frontier/consumer targets and Cargo homes stay isolated because their independence is
  part of the release evidence rather than incidental build work.
- Beta.2 created eleven new crate names. crates.io accepted the first four in one frontier, then
  enforced approximately ten-minute creation windows: recovery attempts added four, one, one and
  finally `sipx-cli`. The checksum-bound resume path kept this safe, but a human had to read each
  429 deadline, rerun and approve the protected environment. The beta.3 workflow must turn that
  observed registry behavior into bounded controller policy rather than rediscovering it live.

- 2026-08-08: **readiness audit — split required before implementation.** The `12m37`/`6m41`/`13m19`
  baseline exists only as prose in this file: it appears in no release record, review or changelog,
  and there is no machine-readable timing store. `scripts/gate.py` has **no clock at all** — it
  reports a step banner, a step count and disk, nothing temporal — so acceptance row 3 cannot be met
  until step-timing instrumentation exists, and that instrumentation is the real first story. Row 5
  (registry 429 pacing in `release.py`) shares nothing with rows 1-4 and belongs on its own. Row 6
  needs a spec edit because `check-release-workflow.py` greps the spec text. Deferred out of rc.4.

## Notes

- Do not remove or parallelize the release gate under this story. A design that wants exact-SHA CI
  to substitute for it changes `docs/specs/release-workflow.md` and needs a separate authority
  review.
