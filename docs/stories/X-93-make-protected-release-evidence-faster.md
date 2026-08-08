---
id: X-93
title: Make protected release evidence faster without weakening it
pillar: Build
status: in-progress
priority: 33
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

- [x] A read-only preflight before the expensive gate requires the successful exact-SHA `main` CI
      run and its successful Pages deployment job; missing or wrong-SHA evidence stops before the
      gate, while the post-consumer Pages job/HTTP proof remains unchanged.
- [x] An Actions-managed Rust artifact cache is restored only after immutable-tag validation. Its
      key covers runner, lockfile, CI flags, stable and MSRV toolchains, and native-feature inputs;
      it does not set a shared `CARGO_TARGET_DIR` or cache isolated registry/package consumers.
- [ ] Cold and warm timings are recorded. The cache is retained only if it saves at least 60
      seconds, and a miss or corrupt entry still runs every one of the gate's steps.
- [x] A measured exact-lock Node dependency cache may skip only installation, never site, anchor or
      rustdoc builds; it is retained only if its wall-time and storage tradeoff are material.
- [x] Publication models crates.io's new-crate rate limit explicitly: a first multi-crate release
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

- 2026-08-08: **implemented, two rows deliberately unticked.**

  *Failing-first.* Every structural rule was written against the workflow as it stood and proved
  red first: `./scripts/check-release-workflow.py --check` exited `1` with

  ```
  release workflow: specification does not put a read-only preflight in front of the gate
  release workflow: specification lets the preflight replace the post-consumer Pages proof
  release workflow: specification lets CI success substitute for a normative proof
  release workflow: specification does not place the artifact cache after immutable-tag validation
  release workflow: specification does not keep the isolated helper roots out of the cache
  release workflow: specification does not require every gate step to run on a cache miss
  release workflow: specification states no retention rule for the artifact cache
  release workflow: specification lets a Node dependency cache skip more than installation
  release workflow: read-only exact-SHA CI and Pages preflight is absent
  release workflow: Actions-managed Rust artifact cache is never restored
  release workflow: Actions-managed Rust artifact cache is never saved
  release workflow: release build cache key is not derived from named inputs
  ```

  and `python3 scripts/test-release-workflow.py` failed `CurrentWorkflow` plus
  `EvidenceMutations.test_pages_must_be_bound_to_sha_and_both_surfaces` — the latter because the
  three Pages-evidence rules became *counted* rather than merely present, so one surviving copy no
  longer answers for both. 33 tests then, 49 now, all green.

  *Row 1.* `Require exact-SHA main CI and Pages deployment evidence` sits between tag validation
  and the prerequisite install. Two read-only API calls, refusing on a missing run or a missing
  successful `deploy docs site` job. It does not curl anything: `preflight_problems` refuses a
  `curl` inside it and requires each public URL to appear exactly once in the file, so the
  post-consumer HTTP proof stays the only one.

  *Row 2.* `Derive the release build cache key` → `actions/cache/restore@v4`, both after
  `Validate the immutable annotated tag` and before the gate; `actions/cache/save@v4` after it.
  Key inputs are read from their sources — runner/arch/`ImageOS`, `sha256sum Cargo.lock`, the `env:`
  block of `ci.yml`, `rustc +stable -vV`, `rustc +"$msrv" -vV`, `pkg-config --modversion opus
  openssl alsa` — and each is separately mutation-tested. No `CARGO_TARGET_DIR` anywhere; no
  `restore-keys`; `path` is an exact allow-list of `~/.cargo/{registry/index,registry/cache,git/db}`
  and `target`. The isolated proofs are untouched by construction: `release.py` gives package
  rehearsal (`:480`), resume verification (`:1762`) and the registry consumer (`:1824`) their own
  `--target-dir` under the runner temp, and `consumer_environment` (`:1608`) strips any inherited
  `CARGO_HOME`/`CARGO_TARGET_DIR` before setting its own — including for the `cargo generate-lockfile`
  at `:1805` that fetches the checksums the resume proof compares against.

  *Row 3 — unticked, and this is the honest half.* Cold is real: six protected runs measured
  717 s, 804 s, 920 s, 943 s, 1068 s and 1079 s in `Run the complete release gate`
  (`31052427439`, `31029004601`, `31018659658`, `31002180570`, `30927185252`, `30955847937`), and
  the failed `30906820031` spent 757 s — the `12m37` this story argued from, confirmed against the
  run record for the first time. **Warm cannot be measured without a hosted protected run**, so the
  ≥60 s retention decision has no second number yet and no number was invented. What is in place is
  the mechanism that produces it: the gate now runs as `./scripts/gate.py --timings "$GATE_TIMINGS"`,
  `Record the gate's cold or warm timings` puts `cache.state`, the wall clock and the per-step table
  on the run summary, and `Preserve the gate timings record` keeps the JSON. Two protected runs —
  one miss, one hit — close this row. The "still runs every step" half *is* done and enforced:
  no normative step may carry an `if:`, and no step other than the cache's own save may be
  conditioned on `cache-hit`. Both cache steps are `continue-on-error: true`, so a corrupt, missing
  or failed entry costs a colder build directory and nothing else. The save is the one exception and
  it is required to be conditional: `actions/cache/save` builds its archive before discovering the
  key is taken, so saving after an exact restore would compress ten gigabytes for nothing and make a
  hit slower than a miss.

  *Row 4.* Measured rather than assumed. On the release runner of `31052427439`, `npm ci` reports
  `added 1405 packages, and audited 1406 packages in 34s` — 34.8 s between `==> building the site`
  and the next marker — inside a gate costing 717-1079 s, and `website/node_modules` is 745 MiB
  against a 10 GB repository-wide Actions cache budget it would share with a multi-gigabyte Rust
  entry. Not material: no `node_modules` cache is added, the `setup-node` npm *download* cache keyed
  on the exact `website/package-lock.json` stays, and the decision with its figures is in
  `docs/specs/release-workflow.md` §2.1. The "installation only" boundary is real either way —
  `build-docs.sh` installs only when `website/node_modules` is absent, and the checker refuses any
  cache path holding `website/build` or `target/doc`.

  *Row 5 — X-119, verified rather than assumed, with two gaps closed.* `release.py` parses a
  registry-stated deadline from `Retry-After` or a `try again after` body (`_retry_hint_seconds`
  `:1219`, `rate_limit_refusal` `:1243`) and waits on it (`rate_limit_restate` `:1191`,
  `rate_limit_ready_at` `:1165`), holds a finite budget decremented across a frontier
  (`publish_frontier` `:1297-1314`), and resumes from checksum-proven visible archives
  (`verify_resume_bytes` `:1740`, `_registry_checksums` `:1679`). Two clauses were **not** covered:
  no test drove more than one 429 at a package, so the `RATE_LIMIT_RETRY_ATTEMPTS` branch
  (`:1334-1340`) had zero coverage; and "not delayed *merely because* first-name creation once
  required pacing" was proved only for a frontier with no new names at all, which cannot observe the
  two buckets interfering. Both closed in `scripts/test-release.py`:
  `test_a_registry_that_keeps_refusing_stops_after_the_stated_attempt_bound` and
  `test_a_new_name_being_paced_does_not_delay_an_existing_name_beside_it`. Each was mutation-proved
  — `RATE_LIMIT_RETRY_ATTEMPTS = 4` makes the first raise nothing, and collapsing
  `kind = NEW_CRATE if package in names else NEW_VERSION` to one bucket moves `sipx-sdp` from
  900.0 s to 1500.0 s in the second, which is the delay the clause forbids in exactly one number.

  *Row 6 — unticked because it ends in "the complete gate stays green", which is the wave gate.*
  The structural half is done: `PreflightMutations`, `BuildCacheMutations` and
  `SpeedSpecificationMutations` (16 new cases) refuse a cache restored before tag validation, a
  cache reaching the isolated consumer roots, a shared `CARGO_TARGET_DIR`, a normative step made
  conditional, a step conditioned on `cache-hit`, a dropped `--timings`, a preflight that probes the
  site, and every spec clause that would permit any of it. `RWF-10` and `RWF-11` name the vectors.

  *Out of scope, filed as `X-126`:* the rate-limit budget resets on every frontier rerun so only the
  job timeout bounds a whole publication; neither workflow passes
  `--registry-retry-budget-seconds`; `docs/specs/release-rehearsal.md:213-215` says an unreadable
  name probe refuses the invocation while `release.py:2122-2129` paces and continues; and `main`'s
  own `new_crates` derivation has no test.

## Notes

- Do not remove or parallelize the release gate under this story. A design that wants exact-SHA CI
  to substitute for it changes `docs/specs/release-workflow.md` and needs a separate authority
  review.
